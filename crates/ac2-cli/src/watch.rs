//! Live terminal views (`--watch`, `spl watch`) and the keyboard used by `gen`.
//!
//! On a terminal: alternate screen, raw mode, redraw at most 10 Hz, q / Esc / Ctrl-C quit.
//! With `--json`: one JSON line per new frame (at most 10 Hz). Otherwise (piped): one text
//! line per new frame. The data socket is drained before every redraw.

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ac2_client::{Client, Latest, MirrorView};
use ac2_proto::model::MeasKind;
use ac2_proto::units::MeasId;
use ac2_proto::{FrameData, Stream, Subscription, Topic};
use ac2_scene::format;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, queue, terminal};
use serde_json::json;
use tokio::sync::mpsc;

use crate::CliError;
use crate::cmd::rate;
use crate::output::{self, Out};

/// Redraw period: 10 Hz.
pub const REDRAW: Duration = Duration::from_millis(100);

/// A key the live views and `gen` react to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// q, Esc, Ctrl-C.
    Quit,
    /// Enter.
    Enter,
}

/// Raw mode (+ alternate screen) for the lifetime of the guard; restored on drop, including
/// on early returns.
pub struct RawTerm {
    alt: bool,
    stop: Arc<AtomicBool>,
}

impl RawTerm {
    /// Enters raw mode and starts a key reader thread.
    pub fn enter(alt: bool) -> std::io::Result<(Self, mpsc::UnboundedReceiver<Key>)> {
        terminal::enable_raw_mode()?;
        if alt {
            execute!(
                std::io::stdout(),
                terminal::EnterAlternateScreen,
                cursor::Hide
            )?;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::unbounded_channel();
        let st = stop.clone();
        std::thread::spawn(move || {
            while !st.load(Ordering::Acquire) {
                match event::poll(Duration::from_millis(50)) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(_) => return,
                }
                let Ok(Event::Key(k)) = event::read() else {
                    continue;
                };
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                let key = match k.code {
                    KeyCode::Esc | KeyCode::Char('q') => Some(Key::Quit),
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        Some(Key::Quit)
                    }
                    KeyCode::Enter => Some(Key::Enter),
                    _ => None,
                };
                if let Some(key) = key
                    && tx.send(key).is_err()
                {
                    return;
                }
            }
        });
        Ok((Self { alt, stop }, rx))
    }
}

impl Drop for RawTerm {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if self.alt {
            let _ = execute!(
                std::io::stdout(),
                cursor::Show,
                terminal::LeaveAlternateScreen
            );
        }
        let _ = terminal::disable_raw_mode();
    }
}

/// One rendering of a live view.
pub struct View {
    /// Text lines.
    pub lines: Vec<String>,
    /// JSON line.
    pub json: serde_json::Value,
    /// Changes when there is something new to print in line modes.
    pub key: Vec<u64>,
}

const NOT_RESPONDING: &str = "DAEMON NOT RESPONDING";

async fn live(
    c: &Client,
    out: &mut Out<'_>,
    subs: &[Subscription],
    mut render: impl FnMut(&MirrorView, &Latest) -> View,
) -> Result<(), CliError> {
    for s in subs {
        c.subscribe(*s)?;
    }
    let tty = !out.json && std::io::stdout().is_terminal();
    let (term, mut keys) = if tty {
        let (t, k) = RawTerm::enter(true)?;
        (Some(t), k)
    } else {
        (None, mpsc::unbounded_channel().1)
    };
    let mut tick = tokio::time::interval(REDRAW);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_key: Option<Vec<u64>> = None;
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let result = loop {
        tokio::select! {
            _ = &mut ctrl_c => break Ok(()),
            k = keys.recv(), if term.is_some() => {
                if matches!(k, Some(Key::Quit) | None) { break Ok(()); }
            }
            _ = tick.tick() => {
                let latest = match c.latest() {
                    Ok(l) => l,
                    Err(e) => break Err(e.into()),
                };
                let view = render(&c.view(), &latest);
                if tty {
                    let mut w = std::io::stdout();
                    let _ = queue!(w, cursor::MoveTo(0, 0), terminal::Clear(terminal::ClearType::All));
                    let mut text = view.lines.join("\r\n");
                    text.push_str("\r\n\r\nq / Esc / Ctrl-C: quit");
                    let _ = write!(w, "{text}");
                    let _ = w.flush();
                } else if last_key.as_ref() != Some(&view.key) {
                    last_key = Some(view.key.clone());
                    let r = if out.json {
                        out.json_line(&view.json)
                    } else {
                        writeln!(out.w, "{}", view.lines.join("\n")).and_then(|()| out.w.flush())
                    };
                    if let Err(e) = r { break Err(e.into()); }
                }
            }
        }
    };
    drop(term);
    for s in subs {
        let _ = c.unsubscribe(*s);
    }
    result
}

/// This machine's wall clock, Unix ns.
fn now_wall() -> ac2_proto::units::WallNs {
    ac2_proto::units::WallNs(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)),
    )
}

fn age_text(age: Option<f64>, stale: bool) -> String {
    let a = age.map_or_else(|| format::NO_VALUE.to_owned(), format::age);
    if stale { format!("{a} STALE") } else { a }
}

fn now_secs_key(latest: &Latest, topic: &Topic) -> Vec<u64> {
    latest.get(topic).map_or(vec![0], |t| {
        vec![
            t.frame.stamp.seq,
            u64::from(t.stale),
            u64::from(latest.responding),
        ]
    })
}

/// `spl watch`.
pub async fn spl(c: &Client, meas: MeasId, out: &mut Out<'_>) -> Result<(), CliError> {
    let topic = Topic::Data {
        meas,
        stream: Stream::Spl,
    };
    live(c, out, &[Subscription::Topic(topic)], |view, latest| {
        let mut lines = Vec::new();
        if !latest.responding {
            lines.push(NOT_RESPONDING.to_owned());
        }
        let key = now_secs_key(latest, &topic);
        // The input's mic name, from the mirrored measurement and input setup.
        let mic = view.state.as_ref().and_then(|s| {
            let input = s.measurements.iter().find(|m| m.id == meas).and_then(|m| {
                match &m.config.kind {
                    MeasKind::Spl { config } => Some(config.input),
                    _ => None,
                }
            })?;
            s.inputs
                .iter()
                .find(|i| i.channel == input)
                .and_then(|i| i.mic.clone())
        });
        let Some(tf) = latest.get(&topic) else {
            lines.push(format!("waiting for {topic} …"));
            return View {
                lines,
                json: json!({ "topic": topic.to_string(), "frame": null }),
                key,
            };
        };
        let FrameData::Spl(f) = &tf.frame.data else {
            return View {
                lines,
                json: json!(null),
                key,
            };
        };
        let m = &f.meta;
        let name = format!(
            "L{}{}",
            output::weighting(m.weighting),
            output::time_weighting(m.time_weighting)
        );
        lines.push(format!(
            "{name} {}   Leq {}   Lmax {}   Lmin {}   L{}peak {}",
            output::level(m.level, m.scale),
            format::level(m.leq),
            format::level(m.lmax),
            format::level(m.lmin),
            output::peak_weighting(m.peak_weighting),
            format::level(m.lpeak),
        ));
        lines.push(format!(
            "integrated {}   age {}",
            format::duration(m.duration.0),
            age_text(tf.age, tf.stale)
        ));
        let offset = view.clock_offset_ns.map_or(0, |o| {
            o.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
        });
        let cal = output::cal_status(m.cal, m.mic_curve, now_wall(), offset);
        let cal = match &mic {
            Some(name) => format!("{name} · {cal}"),
            None => cal,
        };
        lines.push(cal.clone());
        View {
            lines,
            json: json!({
                "topic": topic.to_string(),
                "seq": tf.frame.stamp.seq,
                "age_s": tf.age,
                "stale": tf.stale,
                "responding": latest.responding,
                "spl": m,
                "mic": mic,
                "cal_text": cal,
            }),
            key,
        }
    })
    .await
}

/// `timing --watch`.
pub async fn timing(c: &Client, out: &mut Out<'_>) -> Result<(), CliError> {
    let topic = Topic::Timing;
    live(c, out, &[Subscription::Topic(topic)], |view, latest| {
        let mut lines = Vec::new();
        if !latest.responding {
            lines.push(NOT_RESPONDING.to_owned());
        }
        let rate = view.state.as_deref().and_then(rate);
        let key = now_secs_key(latest, &topic);
        if let Some(g) = &view.generator {
            lines.push(format!(
                "generator {}{}",
                if g.firing {
                    "FIRING"
                } else if g.armed {
                    "armed"
                } else {
                    "idle"
                },
                g.owner
                    .as_ref()
                    .map_or_else(String::new, |o| format!(" (owner {})", o.0))
            ));
        }
        let Some(tf) = latest.get(&topic) else {
            if let Some(t) = &view.timing {
                lines.push(format!("timing   {}", output::timing_state(t, rate)));
            }
            lines.push("waiting for timing frames …".to_owned());
            return View {
                lines,
                json: json!({ "topic": "timing", "frame": null, "ka_timing": view.timing }),
                key,
            };
        };
        let FrameData::Timing(t) = &tf.frame.data else {
            return View {
                lines,
                json: json!(null),
                key,
            };
        };
        lines.push(output::timing(&t.status, rate));
        if let Some(w) = &t.window {
            lines.push(format!(
                "window   offset {}   PSR {}   loopback {}   stimulus {}",
                w.offset.map_or_else(
                    || format::NO_VALUE.to_owned(),
                    |o| format!("{} samples", o.0)
                ),
                w.psr
                    .map_or_else(|| format::NO_VALUE.to_owned(), |p| format::db_readout(p.0)),
                output::dbfs(w.loopback.0),
                output::dbfs(w.stimulus.0),
            ));
        }
        lines.push(format!("age {}", age_text(tf.age, tf.stale)));
        View {
            lines,
            json: json!({
                "topic": "timing",
                "seq": tf.frame.stamp.seq,
                "age_s": tf.age,
                "stale": tf.stale,
                "responding": latest.responding,
                "timing": t,
            }),
            key,
        }
    })
    .await
}

fn main_stream(k: &MeasKind) -> Stream {
    match k {
        MeasKind::Transfer { .. } => Stream::Tf,
        MeasKind::Spectrum { .. } => Stream::Spec,
        MeasKind::Rta { .. } => Stream::Rta,
        MeasKind::Spl { .. } => Stream::Spl,
    }
}

/// `meas list --watch`.
pub async fn meas_list(c: &Client, out: &mut Out<'_>) -> Result<(), CliError> {
    let started = Instant::now();
    live(c, out, &[Subscription::AllData], |view, latest| {
        let mut lines = Vec::new();
        if !latest.responding {
            lines.push(NOT_RESPONDING.to_owned());
        }
        let Some(state) = view.state.as_deref() else {
            lines.push(format!(
                "syncing state … ({})",
                format::age(started.elapsed().as_secs_f64())
            ));
            return View {
                lines,
                json: json!({ "synced": false }),
                key: vec![u64::MAX],
            };
        };
        let mut t = output::table(&["id", "name", "kind", "running", "stream", "age"]);
        let mut rows = Vec::new();
        let mut key = vec![view.rev.0, u64::from(latest.responding)];
        for m in &state.measurements {
            let topic = Topic::Data {
                meas: m.id,
                stream: main_stream(&m.config.kind),
            };
            let tf = latest.get(&topic);
            key.push(tf.map_or(0, |t| t.frame.stamp.seq));
            key.push(tf.map_or(0, |t| u64::from(t.stale)));
            t.add_row(vec![
                m.id.to_string(),
                m.config.name.clone(),
                output::meas_kind(&m.config.kind),
                output::yes(m.running),
                topic.to_string(),
                tf.map_or_else(|| format::NO_VALUE.to_owned(), |t| age_text(t.age, t.stale)),
            ]);
            rows.push(json!({
                "id": m.id,
                "name": m.config.name,
                "running": m.running,
                "topic": topic.to_string(),
                "seq": tf.map(|t| t.frame.stamp.seq),
                "age_s": tf.and_then(|t| t.age),
                "stale": tf.map(|t| t.stale),
            }));
        }
        lines.extend(t.to_string().lines().map(str::to_owned));
        View {
            lines,
            json: json!({ "rev": view.rev, "responding": latest.responding, "measurements": rows }),
            key,
        }
    })
    .await
}

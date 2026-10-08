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

/// Resolves when the process is asked to quit: Ctrl-C, and on Unix also SIGTERM and SIGHUP
/// (a closed terminal or SSH session), so a foreground command always gets to clean up
/// (stop its stimulus, delete its own meter) instead of leaving it behind.
pub async fn quit_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut hup)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::hangup()),
        ) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
            _ = hup.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn live(
    c: &Client,
    out: &mut Out<'_>,
    subs: &[Subscription],
    render: impl FnMut(&MirrorView, &Latest) -> View,
) -> Result<(), CliError> {
    live_until(c, out, subs, None, render).await
}

async fn live_until(
    c: &Client,
    out: &mut Out<'_>,
    subs: &[Subscription],
    until: Option<Instant>,
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
    let quit = quit_signal();
    tokio::pin!(quit);
    let result = loop {
        tokio::select! {
            _ = &mut quit => break Ok(()),
            k = keys.recv(), if term.is_some() => {
                if matches!(k, Some(Key::Quit) | None) { break Ok(()); }
            }
            _ = tick.tick() => {
                if until.is_some_and(|u| Instant::now() >= u) {
                    break Ok(());
                }
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
pub(crate) fn now_wall() -> ac2_proto::units::WallNs {
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
/// `spl watch` of measurement `meas` on (zero-based) `input`.
pub async fn spl(
    c: &Client,
    meas: MeasId,
    input: u16,
    until: Option<Instant>,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    let topic = Topic::Data {
        meas,
        stream: Stream::Spl,
    };
    live_until(
        c,
        out,
        &[Subscription::Topic(topic)],
        until,
        |view, latest| {
            let mut lines = Vec::new();
            if !latest.responding {
                lines.push(NOT_RESPONDING.to_owned());
            }
            let key = now_secs_key(latest, &topic);
            // The input's mic name, from the mirrored input setup.
            let mic = view.state.as_ref().and_then(|s| {
                s.inputs
                    .iter()
                    .find(|i| i.channel == input)
                    .and_then(|i| i.mic.clone())
            });
            // 1-based, as the operator typed it.
            let input_no = u32::from(input) + 1;
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
            // Every level above includes the measuring-position correction: said, never
            // left to be guessed.
            if let Some(p) = &m.position {
                lines.push(ac2_scene::leq::position_text(p));
            }
            let offset = view.clock_offset_ns.map_or(0, |o| {
                o.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
            });
            let cal = output::cal_status(m.cal, m.mic_curve, now_wall(), offset);
            let cal = match &mic {
                Some(name) => format!("{name} · {cal}"),
                None => cal,
            };
            let cal = format!("in {input_no} · {cal}");
            lines.push(cal.clone());
            View {
                lines,
                json: json!({
                    "topic": topic.to_string(),
                    "meas": meas.0,
                    "input": input_no,
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
        },
    )
    .await
}

/// Three-row block digits for the Leq view: `0`–`9`, `.`, `−`, `—`.
fn big_glyph(c: char) -> [&'static str; 3] {
    match c {
        '0' => ["█▀█", "█ █", "▀▀▀"],
        '1' => [" ▀█", "  █", "  ▀"],
        '2' => ["▀▀█", "█▀▀", "▀▀▀"],
        '3' => ["▀▀█", " ▀█", "▀▀▀"],
        '4' => ["█ █", "▀▀█", "  ▀"],
        '5' => ["█▀▀", "▀▀█", "▀▀▀"],
        '6' => ["█▀▀", "█▀█", "▀▀▀"],
        '7' => ["▀▀█", "  █", "  ▀"],
        '8' => ["█▀█", "█▀█", "▀▀▀"],
        '9' => ["█▀█", "▀▀█", "▀▀▀"],
        '.' => [" ", " ", "▄"],
        '\u{2212}' | '-' | '—' => ["   ", "▀▀▀", "   "],
        _ => ["   ", "   ", "   "],
    }
}

/// `text` (a level) in block digits, three rows.
pub(crate) fn big_number(text: &str) -> [String; 3] {
    let mut rows = [String::new(), String::new(), String::new()];
    for c in text.chars() {
        let g = big_glyph(c);
        for (r, part) in rows.iter_mut().zip(g) {
            r.push_str(part);
            r.push(' ');
        }
    }
    rows
}

/// `spl leq watch` of SPL meter `meas`: on a terminal each window as a big number with its
/// state, limit and headroom; piped, one line per window per second; `--json`, one line per
/// second.
pub async fn leq(
    c: &Client,
    meas: MeasId,
    until: Option<Instant>,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    use ac2_scene::leq::{TileState, leq_tiles};
    let topic = Topic::Data {
        meas,
        stream: Stream::Leq,
    };
    let big = !out.json && std::io::stdout().is_terminal();
    live_until(
        c,
        out,
        &[Subscription::Topic(topic)],
        until,
        |view, latest| {
            let mut lines = Vec::new();
            let key = latest
                .get(&topic)
                .map_or(vec![0], |t| vec![t.frame.stamp.seq]);
            if view.state.is_none() {
                lines.push("waiting for the daemon's state …".to_owned());
                return View {
                    lines,
                    json: json!({ "topic": topic.to_string(), "frame": null }),
                    key: vec![0],
                };
            }
            if !latest.responding {
                lines.push(NOT_RESPONDING.to_owned());
            }
            let m = view
                .state
                .as_ref()
                .and_then(|s| s.measurements.iter().find(|m| m.id == meas).cloned());
            let Some(m) = m else {
                lines.push(format!("measurement {} is gone", meas.0));
                return View {
                    lines,
                    json: json!({ "meas": meas.0, "gone": true }),
                    key,
                };
            };
            let MeasKind::Spl { config } = &m.config.kind else {
                return View {
                    lines,
                    json: json!(null),
                    key,
                };
            };
            let Some(tf) = latest.get(&topic) else {
                lines.push(format!(
                    "{}: waiting for the first second …",
                    m.config.name
                ));
                return View {
                    lines,
                    json: json!({ "topic": topic.to_string(), "frame": null }),
                    key,
                };
            };
            let FrameData::Leq(f) = &tf.frame.data else {
                return View {
                    lines,
                    json: json!(null),
                    key,
                };
            };
            let tiles = leq_tiles(&config.leq, f);
            let offset = view.clock_offset_ns.map_or(0, |o| {
                o.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
            });
            let mic = view.state.as_ref().and_then(|s| {
                s.inputs
                    .iter()
                    .find(|i| i.channel == config.input)
                    .and_then(|i| i.mic.clone())
            });
            let cal = output::cal_status(f.meta.cal, f.meta.mic_curve, now_wall(), offset);
            let cal = match &mic {
                Some(name) => format!("{name} · {cal}"),
                None => cal,
            };
            let corrected = f.meta.position.as_ref().map(ac2_scene::leq::position_text);
            let head = format!(
                "{} · in {} · {cal}{} · age {}",
                m.config.name,
                u32::from(config.input) + 1,
                corrected
                    .as_ref()
                    .map_or_else(String::new, |c| format!(" · {c}")),
                age_text(tf.age, tf.stale)
            );
            lines.push(head.clone());
            let run = f
                .meta
                .run
                .map(|r| (r, ac2_scene::leq::run_text(&r, &config.leq, output::local_offset_s)));
            if let Some((_, rt)) = &run {
                lines.push(rt.line());
            }
            for t in &tiles {
                let state = t.state_text.clone().unwrap_or_default();
                let marker = match t.state {
                    TileState::Over => "▶ OVER",
                    TileState::Near if t.on_course => "▷ ON COURSE",
                    TileState::Near => "▷ NEAR",
                    _ => "",
                };
                let details: Vec<String> = [&t.course, &t.limit, &t.headroom, &t.recover, &t.filling, &t.incomplete, &t.held]
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect();
                if big {
                    lines.push(String::new());
                    let title = if marker.is_empty() {
                        format!("{}   {state}", t.name)
                    } else {
                        format!("{}   {marker}", t.name)
                    };
                    lines.push(title);
                    let rows = big_number(&t.value);
                    lines.push(format!("  {}", rows[0]));
                    lines.push(format!("  {}", rows[1]));
                    lines.push(format!("  {}  {}", rows[2], t.weighted_unit));
                    if !details.is_empty() {
                        lines.push(format!("  {}", details.join(" · ")));
                    }
                } else {
                    let mut l = format!("{}  {} {}", t.name, t.value, t.weighted_unit);
                    if !state.is_empty() {
                        l.push_str(&format!("  {state}"));
                    }
                    if !details.is_empty() {
                        l.push_str(&format!("  {}", details.join(" · ")));
                    }
                    lines.push(l);
                }
            }
            let windows: Vec<serde_json::Value> = config
                .leq
                .windows
                .iter()
                .zip(&tiles)
                .enumerate()
                .map(|(i, (w, t))| {
                    let num = |v: f32| f64::from(v).is_finite().then_some(f64::from(v));
                    json!({
                        "name": t.name,
                        "duration_s": w.duration.0,
                        "weighting": w.weighting,
                        "leq": num(f.leq[i]),
                        "limit": w.limit,
                        "warn_margin": w.warn_margin,
                        "judgement": f.flags[i].judgement(),
                        "elapsed_s": f.elapsed[i],
                        "measured_s": f.measured[i],
                        "filling": t.filling(),
                        "incomplete": f.flags[i].contains(ac2_proto::frame::LeqFlags::INCOMPLETE),
                        "least": num(f.least[i]),
                        "on_course": f.flags[i].contains(ac2_proto::frame::LeqFlags::ON_COURSE),
                        "over_in_s": num(f.over_in[i]),
                        "allowed": num(f.allowed[i]),
                        "allowed_until_full": t.allowed_until_full,
                        "cannot_recover": f.flags[i].contains(ac2_proto::frame::LeqFlags::CANNOT_RECOVER),
                        "recover_s": num(f.recover[i]),
                        "text": {
                            "value": t.value,
                            "state": t.state_text,
                            "course": t.course,
                            "limit": t.limit,
                            "headroom": t.headroom,
                            "recover": t.recover,
                            "filling": t.filling,
                            "incomplete": t.incomplete,
                        },
                    })
                })
                .collect();
            let peaks: Vec<serde_json::Value> = tiles
                .iter()
                .filter_map(|t| match t.kind {
                    ac2_scene::leq::TileKind::Peak(q) => Some((q, t)),
                    ac2_scene::leq::TileKind::Window => None,
                })
                .map(|(q, t)| {
                    let p = match q {
                        ac2_proto::model::PeakQuantity::LcPeak => f.meta.lcpeak,
                        ac2_proto::model::PeakQuantity::LafMax => f.meta.lafmax,
                    };
                    json!({
                        "quantity": q,
                        "name": t.name,
                        "level": p.and_then(|p| p.level.is_finite().then_some(p.level)),
                        "held_s": ac2_proto::frame::LeqPeak::HOLD_S,
                        "limit": config.leq.peaks.get(q).map(|l| l.limit),
                        "warn_margin": config.leq.peaks.get(q).map(|l| l.warn_margin),
                        "judgement": p.map(|p| p.judgement),
                        "text": {
                            "value": t.value,
                            "unit": t.weighted_unit,
                            "state": t.state_text,
                            "limit": t.limit,
                            "held": t.held,
                        },
                    })
                })
                .collect();
            View {
                lines,
                json: json!({
                    "topic": topic.to_string(),
                    "meas": meas.0,
                    "name": m.config.name,
                    "seq": tf.frame.stamp.seq,
                    "age_s": tf.age,
                    "stale": tf.stale,
                    "responding": latest.responding,
                    "scale": f.meta.scale,
                    "cal": f.meta.cal,
                    "cal_text": cal,
                    "position": f.meta.position,
                    "position_text": corrected,
                    "peaks": peaks,
                    "horizon_s": f.meta.horizon.0,
                    "logged": f.meta.logged,
                    "run": run.as_ref().map(|(r, rt)| {
                        let num = |v: f64| v.is_finite().then_some(v);
                        json!({
                            "started_at": ac2_traces::spl_log::utc_iso(r.started_at.0),
                            "started_at_ns": r.started_at.0,
                            "until_ns": r.until.0,
                            "running_s": r.until.0.saturating_sub(r.started_at.0) as f64 / 1e9,
                            "measured_s": r.measured.0,
                            "gaps_s": r.gaps.0,
                            "trimmed": r.trimmed,
                            "laeq": num(r.laeq),
                            "lceq": num(r.lceq),
                            "lzeq": num(r.lzeq),
                            "text": {
                                "line": rt.line(),
                                "clock": rt.clock,
                                "since": rt.since,
                                "gaps": rt.gaps,
                            },
                        })
                    }),
                    "windows": windows,
                }),
                key,
            }
        },
    )
    .await
}

/// `spl bands watch`: the headline, the period, the prediction and one line per band; piped,
/// the same each second; `--json`, one line per second.
pub async fn bands(
    c: &Client,
    meas: MeasId,
    until: Option<Instant>,
    out: &mut Out<'_>,
) -> Result<(), CliError> {
    use ac2_scene::band_leq::band_leq_text;
    let topic = Topic::Data {
        meas,
        stream: Stream::BandLeq,
    };
    live_until(
        c,
        out,
        &[Subscription::Topic(topic)],
        until,
        |view, latest| {
            let mut lines = Vec::new();
            let key = latest
                .get(&topic)
                .map_or(vec![0], |t| vec![t.frame.stamp.seq]);
            if !latest.responding {
                lines.push(NOT_RESPONDING.to_owned());
            }
            let m = view
                .state
                .as_ref()
                .and_then(|s| s.measurements.iter().find(|m| m.id == meas).cloned());
            let Some(m) = m else {
                lines.push(format!("measurement {} is gone", meas.0));
                return View {
                    lines,
                    json: json!({ "meas": meas.0, "gone": true }),
                    key,
                };
            };
            let MeasKind::Spl { config } = &m.config.kind else {
                return View {
                    lines,
                    json: json!(null),
                    key,
                };
            };
            let Some(tf) = latest.get(&topic) else {
                lines.push(format!("{}: waiting for the first second …", m.config.name));
                return View {
                    lines,
                    json: json!({ "topic": topic.to_string(), "frame": null }),
                    key,
                };
            };
            let FrameData::BandLeq(f) = &tf.frame.data else {
                return View {
                    lines,
                    json: json!(null),
                    key,
                };
            };
            let f = &f.meta;
            let t = band_leq_text(f);
            let offset = view.clock_offset_ns.map_or(0, |o| {
                o.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
            });
            let mic = view.state.as_ref().and_then(|s| {
                s.inputs
                    .iter()
                    .find(|i| i.channel == config.input)
                    .and_then(|i| i.mic.clone())
            });
            let cal = output::cal_status(f.cal, f.mic_curve, now_wall(), offset);
            let cal = match &mic {
                Some(name) => format!("{name} · {cal}"),
                None => cal,
            };
            lines.push(format!(
                "{} · {}, {} · {cal} · age {}",
                m.config.name,
                t.name,
                t.unit,
                age_text(tf.age, tf.stale)
            ));
            lines.push(t.headline.clone());
            let mut info = vec![t.period.clone(), t.limits_from.clone()];
            info.extend(t.correction.iter().cloned());
            info.extend(t.filling.iter().cloned());
            info.extend(t.incomplete.iter().cloned());
            lines.push(info.join(" · "));
            if let Some(p) = &t.predicted {
                lines.push(p.line.clone());
            }
            for (i, b) in t.bars.iter().enumerate() {
                let mark = if t.worst == Some(i) { "▶" } else { " " };
                let mut l = format!("{mark} {:>5} Hz  {:>6} {}", b.label, b.value, t.unit);
                if let Some(s) = &b.state_text {
                    l.push_str(&format!("  {s}"));
                }
                let details: Vec<String> = [&b.limit, &b.headroom, &b.recover]
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect();
                if !details.is_empty() {
                    l.push_str(&format!("  {}", details.join(" · ")));
                }
                lines.push(l);
            }
            let num = |v: f64| v.is_finite().then_some(v);
            let bands: Vec<serde_json::Value> = f
                .bands
                .iter()
                .zip(&t.bars)
                .map(|(b, x)| {
                    json!({
                        "nominal_hz": b.nominal.0,
                        "leq": num(b.leq),
                        "limit": b.limit,
                        "judgement": b.judgement,
                        "on_course": b.on_course,
                        "allowed": b.allowed,
                        "recover_s": b.recover.map(|r| r.0),
                        "text": {
                            "name": x.name,
                            "label": x.label,
                            "value": x.value,
                            "state": x.state_text,
                            "limit": x.limit,
                            "headroom": x.headroom,
                            "recover": x.recover,
                        },
                    })
                })
                .collect();
            View {
                lines,
                json: json!({
                    "topic": topic.to_string(),
                    "meas": meas.0,
                    "name": m.config.name,
                    "seq": tf.frame.stamp.seq,
                    "age_s": tf.age,
                    "stale": tf.stale,
                    "responding": latest.responding,
                    "scale": f.scale,
                    "cal": f.cal,
                    "cal_text": cal,
                    "duration_s": f.duration.0,
                    "horizon_s": f.horizon.0,
                    "elapsed_s": f.elapsed.0,
                    "measured_s": f.measured.0,
                    "period": f.period,
                    "period_after_horizon": f.period_after_horizon,
                    "correction_db": f.correction.0,
                    "limits_from": f.limits_from,
                    "worst": f.worst,
                    "worst_hz": t.worst.map(|i| t.bars[i].nominal_hz),
                    "predicted": f.predicted.map(|p| json!({
                        "estimate": num(p.estimate),
                        "at_most": num(p.at_most),
                        "limit": p.limit,
                        "judgement": p.judgement,
                    })),
                    "bands": bands,
                    "text": {
                        "name": t.name,
                        "headline": t.headline,
                        "period": t.period,
                        "limits_from": t.limits_from,
                        "correction": t.correction,
                        "filling": t.filling,
                        "incomplete": t.incomplete,
                        "predicted": t.predicted.as_ref().map(|p| p.line.clone()),
                    },
                }),
                key,
            }
        },
    )
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
            // A sweep measurement publishes nothing: its results are its runs.
            let Some(stream) = m.config.kind.stream() else {
                t.add_row(vec![
                    m.id.to_string(),
                    m.config.name.clone(),
                    output::meas_kind(&m.config.kind),
                    output::yes(m.running),
                    format::NO_VALUE.to_owned(),
                    format::NO_VALUE.to_owned(),
                ]);
                rows.push(json!({ "id": m.id, "name": m.config.name, "running": m.running }));
                continue;
            };
            let topic = Topic::Data { meas: m.id, stream };
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

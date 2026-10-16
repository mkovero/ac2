//! `spl leq watch`: Leq windows as big numbers.

use super::*;

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
                            "started_at": ac2_proto::spl_log::utc_iso(r.started_at.0),
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

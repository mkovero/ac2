//! `spl bands watch`: band windows, limits and the prediction.

use super::*;

/// `spl bands watch`: the headline, where the limits come from, the prediction, then per
/// window its caption and one line per shown band; piped, the same each second; `--json`,
/// one line per second with the windows in an array.
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
            let Some(cfg) = config.bands.as_deref() else {
                lines.push(format!("{}: the band meter is off", m.config.name));
                return View {
                    lines,
                    json: json!({ "meas": meas.0, "bands": null }),
                    key,
                };
            };
            let t = band_leq_text(cfg, f, ac2_scene::view::PlotChrome::Full);
            let meta = &f.meta;
            let offset = view.clock_offset_ns.map_or(0, |o| {
                o.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
            });
            let mic = view.state.as_ref().and_then(|s| {
                s.inputs
                    .iter()
                    .find(|i| i.channel == config.input)
                    .and_then(|i| i.mic.clone())
            });
            let cal = output::cal_status(meta.cal, meta.mic_curve, now_wall(), offset);
            let cal = match &mic {
                Some(name) => format!("{name} · {cal}"),
                None => cal,
            };
            lines.push(format!(
                "{} · band Leq {}, {} · {cal} · age {}",
                m.config.name,
                t.bands,
                t.unit,
                age_text(tf.age, tf.stale)
            ));
            lines.push(t.headline.clone());
            let info: Vec<String> = t.limits_from.iter().chain(&t.correction).cloned().collect();
            if !info.is_empty() {
                lines.push(info.join(" · "));
            }
            if let Some(p) = &t.predicted {
                lines.push(p.line.clone());
            }
            let num = |v: f32| v.is_finite().then_some(f64::from(v));
            // The windows of one length and weighting on one row, low band to high.
            let mut rows = Vec::new();
            let mut row_of = vec![0usize; meta.windows.len()];
            for (r, rt) in t.rows.iter().enumerate() {
                lines.push(rt.caption());
                for (i, b) in rt.bars.iter().enumerate() {
                    row_of[b.window] = r;
                    let mark = if t.worst == Some((r, i)) {
                        "▶"
                    } else if rt.worst == Some(i) {
                        "▷"
                    } else {
                        " "
                    };
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
                rows.push(json!({
                    "windows": rt.bars.iter().map(|b| b.window).collect::<Vec<_>>(),
                    "worst_hz": rt.worst.map(|i| rt.bars[i].nominal_hz),
                    "text": {
                        "name": rt.name,
                        "caption": rt.caption(),
                        "period": rt.period,
                        "filling": rt.filling,
                        "incomplete": rt.incomplete,
                    },
                }));
            }
            // Per window, in the configuration's order.
            let bar_of = |k: usize| t.rows.iter().flat_map(|r| &r.bars).find(|b| b.window == k);
            let windows: Vec<serde_json::Value> = meta
                .windows
                .iter()
                .enumerate()
                .map(|(k, st)| {
                    let flags = f.flags[k];
                    let x = bar_of(k);
                    json!({
                        "band_hz": st.band.0,
                        "duration_s": st.duration.0,
                        "weighting": st.weighting,
                        "row": row_of[k],
                        "elapsed_s": st.elapsed.0,
                        "measured_s": st.measured.0,
                        "period": st.period,
                        "period_after_horizon": st.period_after_horizon,
                        "leq": num(f.leq[k]),
                        "limit": num(f.limit[k]),
                        "judgement": flags.judgement(),
                        "on_course": flags.contains(ac2_proto::frame::LeqFlags::ON_COURSE),
                        "allowed": num(f.allowed[k]),
                        "recover_s": num(f.recover[k]),
                        "text": x.map(|x| json!({
                            "name": x.name,
                            "label": x.label,
                            "value": x.value,
                            "state": x.state_text,
                            "limit": x.limit,
                            "headroom": x.headroom,
                            "recover": x.recover,
                        })),
                    })
                })
                .collect();
            // Every window's band, low to high, each once.
            let mut bands_hz: Vec<f64> = meta.windows.iter().map(|w| w.band.0).collect();
            bands_hz.sort_by(f64::total_cmp);
            bands_hz.dedup();
            let worst = t.worst.map(|(r, i)| {
                let b = &t.rows[r].bars[i];
                json!({
                    "window": b.window,
                    "row": r,
                    "band_hz": b.nominal_hz,
                    "name": b.name,
                })
            });
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
                    "scale": meta.scale,
                    "cal": meta.cal,
                    "cal_text": cal,
                    "horizon_s": meta.horizon.0,
                    "correction_db": meta.correction.0,
                    "limits_from": meta.limits_from,
                    "bands_hz": bands_hz,
                    "worst": worst,
                    "predicted": meta.predicted.as_ref().map(|p| json!({
                        "duration_s": p.duration.0,
                        "estimate": p.estimate.is_finite().then_some(p.estimate),
                        "at_most": p.at_most.is_finite().then_some(p.at_most),
                        "limit": p.limit,
                        "judgement": p.judgement,
                    })),
                    "windows": windows,
                    "rows": rows,
                    "text": {
                        "bands": t.bands,
                        "headline": t.headline,
                        "limits_from": t.limits_from,
                        "correction": t.correction,
                        "predicted": t.predicted.as_ref().map(|p| p.line.clone()),
                    },
                }),
                key,
            }
        },
    )
    .await
}

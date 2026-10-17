//! Key commands for the delay, polarity, offsets, averages, legend, phase, spectrum and SPL
//! views.

use super::*;

impl AppState {
    /// The delay, offset, average, legend, spectrum and SPL [`CommandId`]s [`AppState::command`]
    /// routes here.
    pub(super) fn command_tf(&mut self, c: CommandId, out: &mut Vec<Request>) {
        use CommandId as C;
        use ac2_scene::legend::LegendCorner;
        match c {
            C::InsertDelay | C::InsertStrongest => {
                if let Some(m) = self.need_tf() {
                    let pick = if c == C::InsertDelay {
                        DelayPick::FirstArrival
                    } else {
                        DelayPick::Strongest
                    };
                    if matches!(self.overlay, Overlay::DelayPick(_)) {
                        self.overlay = Overlay::None;
                    }
                    out.push(Request::FindDelay {
                        meas: m.id,
                        pick,
                        band: self.finder.band,
                        observation: self.finder.observation,
                    });
                }
            }
            C::TypeDelay => {
                if let Some(m) = self.need_tf() {
                    let text = m
                        .delay
                        .as_ref()
                        .map(|d| format::delay_entry_ms(d.applied.0))
                        .unwrap_or_default();
                    self.prompt(PromptKind::Delay(m.id), text);
                }
            }
            C::DelayDown | C::DelayUp | C::DelayDownFine | C::DelayUpFine => {
                let step = match c {
                    C::DelayDown => -1.0,
                    C::DelayUp => 1.0,
                    C::DelayDownFine => -DELAY_FINE_STEP,
                    _ => DELAY_FINE_STEP,
                };
                self.step_delay(format::DelayStep::Samples(step), out);
            }
            C::TrackDelay => {
                if let Some(m) = self.need_tf() {
                    let enabled = !m.delay.as_ref().is_some_and(|d| d.tracking);
                    let what = if enabled { "on" } else { "off" };
                    self.call(
                        out,
                        Command::DelayTrack {
                            meas: m.id,
                            enabled,
                        },
                        format!("{}: tracking {what}", m.config.name),
                    );
                }
            }
            C::Invert => match self.transfer_trace_for_edit() {
                Err(()) => {}
                Ok(Some(t)) if t.kind == TraceKind::Target => {
                    self.warn(format!("{}: a target curve has no phase", trace_label(&t)));
                }
                Ok(Some(t)) => {
                    let mut edit = t.edit.clone();
                    edit.polarity = match edit.polarity {
                        Polarity::Normal => Polarity::Inverted,
                        Polarity::Inverted => Polarity::Normal,
                    };
                    let what = format!(
                        "{}: polarity {}",
                        trace_label(&t),
                        match edit.polarity {
                            Polarity::Normal => "normal",
                            Polarity::Inverted => "inverted",
                        }
                    );
                    self.call(out, Command::TraceUpdate { trace: t.id, edit }, what);
                }
                Ok(None) => {
                    if let Some(m) = self.need_tf() {
                        let e = self.edits.entry(m.id).or_default();
                        e.inverted = !e.inverted;
                    }
                }
            },
            C::Offset => self.offset_prompt(),
            C::OffsetUp => self.step_offset(Some(1.0), out),
            C::OffsetDown => self.step_offset(Some(-1.0), out),
            C::OffsetUpCoarse => self.step_offset(Some(3.0), out),
            C::OffsetDownCoarse => self.step_offset(Some(-3.0), out),
            C::OffsetClear => self.step_offset(None, out),
            C::NudgeEarlier | C::NudgeLater => {
                let d = if c == C::NudgeEarlier {
                    -NUDGE_S
                } else {
                    NUDGE_S
                };
                // A trace's nudge in whole steps: repeated steps never accumulate float
                // error.
                let step = |v: f64| ((v + d) / NUDGE_S).round() * NUDGE_S;
                match self.transfer_trace_for_edit() {
                    Err(()) => {}
                    Ok(Some(t)) if t.kind == TraceKind::Target => {
                        self.warn(format!("{}: a target curve has no phase", trace_label(&t)));
                    }
                    Ok(Some(t)) => {
                        let mut edit = t.edit.clone();
                        edit.delay_nudge = Seconds(step(edit.delay_nudge.0));
                        let what =
                            format::trace_delay_toast(&trace_label(&t), d, edit.delay_nudge.0);
                        self.call(out, Command::TraceUpdate { trace: t.id, edit }, what);
                    }
                    // A measurement has one delay, the daemon's: the plain keys step it too.
                    Ok(None) => self.step_delay(format::DelayStep::Seconds(d), out),
                }
            }
            C::PhaseReference => match self.transfer_trace_for_edit() {
                Err(()) => {}
                Ok(Some(t)) if t.kind == TraceKind::Target => {
                    self.warn(format!("{}: a target curve has no phase", trace_label(&t)));
                }
                Ok(Some(t)) => {
                    self.view.tf.phase_reference = Some(TraceKey::Stored(t.id));
                    self.toast(format!("phase reference: {}", trace_label(&t)));
                }
                Ok(None) => {
                    if let Some(m) = self.need_tf() {
                        self.view.tf.phase_reference = Some(TraceKey::Live(m.id));
                        self.toast(format!("phase reference: {}", m.config.name));
                    }
                }
            },
            C::Target => self.prompt(PromptKind::ImportFile(ImportRole::Target), String::new()),
            C::Average => self.average(AverageMethod::Power, out),
            C::AverageComplex => self.average(AverageMethod::Complex, out),
            C::AverageCoherence => self.average(AverageMethod::CoherenceWeighted, out),
            C::CoherenceMask => {
                let cur = self.view.tf.coherence.blank_below;
                let i = COHERENCE_MASKS.iter().position(|m| *m == cur).unwrap_or(0);
                let next = COHERENCE_MASKS[(i + 1) % COHERENCE_MASKS.len()];
                self.view.tf.coherence.blank_below = next;
                self.toast(match next {
                    None => "coherence mask off".to_string(),
                    Some(t) => format!("coherence mask: hide γ² < {t:.1}"),
                });
            }
            C::CoherencePlacement => self.toggle_coherence_placement(),
            C::LegendTopLeft => self.view.tf.legend.snap(LegendCorner::TopLeft),
            C::LegendTopRight => self.view.tf.legend.snap(LegendCorner::TopRight),
            C::LegendBottomLeft => self.view.tf.legend.snap(LegendCorner::BottomLeft),
            C::LegendBottomRight => self.view.tf.legend.snap(LegendCorner::BottomRight),
            C::LegendHide => {
                let hidden = !self.view.tf.legend.hidden;
                self.view.tf.legend.hidden = hidden;
                self.toast(if hidden {
                    "legend hidden (Show/hide legend brings it back)"
                } else {
                    "legend shown"
                });
            }
            C::PhaseView => {
                let next = self.view.tf.phase.next();
                self.view.tf.phase = next;
                self.toast(next.name());
            }
            C::MinMax => {
                let on = !self.view.tf.extremes;
                self.view.tf.extremes = on;
                self.toast(if on {
                    "min / max tracking"
                } else {
                    "min / max tracking off"
                });
            }
            C::SpectrumStyle => self.step_spectrum_style(out),
            C::PeakHold => self.step_peak_mode(out),
            C::SpectrographSpan => self.step_spectrograph_span(out),
            C::SplShowMeter | C::SplShowLeq | C::SplShowMeterLeq | C::SplShowBands => {
                self.focus_kind(PaneKind::Spl);
                let m = self.modes_mut();
                m.spl = match c {
                    C::SplShowMeter => SplMode::Meter,
                    C::SplShowLeq => SplMode::Leq,
                    C::SplShowMeterLeq => SplMode::MeterLeq,
                    _ => SplMode::Bands,
                };
            }
            // Either shows the windows (a layout change is about them) and is remembered.
            C::SplLeqStyle | C::SplLeqHistory => {
                self.toggle_leq_layout(c == C::SplLeqHistory);
                self.focus_kind(PaneKind::Spl);
                let m = self.modes_mut();
                m.spl = m.spl.with_leq();
            }
            C::SplLogExport => self.ask_leq_export(false),
            C::SplLogExportPrevious => self.ask_leq_export(true),
            C::SplNewLog => match self.kind_meas(PaneKind::Spl).cloned() {
                Some(m) => {
                    let MeasKind::Spl { config } = &m.config.kind else {
                        return;
                    };
                    let zone = self.local_zone;
                    let run = self
                        .leq_run(m.id)
                        .map(|r| ac2_scene::leq::run_text(&r, &config.leq, |t| zone.offset_s(t)));
                    self.overlay = Overlay::NewLog(Box::new(NewLogPrompt {
                        meas: m.id,
                        meter: m.config.name.clone(),
                        confirm: ac2_scene::leq::new_log_confirm(
                            &m.config.name,
                            &config.leq,
                            run.as_ref(),
                        ),
                    }));
                }
                None => self.warn(self.no_spl_meter()),
            },
            C::SplTimeWeighting
            | C::SplWeighting
            | C::SplFast
            | C::SplSlow
            | C::SplImpulse
            | C::SplA
            | C::SplC
            | C::SplZ => self.spl_weightings(c, out),
            C::LeqWindows => match self.kind_meas(PaneKind::Spl) {
                Some(_) => self.open_settings(Page::Leq, out),
                None => self.warn(self.no_spl_meter()),
            },
            _ => unreachable!("{c:?} is not a transfer/view command: routed by command"),
        }
    }
}

//! The band transfer step over the Leq dialog: opening it on the meters with a band meter,
//! its keys, the spans' times on the meter's clock, and the daemon's answers.

use eframe::egui::Key;

use super::*;
use crate::leq_dialog::{Ask, TransferStep};

impl AppState {
    /// The wall time of SPL meter `meas`'s newest frame: the meter's clock, which its band
    /// log is stamped on (the daemon may run on another computer).
    pub(crate) fn meter_now(&self, meas: MeasId) -> Option<ac2_proto::units::WallNs> {
        let d = self.data.as_ref()?;
        [Stream::Spl, Stream::Leq, Stream::BandLeq]
            .into_iter()
            .filter_map(|stream| d.latest.get(&Topic::Data { meas, stream }))
            .map(|f| f.frame.stamp.capture_wall_ns)
            .max()
    }

    /// T on a band row of the Leq dialog: the band transfer step for the dialog's meter,
    /// once the daemon's meter has a band meter (it reads the band log).
    pub(super) fn open_band_transfer(&mut self) {
        let band_meters: Vec<(MeasId, String)> = self
            .measurements()
            .iter()
            .filter(
                |m| matches!(&m.config.kind, MeasKind::Spl { config } if config.bands.is_some()),
            )
            .map(|m| (m.id, m.config.name.clone()))
            .collect();
        let Some(d) = self.overlay.leq_mut() else {
            return;
        };
        let Some(own) = band_meters.iter().find(|(id, _)| *id == d.meas).cloned() else {
            d.error = Some(
                "the band transfer reads the band log: turn the band meter on and apply it \
                 (Enter) first"
                    .into(),
            );
            return;
        };
        let mut meters = vec![own];
        meters.extend(band_meters.into_iter().filter(|(id, _)| *id != d.meas));
        d.error = None;
        d.transfer = TransferStep::new(meters, d.bands.place(), d.bands.shown());
    }

    /// A key on the band transfer step.
    pub(super) fn band_transfer_key(&mut self, chord: Chord, out: &mut Vec<Request>) {
        let minutes = match chord.key {
            Key::Num1 => Some(1),
            Key::Num2 => Some(2),
            Key::Num3 => Some(3),
            Key::Num4 => Some(4),
            Key::Num5 => Some(5),
            Key::Num6 => Some(6),
            Key::Num7 => Some(7),
            Key::Num8 => Some(8),
            Key::Num9 => Some(9),
            _ => None,
        };
        let focused = self
            .overlay
            .leq()
            .and_then(|d| d.transfer.as_ref())
            .map(|t| t.span(t.focus).clone());
        let Some(span) = focused else {
            return;
        };
        let now = self.meter_now(span.meas);
        let Some(t) = self.overlay.leq_mut().and_then(|d| d.transfer.as_mut()) else {
            return;
        };
        let no_clock = || {
            format!(
                "no readings from {} yet: start it (a span is timed on the meter's clock)",
                span.meter
            )
        };
        // On the place's name the keys that type are its text (`settings_text`).
        let typing = t.on_place
            && (minutes.is_some()
                || matches!(
                    chord.key,
                    Key::Space | Key::Delete | Key::ArrowLeft | Key::ArrowRight
                ));
        let asked = match (chord.key, minutes) {
            _ if typing => None,
            (Key::ArrowUp, _) => {
                t.move_focus(-1);
                None
            }
            (Key::ArrowDown | Key::Tab, _) => {
                t.move_focus(1);
                None
            }
            (Key::ArrowLeft, _) => {
                t.cycle_meter(-1);
                None
            }
            (Key::ArrowRight, _) => {
                t.cycle_meter(1);
                None
            }
            (Key::Delete, _) => {
                t.clear();
                None
            }
            (Key::Space, _) => match now.ok_or_else(no_clock).and_then(|now| t.toggle(now)) {
                Ok(a) => a,
                Err(e) => {
                    t.error = Some(e);
                    None
                }
            },
            (_, Some(n)) => match now
                .ok_or_else(no_clock)
                .and_then(|now| t.last_minutes(n, now))
            {
                Ok(a) => Some(a),
                Err(e) => {
                    t.error = Some(e);
                    None
                }
            },
            (Key::Enter, _) => {
                match t.sources() {
                    Ok(sources) => {
                        t.error = None;
                        t.storing = true;
                        out.push(Request::BandTransfer {
                            meas: t.meas,
                            sources: Box::new(sources),
                            place: t.place_name().to_owned(),
                        });
                    }
                    Err(e) => t.error = Some(e),
                }
                None
            }
            _ => None,
        };
        if let Some(ask) = asked {
            out.push(Request::BandLogGet(ask));
        }
    }

    /// A span read back.
    pub(super) fn band_log_answered(
        &mut self,
        ask: Ask,
        result: Result<Box<ac2_proto::model::SplBandLog>, String>,
    ) {
        if let Some(t) = self.overlay.leq_mut().and_then(|d| d.transfer.as_mut()) {
            t.answered(ask, result.map(|l| *l));
        }
    }

    /// `spl.band_transfer` answered: the step shows what was stored, and the dialog keeps
    /// it (its Enter would otherwise put back the transfer it opened with).
    pub(super) fn band_transfer_answered(&mut self, result: Result<Box<Measurement>, String>) {
        match result {
            Ok(m) => {
                let set = match &m.config.kind {
                    MeasKind::Spl { config } => {
                        config.bands.as_ref().and_then(|b| b.transfer.clone())
                    }
                    _ => None,
                };
                let shown = match &m.config.kind {
                    MeasKind::Spl { config } => config
                        .bands
                        .as_ref()
                        .and_then(|b| b.band_indices())
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                if let Some(d) = self.overlay.leq_mut()
                    && d.meas == m.id
                {
                    d.bands.set_transfer(set.clone());
                    if let Some(t) = &mut d.transfer {
                        t.storing = false;
                        t.stored = set.clone();
                    }
                }
                self.toast(format!(
                    "{}: band transfer stored · {}",
                    m.config.name,
                    ac2_scene::band_leq::transfer_summary(set.as_ref(), &shown)
                ));
            }
            Err(e) => {
                if let Some(t) = self.overlay.leq_mut().and_then(|d| d.transfer.as_mut()) {
                    t.storing = false;
                    t.error = Some(e.clone());
                }
                self.fault(format!("band transfer: {e}"));
            }
        }
    }
}

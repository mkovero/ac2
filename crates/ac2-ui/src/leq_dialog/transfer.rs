//! The band transfer step of the Leq dialog (`docs/design/band-leq.md`, *The transfer*):
//! three spans of band logs named while the meters run — the test signal at FOH, the same
//! signal with the mic in the bedroom, the bedroom with the system silent — each by a start
//! and a stop key or as the last minutes, read back with `spl.band_log_get`, then
//! `spl.band_transfer` from them into the dialog's meter. Times are the meter's clock (its
//! newest frame), so a span lines up with its log on a daemon elsewhere. Pure data: the
//! reducer gives it the times and the answers.

use ac2_proto::model::{
    BandLevelSource, BandLogAverage, BandTransferSet, LF_BAND_COUNT, SplBandLog,
};
use ac2_proto::units::{MeasId, WallNs};
use ac2_scene::band_transfer::{Phase, SpanRole, SpanState};

const NS: u64 = 1_000_000_000;

/// One span being named.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    /// The SPL meter whose band log it reads.
    pub meas: MeasId,
    pub meter: String,
    pub state: SpanState,
    /// Its average, once read back.
    pub average: Option<BandLogAverage>,
    /// Why it could not be read back.
    pub error: Option<String>,
}

impl Span {
    fn new((meas, meter): &(MeasId, String)) -> Self {
        Self {
            meas: *meas,
            meter: meter.clone(),
            state: SpanState::Unmarked,
            average: None,
            error: None,
        }
    }

    fn source(&self) -> Option<BandLevelSource> {
        match self.state {
            SpanState::Marked { from, until } => Some(BandLevelSource::Log {
                meas: self.meas,
                from,
                until,
            }),
            _ => None,
        }
    }
}

/// A span marked: its log to read back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ask {
    pub meas: MeasId,
    pub from: WallNs,
    pub until: WallNs,
}

/// The transfer to compute: FOH, bedroom (`dwelling`), background.
pub type Sources = (BandLevelSource, BandLevelSource, Option<BandLevelSource>);

/// The open step.
#[derive(Clone, Debug, PartialEq)]
pub struct TransferStep {
    /// The meter that takes the transfer (the dialog's).
    pub meas: MeasId,
    pub meter: String,
    /// SPL meters with a band meter, the dialog's first: what a span can read.
    pub meters: Vec<(MeasId, String)>,
    pub focus: SpanRole,
    /// In [`SpanRole::ALL`] order.
    pub spans: [Span; 3],
    /// Why the last key did nothing.
    pub error: Option<String>,
    /// `spl.band_transfer` sent, not answered.
    pub storing: bool,
    /// The transfer this step stored.
    pub stored: Option<BandTransferSet>,
}

impl TransferStep {
    /// `meters` lists the SPL meters with a band meter; the dialog's meter is first and
    /// every span starts on it (one mic moved).
    pub fn new(meters: Vec<(MeasId, String)>) -> Option<Self> {
        let first = meters.first()?.clone();
        let span = Span::new(&first);
        Some(Self {
            meas: first.0,
            meter: first.1,
            meters,
            focus: SpanRole::Foh,
            spans: [span.clone(), span.clone(), span],
            error: None,
            storing: false,
            stored: None,
        })
    }

    pub fn span(&self, r: SpanRole) -> &Span {
        &self.spans[r.index()]
    }

    fn marking(&self) -> Option<SpanRole> {
        SpanRole::ALL
            .into_iter()
            .find(|r| matches!(self.span(*r).state, SpanState::Marking { .. }))
    }

    /// ↑/↓.
    pub fn move_focus(&mut self, d: i32) {
        let i = (self.focus.index() as i32 + d).clamp(0, 2) as usize;
        self.focus = SpanRole::ALL[i];
        self.error = None;
    }

    /// ←/→: the meter whose log the focused span reads (another mic, another rig). The FOH
    /// span is always the dialog's meter: the transfer is from its mic.
    pub fn cycle_meter(&mut self, d: i32) {
        self.error = None;
        if self.focus == SpanRole::Foh {
            if self.meters.len() > 1 {
                self.error = Some(format!(
                    "the FOH span is {}'s, the meter that takes the transfer",
                    self.meter
                ));
            }
            return;
        }
        let n = self.meters.len() as i32;
        let s = &mut self.spans[self.focus.index()];
        let i = self
            .meters
            .iter()
            .position(|(m, _)| *m == s.meas)
            .unwrap_or(0) as i32;
        let next = (i + d).clamp(0, n - 1) as usize;
        if next as i32 != i {
            *s = Span::new(&self.meters[next]);
        }
    }

    fn marked(&mut self, from: WallNs, until: WallNs) -> Ask {
        let s = &mut self.spans[self.focus.index()];
        s.state = SpanState::Marked { from, until };
        s.average = None;
        s.error = None;
        let ask = Ask {
            meas: s.meas,
            from,
            until,
        };
        // On to the next span still to measure.
        if let Some(r) = SpanRole::ALL
            .into_iter()
            .find(|r| self.span(*r).state == SpanState::Unmarked)
        {
            self.focus = r;
        }
        self.stored = None;
        ask
    }

    /// Space at `now` (the meter's clock): starts the focused span, or stops it — then its
    /// log is to be read back.
    pub fn toggle(&mut self, now: WallNs) -> Result<Option<Ask>, String> {
        self.error = None;
        match self.span(self.focus).state {
            SpanState::Marking { from } => {
                if now.0 < from.0 + NS {
                    return Err("a span takes at least a second: Space again a little later".into());
                }
                Ok(Some(self.marked(from, now)))
            }
            SpanState::Unmarked | SpanState::Marked { .. } => {
                if let Some(r) = self.marking() {
                    return Err(format!(
                        "the {} is still running: stop it first",
                        r.title().to_lowercase()
                    ));
                }
                let s = &mut self.spans[self.focus.index()];
                s.state = SpanState::Marking { from: now };
                s.average = None;
                s.error = None;
                self.stored = None;
                Ok(None)
            }
        }
    }

    /// 1–9: the focused span is the last `minutes` before `now`.
    pub fn last_minutes(&mut self, minutes: u32, now: WallNs) -> Result<Ask, String> {
        self.error = None;
        if let Some(r) = self.marking()
            && r != self.focus
        {
            return Err(format!(
                "the {} is still running: stop it first",
                r.title().to_lowercase()
            ));
        }
        let from = WallNs(now.0.saturating_sub(u64::from(minutes) * 60 * NS));
        Ok(self.marked(from, now))
    }

    /// Delete: the focused span is unmarked.
    pub fn clear(&mut self) {
        let s = &mut self.spans[self.focus.index()];
        *s = Span {
            state: SpanState::Unmarked,
            average: None,
            error: None,
            ..s.clone()
        };
        self.error = None;
        self.stored = None;
    }

    /// The answer to reading `ask` back: kept by the span it is still for.
    pub fn answered(&mut self, ask: Ask, result: Result<SplBandLog, String>) {
        for s in &mut self.spans {
            if s.meas == ask.meas
                && s.state
                    == (SpanState::Marked {
                        from: ask.from,
                        until: ask.until,
                    })
            {
                match &result {
                    Ok(l) => {
                        s.average = Some(l.average);
                        s.error = None;
                    }
                    Err(e) => s.error = Some(e.clone()),
                }
            }
        }
    }

    pub fn phases(&self) -> [Phase; 3] {
        SpanRole::ALL.map(|r| self.span(r).state.into())
    }

    /// Enter: the sources of the transfer, or what is missing.
    pub fn sources(&self) -> Result<Sources, String> {
        if let Some(r) = self.marking() {
            return Err(format!(
                "the {} is still running: Space stops it",
                r.title().to_lowercase()
            ));
        }
        let need = |r: SpanRole| {
            self.span(r).source().ok_or_else(|| {
                format!("mark the {} first ({})", r.title().to_lowercase(), r.what())
            })
        };
        let (foh, bedroom) = (need(SpanRole::Foh)?, need(SpanRole::Bedroom)?);
        let background = self.span(SpanRole::Background).source();
        let name = |id: MeasId| {
            self.meters
                .iter()
                .find(|(m, _)| *m == id)
                .map_or_else(|| format!("SPL meter {id}"), |(_, n)| n.clone())
        };
        if let Some(e) =
            ac2_proto::model::overlapping_spans(&foh, &bedroom, background.as_ref(), name)
        {
            return Err(e);
        }
        Ok((foh, bedroom, background))
    }

    /// What to do next.
    pub fn next_step(&self) -> String {
        ac2_scene::band_transfer::next_step(self.phases(), self.stored.is_some(), &self.meter)
    }

    /// The stored transfer: a summary, then the limited bands with their status and
    /// attenuation.
    pub fn result_lines(&self) -> Vec<String> {
        let Some(t) = &self.stored else {
            return Vec::new();
        };
        let mut out = vec![ac2_scene::band_leq::transfer_summary(Some(t))];
        out.extend(
            t.bands[..LF_BAND_COUNT]
                .iter()
                .zip(ac2_proto::model::BAND_NOMINAL_HZ)
                .map(|(b, hz)| ac2_scene::band_leq::transfer_band_text(hz, b)),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::units::Seconds;

    fn t(s: u64) -> WallNs {
        WallNs(1_790_000_000 * NS + s * NS)
    }

    fn step() -> TransferStep {
        TransferStep::new(vec![
            (MeasId(1), "FOH SPL".into()),
            (MeasId(2), "Bedroom SPL".into()),
        ])
        .expect("a meter")
    }

    #[test]
    fn spans_mark_in_order_and_name_their_sources() {
        let mut s = step();
        assert!(s.next_step().starts_with("Play a steady test signal"));
        assert_eq!(s.toggle(t(0)), Ok(None));
        assert!(s.next_step().starts_with("Marking the FOH span"));
        assert!(s.toggle(t(0)).is_err(), "less than a second");
        let a = s.toggle(t(60)).expect("stops").expect("an ask");
        assert_eq!(
            a,
            Ask {
                meas: MeasId(1),
                from: t(0),
                until: t(60)
            }
        );
        assert_eq!(s.focus, SpanRole::Bedroom, "on to the bedroom");
        assert!(s.next_step().starts_with("Move the mic to the bedroom"));
        // The bedroom from the last two minutes; Enter still wants the bedroom first.
        assert!(s.sources().is_err());
        let b = s.last_minutes(2, t(300)).expect("marked");
        assert_eq!((b.from, b.until), (t(180), t(300)));
        assert_eq!(s.focus, SpanRole::Background);
        let (foh, dwelling, background) = s.sources().expect("sources");
        assert_eq!(
            foh,
            BandLevelSource::Log {
                meas: MeasId(1),
                from: t(0),
                until: t(60)
            }
        );
        assert!(matches!(dwelling, BandLevelSource::Log { from, .. } if from == t(180)));
        assert_eq!(background, None);
        // The background on the second meter.
        s.cycle_meter(1);
        assert_eq!(s.span(SpanRole::Background).meter, "Bedroom SPL");
        s.toggle(t(400)).expect("starts");
        assert!(s.sources().is_err(), "still running");
        s.move_focus(-2);
        assert!(s.toggle(t(410)).is_err(), "one span at a time");
        s.cycle_meter(1);
        assert!(s.error.is_some(), "FOH stays the meter's own");
        s.move_focus(2);
        let c = s.toggle(t(460)).expect("stops").expect("an ask");
        assert_eq!(c.meas, MeasId(2));
        assert!(s.sources().expect("sources").2.is_some());
        assert_eq!(
            s.next_step(),
            "Enter computes the band transfer and stores it in FOH SPL."
        );
        // An answer for the span as it is lands; one for an older span does not.
        let reply = |avg: u32| SplBandLog {
            meas: c.meas,
            from: c.from,
            until: c.until,
            step: None,
            average: BandLogAverage {
                seconds: avg,
                measured: Seconds(f64::from(avg)),
                uncalibrated: 0,
                levels: None,
            },
            rows: Vec::new(),
        };
        s.answered(c, Ok(reply(60)));
        assert_eq!(
            s.span(SpanRole::Background).average.map(|a| a.seconds),
            Some(60)
        );
        s.answered(Ask { until: t(999), ..c }, Ok(reply(1)));
        assert_eq!(
            s.span(SpanRole::Background).average.map(|a| a.seconds),
            Some(60)
        );
        s.answered(a, Err("gone".into()));
        assert_eq!(s.span(SpanRole::Foh).error.as_deref(), Some("gone"));
        s.move_focus(-2);
        s.clear();
        assert_eq!(s.span(SpanRole::Foh).state, SpanState::Unmarked);
        assert!(s.sources().is_err());
    }
}

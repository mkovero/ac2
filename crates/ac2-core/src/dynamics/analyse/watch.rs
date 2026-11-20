//! The rest watch: during each rest, the gain of every input about once per millisecond, and
//! from `rest_min` on whether the medians of its last two 1 s windows agree, which is when the
//! processor has stopped releasing and the next step may play.

use super::dsp::median;
use super::*;

impl Analyser {
    pub(super) fn watch_step(&self) -> u64 {
        self.p * ((WATCH_STEP_SECONDS * self.fs / self.p as f64).round() as u64).max(1)
    }

    pub(super) fn start_watch(&mut self) {
        if self.watch.is_some() || self.cal.is_none() {
            return;
        }
        let Some(f) = self.steps.get(self.watched) else {
            return;
        };
        let (Some(e), None) = (f.e, f.recovered) else {
            return;
        };
        self.watch = Some(Watch {
            step: self.watched,
            e,
            next: e + self.watch_step(),
            g: vec![Vec::new(); self.store.inputs.len()],
        });
    }

    /// The first sample a rest's gain readings must not reach: its end where known.
    pub(super) fn watch_limit(&self, w: &Watch) -> Option<u64> {
        let last = w.step + 1 == self.prog.steps();
        match &self.prog.rests {
            Rests::Replay(r) => Some(w.e + u64::from(r[w.step]) * self.p),
            Rests::Adaptive if last => Some(w.e + u64::from(self.prog.rest_max_periods) * self.p),
            Rests::Adaptive => self.steps[w.step].end,
        }
    }

    pub(super) fn watch_point(&mut self) {
        let (Some(cal), Some(mut w)) = (self.cal.as_ref(), self.watch.take()) else {
            return;
        };
        let t = w.next;
        let r = self.store.reference();
        let rc = centred_at(r, t, &self.mx).norm();
        for (i, g) in w.g.iter_mut().enumerate() {
            let y = centred_at(self.store.input(i, cal.shift[i]), t, &self.mx).norm();
            g.push(20.0 * (y / rc).log10() - cal.g0[i]);
        }
        w.next += self.watch_step();
        let since = t - w.e;
        let eval = self.watch_step()
            * ((WATCH_EVAL_SECONDS * self.fs / self.watch_step() as f64).round() as u64).max(1);
        let adaptive = matches!(self.prog.rests, Rests::Adaptive);
        if adaptive
            && since >= u64::from(self.prog.rest_min_periods) * self.p
            && since.is_multiple_of(eval)
            && self.flat(&w)
        {
            let i = w.step;
            self.released = self.released.max(i as u32 + 1);
            self.steps[i].recovered = Some(true);
            self.watched = i + 1;
            if i + 1 == self.prog.steps() {
                // The source plays at least one more period of pilot before its fade (it
                // reads the release only after this point was computed), and a replay of
                // this length reads its last point here too.
                self.steps[i].end = Some(t + self.p);
            }
            return;
        }
        self.watch = Some(w);
    }

    pub(super) fn watch_end(&mut self) {
        let Some(w) = self.watch.take() else {
            return;
        };
        let i = w.step;
        let recovered = match self.prog.rests {
            Rests::Replay(_) => self.flat(&w),
            Rests::Adaptive => false,
        };
        self.steps[i].recovered = Some(recovered);
        self.watched = i + 1;
        if self.steps[i].end.is_none() {
            self.steps[i].end = self.watch_limit(&w);
        }
    }

    /// The medians of the last two windows agree on every input.
    pub(super) fn flat(&self, w: &Watch) -> bool {
        let n =
            ((FLAT_WINDOW_SECONDS * self.fs / self.watch_step() as f64).round() as usize).max(1);
        w.g.iter().all(|g| {
            if g.len() < 2 * n {
                return false;
            }
            let mut a = g[g.len() - 2 * n..g.len() - n].to_vec();
            let mut b = g[g.len() - n..].to_vec();
            (median(&mut a) - median(&mut b)).abs() < FLAT_DB
        })
    }
}

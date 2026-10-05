//! `spl.history_get`: each Leq window of a meter second by second, rebuilt from its log as
//! the meter's job computed it ([`ac2_core::leq::LogReplay`]), so a client that was not
//! connected (an app restarted, another machine) gets what the `leq` frames it missed
//! carried (`docs/design/leq.md`, *The history strip*).

use ac2_core::leq::{Judgement, Latch, LogReplay, WindowSpec, judge_window};
use ac2_proto::model::{LeqConfig, LevelScale, SplHistory, SplLogRow};
use ac2_proto::units::{MeasId, WallNs};

use crate::conv;
use crate::leq_log::row_second;

const NS: u64 = 1_000_000_000;

/// Rows the history of the newest `seconds` needs: those seconds, and before them the
/// longest window of `cfg`, whose seconds the first of them is computed over. A log has at
/// most a row a second, so as many rows cover at least as much time.
pub(crate) fn rows_needed(cfg: &LeqConfig, seconds: u32) -> usize {
    let longest = cfg
        .windows
        .iter()
        .filter_map(|w| w.seconds())
        .max()
        .unwrap_or(1);
    seconds as usize + longest as usize
}

/// Each window of `cfg` second by second over `rows` (the newest of the meter's current
/// log, oldest first; `from_log_start` when the first of them is the log's first row), as
/// the `leq` frames carried them: the Leq in the meter's unit then (dB SPL with the row's
/// sensitivity, or dBFS) as f32, and whether the window was over its limit, judged as the
/// job judges (a filling window on its budget, with the job's hysteresis: the latches run
/// over every row given, so begun part way through the log a state the job held from
/// before the first row can be released up to `RELEASE_HOLD_S` seconds early). A second's time is its end; a second
/// without a row gets none (no frame was sent for it). Seconds before the replay is settled
/// (begun part way through the log, the longest window not yet holding only replayed rows)
/// are left out, as are those before the last change of unit and those more than `seconds`
/// before the newest.
pub(crate) fn history(
    meas: MeasId,
    cfg: &LeqConfig,
    rows: &[SplLogRow],
    from_log_start: bool,
    seconds: u32,
) -> SplHistory {
    let specs: Vec<WindowSpec> = cfg
        .windows
        .iter()
        .map(|w| WindowSpec {
            seconds: w.seconds().unwrap_or(1),
            weighting: conv::weighting(w.weighting),
        })
        .collect();
    let mut replay = LogReplay::new(&specs, cfg.horizon_seconds().unwrap_or(60), from_log_start);
    let n = cfg.windows.len();
    let mut h = SplHistory {
        meas,
        windows: cfg.windows.clone(),
        scale: LevelScale::Dbfs,
        at: Vec::new(),
        leq: vec![Vec::new(); n],
        over: vec![Vec::new(); n],
    };
    let mut scale = None;
    let mut latches = vec![Latch::default(); n];
    for r in rows {
        replay.push(r.start.0, row_second(r));
        let offset = r.sensitivity.map(|s| s.0);
        let windows = replay.windows();
        let over: Vec<bool> = cfg
            .windows
            .iter()
            .zip(&mut latches)
            .enumerate()
            .map(|(i, (w, latch))| match (w.limit, offset) {
                (Some(limit), Some(o)) => {
                    let v = windows.value(i);
                    latch
                        .judge(
                            judge_window(&v, o, limit.0, w.warn_margin.0),
                            v.leq_dbfs + o,
                            limit.0,
                            w.warn_margin.0,
                        )
                        .is_some_and(|x| x.judgement == Judgement::Over)
                }
                _ => {
                    *latch = Latch::default();
                    false
                }
            })
            .collect();
        if !replay.settled() {
            continue;
        }
        let s = if offset.is_some() {
            LevelScale::DbSpl
        } else {
            LevelScale::Dbfs
        };
        if scale != Some(s) {
            // A change of unit starts the history over, as it does for a client live.
            h.at.clear();
            h.leq.iter_mut().for_each(Vec::clear);
            h.over.iter_mut().for_each(Vec::clear);
            scale = Some(s);
        }
        h.at.push(WallNs(r.start.0 + NS));
        for (i, over) in over.into_iter().enumerate() {
            let v = windows.value(i);
            // As the job puts it in the frame: f64 levels, sent as f32.
            h.leq[i].push((v.leq_dbfs + offset.unwrap_or(0.0)) as f32);
            h.over[i].push(over);
        }
    }
    if let Some(newest) = h.at.last().copied() {
        let from =
            h.at.partition_point(|t| t.0 + u64::from(seconds) * NS < newest.0);
        h.at.drain(..from);
        h.leq.iter_mut().for_each(|v| drop(v.drain(..from)));
        h.over.iter_mut().for_each(|v| drop(v.drain(..from)));
    }
    h.scale = scale.unwrap_or(LevelScale::Dbfs);
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_core::leq::{RollingLeq, Second};
    use ac2_proto::model::{LeqWindow, Weighting};
    use ac2_proto::units::{Db, DbSpl, Dbfs, Seconds};

    const T0: u64 = 1_790_000_000 * NS;

    fn cfg() -> LeqConfig {
        let w = |seconds: f64, weighting, limit: Option<f64>| LeqWindow {
            duration: Seconds(seconds),
            weighting,
            limit: limit.map(DbSpl),
            warn_margin: Db(3.0),
        };
        LeqConfig {
            windows: vec![
                w(10.0, Weighting::A, Some(90.0)),
                w(60.0, Weighting::A, Some(88.0)),
                w(60.0, Weighting::C, None),
                w(300.0, Weighting::A, Some(85.0)),
            ],
            horizon: Seconds(30.0),
        }
    }

    /// A calibrated meter (120 dB SPL at 0 dBFS): 10 min at 80 dB(A), 3 min at 95, the
    /// meter stopped for 2 min, 5 min at 84; a lost second now and then, a partial one,
    /// wall times a little late.
    fn rows() -> Vec<SplLogRow> {
        let mut v = Vec::new();
        let mut t = T0;
        for k in 0..1080u64 {
            t += match k {
                0 => 0,
                780 => 121 * NS,
                _ if k % 97 == 0 => 2 * NS,
                _ => NS,
            };
            let a = match k {
                ..600 => -40.0 + (k % 7) as f64 * 0.3,
                600..780 => -25.0,
                _ => -36.0,
            };
            v.push(SplLogRow {
                start: WallNs(t + (k % 5) * 3_000_000),
                measured: Seconds(if k % 41 == 0 { 0.6 } else { 1.0 }),
                laeq: Dbfs(a),
                lceq: Dbfs(a + 4.0),
                lzeq: Dbfs(a + 6.0),
                sensitivity: Some(Db(120.0)),
            });
        }
        v
    }

    /// One second of a frame: its time (ns), and per window the Leq and over flag.
    type Live = Vec<(u64, Vec<(f32, bool)>)>;

    /// What the job's `leq` frames carry while it logs `rows`: windows refilled from the
    /// log whenever it starts (the first row and after the pause), a lost second pushed as
    /// a gap, a frame after every logged second, judged as the job judges.
    fn live(cfg: &LeqConfig, rows: &[SplLogRow]) -> Live {
        let specs: Vec<WindowSpec> = cfg
            .windows
            .iter()
            .map(|w| WindowSpec {
                seconds: w.seconds().expect("whole"),
                weighting: conv::weighting(w.weighting),
            })
            .collect();
        let mut ring = RollingLeq::new(&specs, 30);
        let mut latches = vec![Latch::default(); cfg.windows.len()];
        let mut out = Vec::new();
        for (i, r) in rows.iter().enumerate() {
            let step = (i > 0).then(|| (r.start.0 - rows[i - 1].start.0 + NS / 2) / NS);
            match step {
                // The job runs on; a second it lost (nothing measured, no row) is a gap.
                Some(s) if s <= 2 => {
                    for _ in 1..s {
                        ring.push(Second::GAP);
                    }
                }
                _ => ring.refill(
                    rows[..i].iter().rev().map(|r| (r.start.0, row_second(r))),
                    r.start.0,
                ),
            }
            ring.push(row_second(r));
            let o = 120.0;
            let vals = cfg
                .windows
                .iter()
                .enumerate()
                .zip(&mut latches)
                .map(|((k, w), latch)| {
                    let v = ring.value(k);
                    let over = w.limit.is_some_and(|l| {
                        latch
                            .judge(
                                judge_window(&v, o, l.0, w.warn_margin.0),
                                v.leq_dbfs + o,
                                l.0,
                                w.warn_margin.0,
                            )
                            .is_some_and(|j| j.judgement == Judgement::Over)
                    });
                    ((v.leq_dbfs + o) as f32, over)
                })
                .collect();
            out.push((r.start.0 + NS, vals));
        }
        out
    }

    fn assert_same(live: &[(u64, Vec<(f32, bool)>)], h: &SplHistory) {
        assert_eq!(live.len(), h.at.len());
        for (k, (t, vals)) in live.iter().enumerate() {
            assert_eq!(h.at[k].0, *t);
            for (w, (leq, over)) in vals.iter().enumerate() {
                assert_eq!(
                    h.leq[w][k].to_bits(),
                    leq.to_bits(),
                    "second {k} window {w}"
                );
                assert_eq!(h.over[w][k], *over, "second {k} window {w}");
            }
        }
    }

    /// Rebuilt from the log, every window's series is what the live frames carried over
    /// the same seconds, to the bit: through the loud stretch (over its limits), lost
    /// seconds and the pause.
    #[test]
    fn history_equals_the_live_frames() {
        let (c, rows) = (cfg(), rows());
        let live = live(&c, &rows);
        let h = history(MeasId(4), &c, &rows, true, SplHistory::MAX_SECONDS);
        assert_eq!(h.scale, LevelScale::DbSpl);
        assert_eq!(h.windows, c.windows);
        assert_same(&live, &h);
        let overs: usize = h.over.iter().flatten().filter(|o| **o).count();
        assert!(overs > 100, "the loud stretch is over the limits: {overs}");
        // The newest 300 s only.
        let h = history(MeasId(4), &c, &rows, true, 300);
        let newest = h.at.last().expect("seconds").0;
        assert!(h.at.iter().all(|t| t.0 + 300 * NS >= newest));
        assert!(h.at.len() > 290 && h.at.len() <= 301, "{}", h.at.len());
        assert_same(&live[live.len() - h.at.len()..], &h);
    }

    /// Begun part way through the log (the rows before not read), the history starts
    /// where the longest window holds only rows read, and is the live one from there.
    #[test]
    fn history_from_part_way_starts_once_settled() {
        let (c, rows) = (cfg(), rows());
        let live = live(&c, &rows);
        let h = history(MeasId(4), &c, &rows[200..], false, SplHistory::MAX_SECONDS);
        assert!(
            h.at.len() < rows.len() - 200 && h.at.len() > 400,
            "{}",
            h.at.len()
        );
        assert_same(&live[live.len() - h.at.len()..], &h);
        assert_eq!(rows_needed(&c, 3600), 3900);
    }

    /// Only the seconds after the last change of unit: the meter calibrated part way.
    #[test]
    fn history_starts_over_at_a_change_of_unit() {
        let (c, mut rows) = (cfg(), rows());
        for r in &mut rows[..500] {
            r.sensitivity = None;
        }
        let h = history(MeasId(4), &c, &rows, true, SplHistory::MAX_SECONDS);
        assert_eq!(h.scale, LevelScale::DbSpl);
        assert_eq!(h.at.len(), rows.len() - 500);
        let h = history(MeasId(4), &c, &[], true, SplHistory::MAX_SECONDS);
        assert!(h.at.is_empty() && h.leq.iter().all(Vec::is_empty));
    }
}

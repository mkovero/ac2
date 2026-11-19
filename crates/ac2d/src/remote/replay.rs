//! A recording's remote-channel marks put back on its replay. Live, the joined feed marks
//! the samples another rig never delivered (zeros, no signal) and the moves of its map; the
//! recorder lists them in the sidecar per file channel. A replay delivers the file as one
//! device's blocks, so the fan-out stamps those marks onto them again: meters, SPL and
//! transfer functions then read a gap as lost time and a slip as a move, as they did live,
//! never the zeros as silence.

use std::ops::Range;

use crate::fanout::Block;

/// One column's missing sample ranges, oldest first and not overlapping, and whether they
/// arrived late.
type ColumnGaps = Vec<(Range<u64>, bool)>;

/// The marks of a replay's columns, in replay samples.
#[derive(Debug, Default)]
pub(crate) struct ReplayMarks {
    /// Per column: its gaps.
    gaps: Vec<(u16, ColumnGaps)>,
    /// Per column: the sample a slip lands on and its size, oldest first.
    slips: Vec<(u16, Vec<(u64, i64)>)>,
}

impl ReplayMarks {
    /// From `(column, samples, late)` gaps and `(column, sample, by)` slips; `None` without
    /// any.
    pub(crate) fn new(
        gaps: impl IntoIterator<Item = (u16, Range<u64>, bool)>,
        slips: impl IntoIterator<Item = (u16, u64, i64)>,
    ) -> Option<Self> {
        let mut m = Self::default();
        for (col, r, late) in gaps {
            if r.is_empty() {
                continue;
            }
            column(&mut m.gaps, col).push((r, late));
        }
        for (col, at, by) in slips {
            column(&mut m.slips, col).push((at, by));
        }
        for (_, g) in &mut m.gaps {
            g.sort_by_key(|(r, _)| r.start);
        }
        for (_, s) in &mut m.slips {
            s.sort_by_key(|&(at, _)| at);
        }
        (!m.gaps.is_empty() || !m.slips.is_empty()).then_some(m)
    }

    /// Marks `b` where its samples fall in a gap or on a slip.
    pub(crate) fn stamp(&self, b: &mut Block) {
        let s = b.start_sample;
        let e = s + u64::from(b.frames);
        for (col, gaps) in &self.gaps {
            let mut i = gaps.partition_point(|(r, _)| r.end <= s);
            while let Some((r, late)) = gaps.get(i).filter(|(r, _)| r.start < e) {
                let lo = u32::try_from(r.start.max(s) - s).unwrap_or(u32::MAX);
                let hi = u32::try_from(r.end.min(e) - s).unwrap_or(u32::MAX);
                b.marks.gap_span(*col, lo..hi);
                if *late {
                    b.marks.late(*col);
                }
                i += 1;
            }
        }
        for (col, slips) in &self.slips {
            let mut i = slips.partition_point(|&(at, _)| at < s);
            while let Some(&(_, by)) = slips.get(i).filter(|&&(at, _)| at < e) {
                b.marks.slip(*col, by);
                i += 1;
            }
        }
    }
}

fn column<T>(v: &mut Vec<(u16, Vec<T>)>, col: u16) -> &mut Vec<T> {
    let i = match v.iter().position(|(c, _)| *c == col) {
        Some(i) => i,
        None => {
            v.push((col, Vec::new()));
            v.len() - 1
        }
    };
    &mut v[i].1
}

#[cfg(test)]
mod tests {
    #![allow(clippy::single_range_in_vec_init)]
    use super::*;
    use ac2_audio::BlockFlags;

    fn block(start: u64, frames: u32) -> Block {
        Block::new(
            start,
            frames,
            2,
            BlockFlags::NONE,
            0,
            vec![0.0; 2 * frames as usize],
        )
    }

    #[test]
    fn gaps_and_slips_land_on_the_blocks_they_fall_in() {
        let m = ReplayMarks::new([(1, 100..300, false), (1, 600..610, true)], [(1, 512, -3)])
            .expect("marks");
        let mut a = block(0, 256);
        m.stamp(&mut a);
        assert_eq!(a.marks.gaps(1), &[100..256]);
        assert!(a.marks.gaps(0).is_empty(), "the other column reads on");
        assert!(!a.marks.get(1).is_some_and(|c| c.late));
        let mut b = block(256, 256);
        m.stamp(&mut b);
        assert_eq!(b.marks.gaps(1), &[0..44]);
        assert!(!b.marks.breaks(0));
        let mut c = block(512, 256);
        m.stamp(&mut c);
        assert_eq!(c.marks.gaps(1), &[88..98]);
        let mark = c.marks.get(1).expect("marked");
        assert!(mark.late);
        assert_eq!(mark.slip, -3);
        let mut d = block(768, 256);
        m.stamp(&mut d);
        assert!(!d.marks.breaks(1));
    }

    #[test]
    fn a_recording_without_marks_has_none() {
        assert!(ReplayMarks::new([], []).is_none());
    }
}

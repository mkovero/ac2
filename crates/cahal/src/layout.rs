//! Where each device channel lives in an IOProc's buffers, and the views a cycle hands to
//! the closure.
//!
//! An IOProc gets one `AudioBuffer` per interleaved stream (all of its channels, frame by
//! frame) or one per channel of a non-interleaved stream. A device can have several streams
//! per direction (an interface: analog 1–8 in one, ADAT 9–16 in another). The table is built
//! once at start, so a cycle finds any channel without searching.

use crate::types::{CycleTime, StreamLayout};

/// Where one device channel is: `buffers[buffer][offset + frame * stride]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelLoc {
    /// Index of the `AudioBuffer` in the cycle's list.
    pub buffer: usize,
    /// First sample of the channel in that buffer.
    pub offset: usize,
    /// Samples from one frame to the next.
    pub stride: usize,
}

/// Device channel → buffer, for one direction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelTable {
    locs: Vec<ChannelLoc>,
    buffers: Vec<u32>,
}

impl ChannelTable {
    /// The table for `layout`, the direction's streams in device order.
    pub fn new(layout: &StreamLayout) -> Self {
        let mut t = Self::default();
        for s in &layout.streams {
            if s.interleaved || s.channels <= 1 {
                let buffer = t.buffers.len();
                t.buffers.push(s.channels);
                let stride = s.channels.max(1) as usize;
                t.locs
                    .extend((0..s.channels as usize).map(|offset| ChannelLoc {
                        buffer,
                        offset,
                        stride,
                    }));
            } else {
                for _ in 0..s.channels {
                    t.locs.push(ChannelLoc {
                        buffer: t.buffers.len(),
                        offset: 0,
                        stride: 1,
                    });
                    t.buffers.push(1);
                }
            }
        }
        t
    }

    /// Device channels in the direction.
    pub fn channels(&self) -> usize {
        self.locs.len()
    }

    /// Channels each `AudioBuffer` of a cycle must carry, in list order.
    pub fn buffers(&self) -> &[u32] {
        &self.buffers
    }

    /// Where device channel `channel` is.
    pub fn loc(&self, channel: usize) -> Option<ChannelLoc> {
        self.locs.get(channel).copied()
    }
}

/// One channel of a cycle's input, read-only.
#[derive(Clone, Copy, Debug)]
pub struct Channel<'a> {
    data: &'a [f32],
    offset: usize,
    stride: usize,
    frames: usize,
}

impl Channel<'_> {
    /// Frames in the cycle.
    pub fn len(&self) -> usize {
        self.frames
    }

    /// True for a cycle of no frames.
    pub fn is_empty(&self) -> bool {
        self.frames == 0
    }

    /// Sample `frame`; 0 past the end or where the HAL gave no buffer (a disabled stream).
    #[inline]
    pub fn get(&self, frame: usize) -> f32 {
        if frame >= self.frames {
            return 0.0;
        }
        self.data
            .get(self.offset + frame * self.stride)
            .copied()
            .unwrap_or(0.0)
    }
}

/// One channel of a cycle's output.
#[derive(Debug)]
pub struct ChannelMut<'a> {
    data: &'a mut [f32],
    offset: usize,
    stride: usize,
    frames: usize,
}

impl ChannelMut<'_> {
    /// Frames in the cycle.
    pub fn len(&self) -> usize {
        self.frames
    }

    /// True for a cycle of no frames.
    pub fn is_empty(&self) -> bool {
        self.frames == 0
    }

    /// Writes sample `frame`; ignored past the end.
    #[inline]
    pub fn set(&mut self, frame: usize, value: f32) {
        if frame < self.frames
            && let Some(s) = self.data.get_mut(self.offset + frame * self.stride)
        {
            *s = value;
        }
    }
}

/// The input buffers of one cycle.
#[derive(Clone, Copy, Debug)]
pub struct Input<'a> {
    bufs: &'a [&'a [f32]],
    table: &'a ChannelTable,
    frames: usize,
}

impl<'a> Input<'a> {
    /// A view of `bufs` (one slice per `AudioBuffer`, in the table's order) holding
    /// `frames` frames. The IOProc builds it from the HAL's buffers; tests build it from
    /// their own.
    pub fn new(bufs: &'a [&'a [f32]], table: &'a ChannelTable, frames: usize) -> Self {
        Self {
            bufs,
            table,
            frames,
        }
    }

    /// Frames in the cycle.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Device channels.
    pub fn channels(&self) -> usize {
        self.table.channels()
    }

    /// Device channel `channel`; an empty channel (all zeros) past the last.
    #[inline]
    pub fn channel(&self, channel: usize) -> Channel<'a> {
        match self.table.loc(channel) {
            Some(l) => Channel {
                data: self.bufs.get(l.buffer).copied().unwrap_or(&[]),
                offset: l.offset,
                stride: l.stride,
                frames: self.frames,
            },
            None => Channel {
                data: &[],
                offset: 0,
                stride: 1,
                frames: self.frames,
            },
        }
    }
}

/// The output buffers of one cycle, zeroed before the closure runs.
#[derive(Debug)]
pub struct Output<'a, 'b> {
    bufs: &'a mut [&'b mut [f32]],
    table: &'a ChannelTable,
    frames: usize,
}

impl<'a, 'b> Output<'a, 'b> {
    /// A view of `bufs` (one slice per `AudioBuffer`, in the table's order) holding
    /// `frames` frames.
    pub fn new(bufs: &'a mut [&'b mut [f32]], table: &'a ChannelTable, frames: usize) -> Self {
        Self {
            bufs,
            table,
            frames,
        }
    }

    /// Frames in the cycle.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Device channels.
    pub fn channels(&self) -> usize {
        self.table.channels()
    }

    /// Device channel `channel`; writes past the last channel go nowhere.
    #[inline]
    pub fn channel_mut(&mut self, channel: usize) -> ChannelMut<'_> {
        let loc = self.table.loc(channel);
        match loc.and_then(|l| self.bufs.get_mut(l.buffer).map(|b| (l, b))) {
            Some((l, b)) => ChannelMut {
                data: b,
                offset: l.offset,
                stride: l.stride,
                frames: self.frames,
            },
            None => ChannelMut {
                data: &mut [],
                offset: 0,
                stride: 1,
                frames: self.frames,
            },
        }
    }

    /// Zeroes every buffer.
    pub fn silence(&mut self) {
        for b in self.bufs.iter_mut() {
            b.fill(0.0);
        }
    }
}

/// One IOProc cycle, borrowed from the HAL for the call only.
#[derive(Debug)]
pub struct Cycle<'a, 'b> {
    /// What the device captured.
    pub input: Input<'a>,
    /// What it will play.
    pub output: Output<'a, 'b>,
    /// Index and times.
    pub time: CycleTime,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{StreamFormat, StreamInfo};

    fn stream(channels: u32, interleaved: bool) -> StreamInfo {
        StreamInfo {
            channels,
            interleaved,
            format: StreamFormat::Float32,
        }
    }

    #[test]
    fn one_interleaved_stream() {
        let t = ChannelTable::new(&StreamLayout {
            streams: vec![stream(2, true)],
        });
        assert_eq!(t.buffers(), &[2]);
        let buf = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let bufs = [&buf[..]];
        let input = Input::new(&bufs, &t, 3);
        let c1 = input.channel(1);
        assert_eq!(
            (c1.get(0), c1.get(1), c1.get(2), c1.get(3)),
            (2.0, 4.0, 6.0, 0.0)
        );
        assert_eq!(input.channel(2).get(0), 0.0);
    }

    #[test]
    fn several_streams_in_device_order() {
        // Analog 1–2 in one stream, then a mono stream, then 3 non-interleaved channels.
        let t = ChannelTable::new(&StreamLayout {
            streams: vec![stream(2, true), stream(1, true), stream(3, false)],
        });
        assert_eq!(t.channels(), 6);
        assert_eq!(t.buffers(), &[2, 1, 1, 1, 1]);
        assert_eq!(
            t.loc(1),
            Some(ChannelLoc {
                buffer: 0,
                offset: 1,
                stride: 2
            })
        );
        assert_eq!(
            t.loc(2),
            Some(ChannelLoc {
                buffer: 1,
                offset: 0,
                stride: 1
            })
        );
        assert_eq!(
            t.loc(5),
            Some(ChannelLoc {
                buffer: 4,
                offset: 0,
                stride: 1
            })
        );
        let (a, b, c, d, e) = (
            [0.0, 1.0, 0.0, 1.0],
            [2.0, 2.0],
            [3.0, 3.0],
            [4.0, 4.0],
            [5.0, 5.5],
        );
        let bufs = [&a[..], &b[..], &c[..], &d[..], &e[..]];
        let input = Input::new(&bufs, &t, 2);
        assert_eq!(input.channel(5).get(1), 5.5);
        assert_eq!(input.channel(2).get(0), 2.0);
    }

    #[test]
    fn a_missing_buffer_reads_as_silence() {
        let t = ChannelTable::new(&StreamLayout {
            streams: vec![stream(2, true), stream(2, true)],
        });
        let a = [1.0; 8];
        let bufs = [&a[..], &[][..]];
        let input = Input::new(&bufs, &t, 4);
        assert_eq!(input.channel(0).get(3), 1.0);
        assert_eq!(input.channel(3).get(0), 0.0);
    }

    #[test]
    fn output_writes_land_on_their_channel_only() {
        let t = ChannelTable::new(&StreamLayout {
            streams: vec![stream(2, true), stream(2, false)],
        });
        let (mut a, mut b, mut c) = ([9.0f32; 6], [9.0f32; 3], [9.0f32; 3]);
        let mut bufs = [&mut a[..], &mut b[..], &mut c[..]];
        let mut out = Output::new(&mut bufs, &t, 3);
        out.silence();
        for f in 0..3 {
            out.channel_mut(1).set(f, 1.0);
            out.channel_mut(3).set(f, 3.0);
            out.channel_mut(7).set(f, 7.0);
        }
        out.channel_mut(0).set(3, 5.0);
        assert_eq!(a, [0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
        assert_eq!(b, [0.0; 3]);
        assert_eq!(c, [3.0; 3]);
    }
}

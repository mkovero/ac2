//! Stereo: a second channel mirrored in the floor, as if a perfectly still lake lay between
//! the two. The first channel stands on the floor as always; the second hangs below it,
//! each of its spikes the reflection of where that channel's own spike would stand, so the
//! same frequency and age meet at the same point of the surface and the eye compares the two
//! shapes across one line.

use super::*;

/// The tag the legend and the caption give the mirrored channel.
pub const MIRRORED: &str = "mirrored";

/// The second channel of a 3D spectrograph: its history and skyline, kept like the first's
/// ([`Spectrograph3dInput`]) on the pane's frequency span and tier.
#[derive(Clone, Debug)]
pub struct Mirror3dInput<'a> {
    /// Its live curve: the legend row tagged [`MIRRORED`].
    pub key: TraceKey,
    pub history: &'a SpectrographHistory,
    pub skyline: &'a Skyline,
    pub name: String,
    /// Its own display offset, dB: the levels it draws are its displayed ones, as the
    /// first channel's are.
    pub offset_db: f64,
}

impl Mirror3dInput<'_> {
    /// Its name as the caption gives it, the tag first so an offset note stays its own.
    pub(super) fn caption_name(&self) -> String {
        let name = if self.offset_db != 0.0 {
            offset_note(&self.name, self.offset_db)
        } else {
            self.name.clone()
        };
        format!("{MIRRORED} {name}")
    }

    /// The mirrored rows for the renderer at the scroll `lead_slots` past the newest slot,
    /// heights over the pane's level `range`; `None` while there is nothing to draw (no
    /// frame yet, or no column within the span), which leaves the lake empty.
    pub(super) fn rows(&self, range: Range, lead_slots: f64) -> Option<SkylineMirror> {
        let sk = self.skyline;
        if self.history.is_empty() || sk.span_hz().is_none() || sk.rows() == 0 {
            return None;
        }
        let drawn = sk.drawn(lead_slots);
        let off = self.offset_db;
        Some(SkylineMirror {
            cols: sk.cols() as u32,
            rows: sk.rows() as u32,
            newest: drawn.newest as u32,
            newest_id: drawn.newest_id,
            phase: drawn.phase,
            range: [(range.lo - off) as f32, (range.hi - off) as f32],
            data: sk.ring().to_vec(),
        })
    }
}

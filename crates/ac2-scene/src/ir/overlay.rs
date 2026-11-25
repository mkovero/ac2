//! The transfer pane's IR views over several curves: the live IR of the pane's measurement
//! and the IR of every stored trace the pane shows, each in its own colour, with a legend
//! of the same rows and order as the magnitude view's. A shown trace captured without an
//! IR keeps its row, tagged `no IR`, so nothing the magnitude view lists goes missing here
//! unsaid. Every curve is on the lead curve's time axis (ms re the lead's inserted delay,
//! whatever its sample rate): another curve inserted later is drawn that much later, and
//! its row says by how much (`Δt +0.50 ms`). In dB, every curve is re the lead's peak.

use ac2_proto::frame::IrFrame;

use super::{Extra, IrCtx, IrScene, draw};
use crate::primitives::Color;
use crate::tf::LegendEntry;
use crate::time::Freshness;
use crate::trace::TraceKey;

/// The tag of a shown curve that has no impulse response to draw.
pub const NO_IR: &str = "no IR";

/// One curve of the view: a live measurement's (`Live`) or a stored trace's.
#[derive(Clone, Debug)]
pub struct IrCurve<'a> {
    pub key: TraceKey,
    pub name: String,
    /// Tags before the freshness one (`cmp` for a curve compared into the pane).
    pub tags: Vec<String>,
    /// The impulse response; `None` for a trace captured without one.
    pub frame: Option<&'a IrFrame>,
    pub color: Color,
    /// A live curve's: its tag (`stopped`, `STALE 3.2 s`) and whether it is dimmed.
    pub freshness: Option<Freshness>,
    pub selected: bool,
}

impl IrCurve<'_> {
    /// Its legend row; `lead_delay`: the lead curve's inserted delay, against which a
    /// later or earlier time base is said as the magnitude view says one (`Δt +0.50 ms`).
    fn entry(&self, lead_delay: Option<f64>) -> LegendEntry {
        let mut tags = self.tags.clone();
        match (self.frame, lead_delay) {
            (None, _) => tags.push(NO_IR.to_owned()),
            (Some(f), Some(lead)) => {
                let d = f.meta.inserted_delay.0 - lead;
                // Below a tenth of a sample at 192 kHz, the same time base.
                if d.abs() > 5e-7 {
                    tags.push(format!("Δt {}", crate::tf::signed_ms(d)));
                }
            }
            (Some(_), None) => {}
        }
        tags.extend(self.freshness.and_then(|f| f.tag()));
        let mut text = self.name.clone();
        for t in &tags {
            text.push_str(" · ");
            text.push_str(t);
        }
        LegendEntry {
            key: self.key,
            name: self.name.clone(),
            tags,
            text,
            stale: self.frame.is_none() || self.freshness.is_some_and(|f| f.is_stale()),
            selected: self.selected,
        }
    }
}

/// The IR view of `curves`, in legend order. The lead, whose IR the cursor reads and whose
/// inserted delay is time zero, is the selected curve with an IR, else the first with one.
/// `None` when no curve has an IR. One curve alone draws without a legend, as before.
pub fn ir_overlay_scene(
    curves: &[IrCurve<'_>],
    ctx: &IrCtx,
    between: Option<(&str, &str)>,
) -> Option<IrScene> {
    let lead = curves
        .iter()
        .position(|c| c.selected && c.frame.is_some())
        .or_else(|| curves.iter().position(|c| c.frame.is_some()))?;
    let l = &curves[lead];
    let frame = l.frame?;
    let tag = match l.key {
        TraceKey::Stored(_) => Some(l.name.clone()),
        TraceKey::Live(_) => l.freshness.and_then(|f| f.tag()),
    };
    let extra = if curves.len() > 1 {
        Extra {
            lines: curves
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != lead)
                .filter_map(|(_, c)| {
                    let stale = c.freshness.is_some_and(|f| f.is_stale());
                    Some((c.frame?, c.color, stale))
                })
                .collect(),
            legend: curves
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let lead_delay = (i != lead).then_some(frame.meta.inserted_delay.0);
                    (c.entry(lead_delay), c.color)
                })
                .collect(),
        }
    } else {
        Extra::default()
    };
    Some(draw(frame, l.color, l.freshness, tag, ctx, between, &extra))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::banner::Status;
    use crate::ir::{ir_values_re, peak_ref};
    use crate::primitives::{Color, Viewport};
    use crate::theme::Theme;
    use crate::view::{IrAxes, IrMode, IrView, PlotChrome, Style, ViewState};
    use ac2_proto::frame::IrMeta;
    use ac2_proto::units::{Hz, MeasId, Seconds, TraceId};

    const RED: Color = Color::rgb(1.0, 0.0, 0.0);
    const BLUE: Color = Color::rgb(0.0, 0.0, 1.0);
    const GREEN: Color = Color::rgb(0.0, 1.0, 0.0);

    /// An impulse of `peak` at t = 0 between −0.5 and +1.5 ms at `rate`.
    fn frame(rate: f64, peak: f32) -> IrFrame {
        frame_at(rate, peak, 0.01)
    }

    /// [`frame`] with `delay` s inserted.
    fn frame_at(rate: f64, peak: f32, delay: f64) -> IrFrame {
        let dt = 1.0 / rate;
        let n = (0.002 / dt).round() as usize + 1;
        let at = (0.0005 / dt).round() as usize;
        let mut linear = vec![0.0f32; n];
        linear[at] = peak;
        IrFrame {
            meas: MeasId(1),
            meta: IrMeta {
                sample_rate: Hz(rate),
                t0: Seconds(-(at as f64) * dt),
                dt: Seconds(dt),
                inserted_delay: Seconds(delay),
            },
            linear,
            etc: None,
        }
    }

    fn scene(curves: &[IrCurve<'_>], cursor_ms: Option<f64>) -> Option<IrScene> {
        scene_in(curves, cursor_ms, IrMode::Linear)
    }

    fn scene_in(curves: &[IrCurve<'_>], cursor_ms: Option<f64>, mode: IrMode) -> Option<IrScene> {
        let view = ViewState {
            ir: IrView {
                mode,
                ..IrView::default()
            },
            ..ViewState::default()
        };
        let axes = IrAxes {
            cursor_ms,
            ..IrAxes::default()
        };
        ir_overlay_scene(
            curves,
            &IrCtx {
                status: &Status::default(),
                view: &view,
                axes: &axes,
                style: Style {
                    chrome: PlotChrome::Full,
                    theme: &Theme::dark(),
                },
                size: Viewport::new(800.0, 300.0),
            },
            None,
        )
    }

    fn curve<'a>(key: TraceKey, name: &str, f: Option<&'a IrFrame>, color: Color) -> IrCurve<'a> {
        IrCurve {
            key,
            name: name.into(),
            tags: Vec::new(),
            frame: f,
            color,
            freshness: None,
            selected: false,
        }
    }

    /// The live curve stopped, a capture of it at another rate and one captured without an
    /// IR: three lines' worth of rows, in order, the last tagged and dimmed; two lines
    /// under the lead's, each in its colour, on one time axis.
    #[test]
    fn stored_irs_draw_over_the_live_one_with_a_legend() {
        let live = frame(48_000.0, 0.5);
        let kept = frame(96_000.0, 0.25);
        let mut l = curve(TraceKey::Live(MeasId(1)), "TF 1", Some(&live), RED);
        l.freshness = Some(Freshness::Stopped { age_s: 3.0 });
        let mut s1 = curve(TraceKey::Stored(TraceId(4)), "TF 1 S1", Some(&kept), BLUE);
        s1.tags = vec![crate::trace::COMPARED.to_owned()];
        let none = curve(TraceKey::Stored(TraceId(5)), "old", None, GREEN);
        let s = scene(&[l, s1, none], None).expect("drawn");

        let texts: Vec<&str> = s.legend.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["TF 1 · stopped", "TF 1 S1 · cmp", "old · no IR"]);
        assert!(s.legend[2].stale && !s.legend[1].stale);
        assert!(s.legend_box.is_some());
        assert_eq!(s.legend_hits.len(), 3);

        let lines = &s.scene.layers[1].polylines;
        let colors: Vec<Color> = lines.iter().map(|p| p.stroke.color).collect();
        assert!(colors.contains(&RED) && colors.contains(&BLUE));
        assert!(!colors.contains(&GREEN));
        // Both impulses at t = 0 land on the same pixel column whatever their rate.
        let peak_x = |c: Color| {
            let p = lines.iter().find(|p| p.stroke.color == c).expect("present");
            let top = p
                .points
                .iter()
                .min_by(|a, b| a[1].total_cmp(&b[1]))
                .expect("present");
            top[0]
        };
        assert!((peak_x(RED) - peak_x(BLUE)).abs() < 0.5);
    }

    /// Two time bases: a capture at 12.50 ms drawn with the live IR at 13.00 ms (another
    /// rate) peaks 0.5 ms before it, and its row says so; dB are re the lead's peak.
    #[test]
    fn another_inserted_delay_keeps_its_arrival_difference() {
        let live = frame_at(48_000.0, 0.5, 0.013);
        let kept = frame_at(96_000.0, 0.25, 0.0125);
        let l = curve(TraceKey::Live(MeasId(1)), "TF 1", Some(&live), RED);
        let s1 = curve(TraceKey::Stored(TraceId(4)), "TF 1 S1", Some(&kept), BLUE);
        let s = scene(&[l, s1], None).expect("drawn");
        let texts: Vec<&str> = s.legend.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["TF 1", "TF 1 S1 · Δt −0.50 ms"]);
        let lines = &s.scene.layers[1].polylines;
        let peak_x = |c: Color| {
            let p = lines.iter().find(|p| p.stroke.color == c).expect("present");
            p.points
                .iter()
                .min_by(|a, b| a[1].total_cmp(&b[1]))
                .expect("present")[0]
        };
        let px_per_ms = (s.x_axis.mapping.to_px(1.0) - s.x_axis.mapping.to_px(0.0)) as f64;
        let gap_ms = f64::from(peak_x(RED) - peak_x(BLUE)) / px_per_ms;
        assert!((gap_ms - 0.5).abs() < 0.03, "{gap_ms}");

        // The log view: the capture's peak is 6 dB under the live one's.
        let v = ir_values_re(&kept, IrMode::Log, peak_ref(&live, IrMode::Log)).expect("values");
        let top = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!((top + 6.02).abs() < 0.01, "{top}");
    }

    /// A curve louder than the lead is never clipped: in dB the top rises to fit it (by
    /// whole steps of the default headroom), and the automatic amplitude fits it too.
    #[test]
    fn a_louder_curve_raises_the_top() {
        let live = frame_at(48_000.0, 0.25, 0.013);
        let loud = frame_at(96_000.0, 0.5, 0.0125);
        let l = curve(TraceKey::Live(MeasId(1)), "TF 1", Some(&live), RED);
        let s1 = curve(TraceKey::Stored(TraceId(4)), "TF 1 S1", Some(&loud), BLUE);
        let s = scene(&[l.clone(), s1.clone()], None).expect("drawn");
        assert!(
            s.y_axis.mapping.range.hi >= 0.55,
            "{:?}",
            s.y_axis.mapping.range
        );
        let s = scene_in(&[l, s1], None, IrMode::Log).expect("drawn");
        // +6.02 dB and the 3 dB over it, up to the next 3 dB step.
        assert_eq!(s.y_axis.mapping.range.hi, 12.0);
    }

    /// The cursor reads the selected curve; time zero and the tag are that curve's.
    #[test]
    fn the_cursor_reads_the_selected_curve() {
        let live = frame(48_000.0, 0.5);
        let kept = frame(48_000.0, 0.25);
        let l = curve(TraceKey::Live(MeasId(1)), "TF 1", Some(&live), RED);
        let mut s1 = curve(TraceKey::Stored(TraceId(4)), "TF 1 S1", Some(&kept), BLUE);
        let first = scene(&[l.clone(), s1.clone()], Some(0.0)).expect("present");
        assert_eq!(first.cursor.expect("present").value, "+0.500 FS");
        s1.selected = true;
        let sel = scene(&[l, s1], Some(0.0)).expect("present");
        assert_eq!(sel.cursor.expect("present").value, "+0.250 FS");
        assert_eq!(sel.tag.as_deref(), Some("TF 1 S1"));
        assert!(sel.legend[1].selected);
    }

    /// One curve draws as it always has, without a legend; no IR at all draws nothing.
    #[test]
    fn one_curve_has_no_legend_and_none_without_an_ir() {
        let live = frame(48_000.0, 0.5);
        let one = scene(
            &[curve(TraceKey::Live(MeasId(1)), "TF 1", Some(&live), RED)],
            None,
        )
        .expect("present");
        assert!(one.legend.is_empty() && one.legend_box.is_none());
        assert!(
            scene(
                &[curve(TraceKey::Stored(TraceId(5)), "old", None, GREEN)],
                None
            )
            .is_none()
        );
    }
}

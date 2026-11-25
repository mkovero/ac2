//! The transfer pane's IR views over several curves: the live IR of the pane's measurement
//! and the IR of every stored trace the pane shows, each in its own colour, with a legend
//! of the same rows and order as the magnitude view's. A shown trace captured without an
//! IR keeps its row, tagged `no IR`, so nothing the magnitude view lists goes missing here
//! unsaid. Every curve is on one time axis, ms re its own inserted delay, whatever its
//! sample rate.

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
    fn entry(&self) -> LegendEntry {
        let mut tags = self.tags.clone();
        if self.frame.is_none() {
            tags.push(NO_IR.to_owned());
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
            legend: curves.iter().map(|c| (c.entry(), c.color)).collect(),
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
                inserted_delay: Seconds(0.01),
            },
            linear,
            etc: None,
        }
    }

    fn scene(curves: &[IrCurve<'_>], cursor_ms: Option<f64>) -> Option<IrScene> {
        let view = ViewState {
            ir: IrView {
                mode: IrMode::Linear,
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

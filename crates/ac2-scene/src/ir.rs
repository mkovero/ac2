//! Live impulse-response pane: linear, log (|h| in dB re peak) or ETC, on a time axis whose
//! origin is the inserted delay.
//!
//! The IR frame's point `i` is at `t0 + i·dt` seconds relative to the inserted delay, so
//! t = 0 is where the delay finder put the arrival; the absolute time is `t + inserted`.
//! The log and ETC views are normalised to their own peak (0 dB = strongest point), which
//! is display scaling, not a level claim.

use ac2_proto::frame::IrFrame;

use crate::axis::{self, Axis, Range};
use crate::banner::{BannerRow, Status};
use crate::canvas::{self, Canvas, MARGINS, anchor, gapped, label};
use crate::format;
use crate::primitives::{Color, HAlign, Polyline, Rect, Scene, Stroke, VAlign, Viewport};
use crate::readout;
use crate::theme::Theme;
use crate::time::Freshness;
use crate::view::{IrAxes, IrExtent, IrMode, PlotChrome, ViewState};

#[derive(Clone, Debug, PartialEq)]
pub struct IrScene {
    pub scene: Scene,
    pub plot: Rect,
    pub x_axis: Axis,
    pub y_axis: Axis,
    pub title: String,
    /// `t = 0 at inserted delay 12.34 ms · 4.24 m @ 20 °C`.
    pub origin: String,
    /// Why nothing is drawn, when that is the case.
    pub note: Option<String>,
    /// The curve's freshness as its transfer curve's legend says it (`stopped`,
    /// `STALE 3.2 s`, `audio stopped`), drawn after the origin.
    pub tag: Option<String>,
    /// Banner strip above the plot; zero height when no banner is up.
    pub strip: Rect,
    pub banners: Vec<BannerRow>,
    /// Room parameters drawn under the plot (a sweep trace's IR), if any.
    pub room: Option<crate::room::RoomTable>,
    /// The cursor's reading, when the cursor is on.
    pub cursor: Option<IrCursor>,
}

/// What the IR cursor reads: the sample nearest its time, as the view draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct IrCursor {
    /// The sample's time, ms re t = 0.
    pub t_ms: f64,
    /// `1.25 ms`.
    pub time: String,
    /// `+0.500 FS` (linear), `−12.3 dB` (log, ETC).
    pub value: String,
}

impl IrCursor {
    /// The readout line: `1.25 ms · +0.500 FS`.
    pub fn text(&self) -> String {
        format!("{} · {}", self.time, self.value)
    }
}

/// Time of every point, ms re the inserted delay.
pub fn times_ms(frame: &IrFrame) -> Vec<f64> {
    let (t0, dt) = (frame.meta.t0.0, frame.meta.dt.0);
    (0..frame.linear.len())
        .map(|i| (t0 + i as f64 * dt) * 1000.0)
        .collect()
}

/// Where `frame` lies in time: the extent its time axis is navigated within.
pub fn extent(frame: &IrFrame) -> IrExtent {
    IrExtent::of(frame.meta.t0.0, frame.meta.dt.0, frame.linear.len())
}

/// The value axis's title in `mode`.
pub fn title(mode: IrMode) -> &'static str {
    match mode {
        IrMode::Linear => "Amplitude (FS)",
        IrMode::Log => "dB re peak",
        IrMode::Etc => "ETC dB re peak",
    }
}

fn finite_max(v: impl Iterator<Item = f64>) -> f64 {
    v.filter(|x| x.is_finite())
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Values drawn for `mode`: amplitude (FS) or dB re the peak; `None` when the mode has no
/// data. A zero sample has no level: NaN in the log view.
pub fn ir_values(frame: &IrFrame, mode: IrMode) -> Option<Vec<f64>> {
    match mode {
        IrMode::Linear => Some(frame.linear.iter().map(|x| f64::from(*x)).collect()),
        IrMode::Log => {
            let peak = finite_max(frame.linear.iter().map(|x| f64::from(x.abs())));
            if peak.is_nan() || peak <= 0.0 {
                return None;
            }
            let v = frame
                .linear
                .iter()
                .map(|x| {
                    let a = f64::from(x.abs());
                    if a > 0.0 && a.is_finite() {
                        20.0 * (a / peak).log10()
                    } else {
                        f64::NAN
                    }
                })
                .collect();
            Some(v)
        }
        IrMode::Etc => {
            let etc = frame.etc.as_ref()?;
            let peak = finite_max(etc.iter().map(|x| f64::from(*x)));
            if !peak.is_finite() {
                return None;
            }
            Some(etc.iter().map(|x| f64::from(*x) - peak).collect())
        }
    }
}

/// The linear view's amplitude range when none was chosen: symmetric, 10 % over the peak,
/// so both polarities of the arrival show at the same scale.
pub fn auto_amplitude(frame: &IrFrame) -> Range {
    let peak = finite_max(frame.linear.iter().map(|x| f64::from(x.abs())));
    let lim = if peak > 0.0 { peak * 1.1 } else { 1.0 };
    Range::new(-lim, lim)
}

/// The value axis's range in `mode` with `axes`.
pub fn y_range(frame: &IrFrame, mode: IrMode, axes: &IrAxes) -> Range {
    match mode {
        IrMode::Linear => axes
            .amplitude
            .filter(Range::is_valid)
            .unwrap_or_else(|| auto_amplitude(frame)),
        IrMode::Log | IrMode::Etc => axes.level_db,
    }
}

/// The time axis's range with `axes`: the chosen one, else the whole IR.
pub fn time_range(frame: &IrFrame, axes: &IrAxes) -> Range {
    axes.time_ms
        .filter(Range::is_valid)
        .unwrap_or(extent(frame).full)
}

/// The cursor's reading at `axes.cursor_ms`, at the nearest sample of the values `mode`
/// draws.
pub fn cursor_reading(frame: &IrFrame, mode: IrMode, axes: &IrAxes) -> Option<IrCursor> {
    let t = axes.cursor_ms?;
    let e = extent(frame);
    let i = e.nearest(t)?;
    let t_ms = e.time_of(i);
    let v = ir_values(frame, mode)
        .and_then(|v| v.get(i).copied())
        .unwrap_or(f64::NAN);
    let value = match mode {
        IrMode::Linear => format::amplitude_readout(v),
        IrMode::Log | IrMode::Etc => format::db_readout(v),
    };
    Some(IrCursor {
        t_ms,
        time: format::ir_time(t_ms, e.dt_ms),
        value,
    })
}

/// The IR `frame` in the view's mode on `axes` (the IR pane's or the sweep view's).
#[allow(clippy::too_many_arguments)]
pub fn ir_scene(
    frame: &IrFrame,
    color: Color,
    freshness: Option<Freshness>,
    status: &Status,
    view: &ViewState,
    axes: &IrAxes,
    chrome: PlotChrome,
    theme: &Theme,
    size: Viewport,
) -> IrScene {
    let mode = view.ir.mode;
    let mut c = Canvas::new(size, theme);
    let plot_w = (size.width - MARGINS.left - MARGINS.right).max(1.0);
    let strip = canvas::banner_strip(&mut c, status, MARGINS.left, plot_w, size, theme);
    let plot = canvas::plot_area(size, strip.rect.bottom(), MARGINS.right);
    let t = times_ms(frame);
    let trange = time_range(frame, axes);
    let x_axis = axis::linear_axis(trange, plot.x, plot.right(), "ms");
    let values = ir_values(frame, mode);
    let yrange = y_range(frame, mode, axes);
    let title = title(mode).to_string();
    let y_unit = if mode == IrMode::Linear { "FS" } else { "dB" };
    let y_axis = axis::linear_axis(yrange, plot.bottom(), plot.y, y_unit);
    canvas::pane_frame(&mut c, plot, &x_axis, &y_axis, true, &title, chrome, theme);
    let (xm, ym) = (x_axis.mapping, y_axis.mapping);
    if mode == IrMode::Linear {
        canvas::hline(&mut c, plot, ym.to_px(0.0), theme.zero_line);
    }
    // t = 0: where the inserted delay puts the arrival.
    canvas::vline(&mut c.base, plot, xm.to_px(0.0), theme.zero_line);
    let origin = format!(
        "t = 0 at inserted delay {}",
        readout::delay_readout(frame.meta.inserted_delay.0, view.temperature_c)
    );
    let tag = freshness.and_then(|f| f.tag());
    let origin_line = match &tag {
        Some(t) => format!("{origin} · {t}"),
        None => origin.clone(),
    };
    // Beside the title when both fit, else on the row below it.
    let side_by_side = 6.0
        + canvas::text_width(&title, theme.small_font_size)
        + 16.0
        + canvas::text_width(&origin_line, theme.small_font_size)
        + 6.0
        <= plot.w;
    let origin_y = if side_by_side {
        plot.y + 4.0
    } else {
        plot.y + 4.0 + 1.4 * theme.small_font_size
    };
    c.base.labels.push(label(
        origin_line,
        [plot.right() - 6.0, origin_y],
        anchor(HAlign::Right, VAlign::Top),
        theme.small_font_size,
        theme.text_dim,
    ));

    let note = match &values {
        None if mode == IrMode::Etc && frame.etc.is_none() => {
            Some("ETC not published for this measurement".to_string())
        }
        None => Some("no impulse energy".to_string()),
        Some(_) => None,
    };
    if let Some(v) = &values {
        let xs: Vec<f32> = t.iter().map(|x| xm.to_px(*x)).collect();
        // Log views: points below the axis sit on its floor instead of vanishing, so the
        // decay stays a connected line.
        let floor = yrange.lo;
        let ys: Vec<f32> = v
            .iter()
            .map(|y| {
                if mode != IrMode::Linear && y.is_nan() {
                    ym.to_px(floor)
                } else if mode != IrMode::Linear {
                    ym.to_px(y.max(floor))
                } else {
                    ym.to_px(*y)
                }
            })
            .collect();
        let (points, _) = gapped(&xs, &ys, None, |_, _| false);
        let dim = if freshness.is_some_and(|f| f.is_stale()) {
            theme.stale_alpha
        } else {
            1.0
        };
        c.data.polylines.push(Polyline {
            points,
            alpha: vec![],
            stroke: Stroke::solid(color.with_alpha(dim), (theme.trace_width * 0.75).max(1.0)),
            clip: Some(plot),
        });
    }
    let cursor = values
        .as_ref()
        .filter(|_| chrome.cursor())
        .and_then(|_| cursor_reading(frame, mode, axes));
    if let Some(cur) = &cursor {
        canvas::vline(&mut c.overlay, plot, xm.to_px(cur.t_ms), theme.cursor);
        // Under the origin line, right-aligned with it.
        c.overlay.labels.push(label(
            cur.text(),
            [plot.right() - 6.0, origin_y + 1.4 * theme.small_font_size],
            anchor(HAlign::Right, VAlign::Top),
            theme.small_font_size,
            theme.text,
        ));
    }
    if let Some(n) = &note {
        c.overlay.labels.push(label(
            n.clone(),
            [plot.x + plot.w / 2.0, plot.y + plot.h / 2.0],
            anchor(HAlign::Center, VAlign::Center),
            theme.font_size,
            theme.text_dim,
        ));
    }
    IrScene {
        scene: c.into_scene(size),
        plot,
        x_axis,
        y_axis,
        title,
        origin,
        note,
        tag,
        strip: strip.rect,
        banners: strip.rows,
        room: None,
        cursor,
    }
}

/// `note` as it fits `width`: one line at the pane's font, else at the small font, else
/// broken after its name (`: ` or ` — `) into two lines, each cut with `…` if need be.
pub fn note_lines(note: &str, width: f32, theme: &Theme) -> (Vec<String>, f32) {
    let fits = |t: &str, size: f32| canvas::text_width(t, size) <= width;
    if fits(note, theme.font_size) {
        return (vec![note.to_string()], theme.font_size);
    }
    let size = theme.small_font_size;
    if fits(note, size) {
        return (vec![note.to_string()], size);
    }
    let cut = |t: &str| {
        if fits(t, size) {
            return t.to_string();
        }
        let mut out = t.to_string();
        while !out.is_empty() && !fits(&format!("{out}…"), size) {
            out.pop();
        }
        format!("{}…", out.trim_end())
    };
    let split = note
        .split_once(" — ")
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .or_else(|| {
            note.split_once(": ")
                .map(|(a, b)| (format!("{a}:"), b.to_string()))
        });
    match split {
        Some((a, b)) => (vec![cut(&a), cut(&b)], size),
        None => (vec![cut(note)], size),
    }
}

/// Why the IR pane has no picture of its transfer measurement's impulse response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IrMissing {
    /// The measurement is stopped (and never made an IR frame).
    Stopped,
    /// The session's audio stopped.
    AudioStopped,
    /// Its transfer stream says the reference carries nothing, with this app's stimulus
    /// doing this.
    NoReference(crate::stimulus::Drive),
    /// Its transfer stream says the measurement input is below its floor.
    NoSignal,
    /// Running, nothing wrong known: the first frame is on its way.
    NotYet,
    /// Its curves are hidden in this app (it keeps measuring).
    Hidden,
}

/// The empty IR pane's text for measurement `name`; `key` brings the IR back: the key that
/// starts a stopped measurement, or shows a hidden one.
pub fn missing_text(name: &str, why: IrMissing, key: &str) -> String {
    match why {
        IrMissing::Stopped => format!("{name} stopped — {key} starts it"),
        IrMissing::Hidden => format!("{name} hidden — {key} shows it"),
        IrMissing::AudioStopped => format!("{name}: the audio stopped — the IR returns with it"),
        IrMissing::NoReference(d) => crate::stimulus::no_reference_note(&d),
        IrMissing::NoSignal => "no signal: the measurement input is below its floor".into(),
        IrMissing::NotYet => format!("{name}: no IR frame yet"),
    }
}

/// The IR pane without an IR to draw: the banners of its transfer measurement and an empty
/// plot that says why (`note`).
pub fn missing_scene(
    note: String,
    status: &Status,
    axes: &IrAxes,
    chrome: PlotChrome,
    theme: &Theme,
    size: Viewport,
) -> IrScene {
    let mut c = Canvas::new(size, theme);
    let plot_w = (size.width - MARGINS.left - MARGINS.right).max(1.0);
    let strip = canvas::banner_strip(&mut c, status, MARGINS.left, plot_w, size, theme);
    let plot = canvas::plot_area(size, strip.rect.bottom(), MARGINS.right);
    let trange = axes
        .time_ms
        .filter(Range::is_valid)
        .unwrap_or(Range::new(-1.0, 10.0));
    let x_axis = axis::linear_axis(trange, plot.x, plot.right(), "ms");
    let y_axis = axis::linear_axis(Range::new(-1.0, 1.0), plot.bottom(), plot.y, "FS");
    canvas::pane_frame(&mut c, plot, &x_axis, &y_axis, true, "", chrome, theme);
    let (lines, font) = note_lines(&note, plot.w - 12.0, theme);
    let pitch = font * 1.4;
    let top = plot.y + plot.h / 2.0 - pitch * (lines.len() as f32 - 1.0) / 2.0;
    for (i, line) in lines.into_iter().enumerate() {
        c.overlay.labels.push(crate::primitives::Label {
            clip: Some(plot),
            ..label(
                line,
                [plot.x + plot.w / 2.0, top + pitch * i as f32],
                anchor(HAlign::Center, VAlign::Center),
                font,
                theme.text_dim,
            )
        });
    }
    IrScene {
        scene: c.into_scene(size),
        plot,
        x_axis,
        y_axis,
        title: String::new(),
        origin: String::new(),
        note: Some(note),
        tag: None,
        strip: strip.rect,
        banners: strip.rows,
        room: None,
        cursor: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stimulus::Drive;
    use crate::view::IrView;
    use ac2_proto::frame::IrMeta;
    use ac2_proto::units::{Hz, MeasId, Seconds};

    const SIZE: Viewport = Viewport {
        width: 800.0,
        height: 300.0,
    };

    fn frame() -> IrFrame {
        // 48 kHz, 21 points from −0.5 ms; impulse 0.5 at t = 0, a −0.25 reflection at 2 ms.
        let mut linear = vec![0.0f32; 21];
        linear[5] = 0.5;
        linear[9] = 0.05;
        linear[20] = -0.25;
        IrFrame {
            meas: MeasId(1),
            meta: IrMeta {
                sample_rate: Hz(48_000.0),
                t0: Seconds(-0.0005),
                dt: Seconds(0.0001),
                inserted_delay: Seconds(0.012_34),
            },
            linear,
            etc: None,
        }
    }

    fn view(mode: IrMode) -> ViewState {
        ViewState {
            ir: IrView {
                mode,
                ..IrView::default()
            },
            ..ViewState::default()
        }
    }

    #[test]
    fn time_axis_origin_is_the_inserted_delay() {
        let f = frame();
        let t = times_ms(&f);
        assert!((t[5]).abs() < 1e-12);
        assert!((t[20] - 1.5).abs() < 1e-9);
        let s = ir_scene(
            &f,
            Color::WHITE,
            None,
            &Status::default(),
            &view(IrMode::Linear),
            &IrAxes::default(),
            PlotChrome::Full,
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(
            s.origin,
            "t = 0 at inserted delay 12.34 ms · 4.24 m @ 20 °C"
        );
        assert_eq!(s.x_axis.title, "ms");
        assert!(
            s.x_axis.labels().contains(&"0.0"),
            "{:?}",
            s.x_axis.labels()
        );
        // Linear: symmetric, 10 % headroom over the peak.
        assert!((s.y_axis.mapping.range.hi - 0.55).abs() < 1e-6);
        assert!((s.y_axis.mapping.range.lo + 0.55).abs() < 1e-6);
        assert_eq!(s.title, "Amplitude (FS)");
        // The impulse point is drawn at t = 0.
        let line = &s.scene.layers[1].polylines[0];
        assert!((line.points[5][0] - s.x_axis.mapping.to_px(0.0)).abs() < 1e-3);
        assert!(s.note.is_none());
    }

    #[test]
    fn log_view_is_re_peak() {
        let f = frame();
        let v = ir_values(&f, IrMode::Log).expect("values");
        assert_eq!(title(IrMode::Log), "dB re peak");
        assert!((v[5] - 0.0).abs() < 1e-9);
        assert!((v[9] + 20.0).abs() < 1e-6);
        assert!((v[20] + 6.0206).abs() < 1e-3);
        assert!(v[0].is_nan());
        assert_eq!(
            y_range(&f, IrMode::Log, &IrAxes::default()),
            Range::new(-60.0, 3.0)
        );
        // Silent samples sit on the floor in the drawing.
        let s = ir_scene(
            &f,
            Color::WHITE,
            None,
            &Status::default(),
            &view(IrMode::Log),
            &IrAxes::default(),
            PlotChrome::Full,
            &Theme::dark(),
            SIZE,
        );
        let line = &s.scene.layers[1].polylines[0];
        assert_eq!(line.points.len(), 21);
        assert!((line.points[0][1] - s.plot.bottom()).abs() < 1e-3);
    }

    #[test]
    fn etc_missing_or_present() {
        let mut f = frame();
        let s = ir_scene(
            &f,
            Color::WHITE,
            None,
            &Status::default(),
            &view(IrMode::Etc),
            &IrAxes::default(),
            PlotChrome::Full,
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(
            s.note.as_deref(),
            Some("ETC not published for this measurement")
        );
        assert!(s.scene.layers[1].polylines.is_empty());
        f.etc = Some((0..21).map(|i| -10.0 - i as f32).collect());
        let v = ir_values(&f, IrMode::Etc).expect("etc");
        assert_eq!(v[0], 0.0);
        assert_eq!(v[20], -20.0);
    }

    #[test]
    fn zoomed_time_range_and_stale() {
        let f = frame();
        let mut v = view(IrMode::Linear);
        v.ir.axes.time_ms = Some(Range::new(-0.2, 0.2));
        let s = ir_scene(
            &f,
            Color::WHITE,
            Some(Freshness::from_age(2.0)),
            &Status::default(),
            &v,
            &v.ir.axes,
            PlotChrome::Full,
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(s.x_axis.mapping.range, Range::new(-0.2, 0.2));
        assert_eq!(
            s.x_axis.labels(),
            [
                "−0.20", "−0.15", "−0.10", "−0.05", "0.00", "0.05", "0.10", "0.15", "0.20"
            ]
        );
        let stroke = s.scene.layers[1].polylines[0].stroke;
        assert!((stroke.color.a - Theme::dark().stale_alpha).abs() < 1e-6);
    }

    #[test]
    fn banners_sit_above_the_plot() {
        let f = frame();
        let calm = ir_scene(
            &f,
            Color::WHITE,
            None,
            &Status::default(),
            &view(IrMode::Log),
            &IrAxes::default(),
            PlotChrome::Full,
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(calm.strip.h, 0.0);
        assert_eq!(calm.plot.y, MARGINS.top);
        let s = ir_scene(
            &f,
            Color::WHITE,
            None,
            &crate::banner::tests::everything(),
            &view(IrMode::Log),
            &IrAxes::default(),
            PlotChrome::Full,
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(s.plot.y, s.strip.bottom() + MARGINS.top);
        assert_eq!(s.plot.bottom(), calm.plot.bottom());
        crate::canvas::tests::assert_banners_clear(&s.scene, &s.banners, &[s.plot]);
    }

    #[test]
    fn narrow_plot_puts_the_origin_under_the_title() {
        use crate::canvas::tests::{intersects, label_box};
        let f = frame();
        let build = |w: f32| {
            let size = Viewport {
                width: w,
                height: 300.0,
            };
            ir_scene(
                &f,
                Color::WHITE,
                None,
                &Status::default(),
                &view(IrMode::Linear),
                &IrAxes::default(),
                PlotChrome::Full,
                &Theme::dark(),
                size,
            )
        };
        let find = |s: &IrScene, t: &str| {
            s.scene.layers[0]
                .labels
                .iter()
                .find(|l| l.text == t)
                .cloned()
                .expect(t)
        };
        let wide = build(800.0);
        assert_eq!(
            find(&wide, &wide.origin).pos[1],
            find(&wide, &wide.title).pos[1]
        );
        let narrow = build(330.0);
        let (o, t) = (find(&narrow, &narrow.origin), find(&narrow, &narrow.title));
        assert!(o.pos[1] > t.pos[1]);
        assert!(!intersects(label_box(&o), label_box(&t)));
    }

    /// The empty IR pane names why, in the operator's terms, under its banners.
    #[test]
    fn missing_ir_says_why() {
        let t = |w| missing_text("TF 1", w, "S");
        assert_eq!(t(IrMissing::Stopped), "TF 1 stopped — S starts it");
        assert_eq!(
            t(IrMissing::AudioStopped),
            "TF 1: the audio stopped — the IR returns with it"
        );
        assert_eq!(
            t(IrMissing::NoReference(Drive::Playing)),
            "no reference: nothing is driving the loopback"
        );
        assert_eq!(
            t(IrMissing::NoReference(Drive::Armed {
                fire: "Enter".into()
            })),
            "no reference: armed — Enter starts the stimulus"
        );
        assert_eq!(
            t(IrMissing::NoSignal),
            "no signal: the measurement input is below its floor"
        );
        assert_eq!(t(IrMissing::NotYet), "TF 1: no IR frame yet");
        assert_eq!(
            missing_text("TF 1", IrMissing::Hidden, "A"),
            "TF 1 hidden — A shows it"
        );
        let status = Status {
            protection: ac2_proto::frame::ProtectionFlags::NO_REFERENCE,
            ..Status::default()
        };
        let s = missing_scene(
            t(IrMissing::NoReference(Drive::Playing)),
            &status,
            &IrAxes::default(),
            PlotChrome::Full,
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(s.banners[0].text, "NO REFERENCE");
        assert!(s.strip.bottom() < s.plot.y);
        let labels: Vec<&str> = s
            .scene
            .layers
            .iter()
            .flat_map(|l| l.labels.iter().map(|x| x.text.as_str()))
            .collect();
        assert!(labels.contains(&"no reference: nothing is driving the loopback"));
    }

    /// A long reason in a narrow pane breaks after the measurement's name and never leaves
    /// the plot.
    #[test]
    fn a_long_reason_fits_a_narrow_pane() {
        let theme = Theme::dark();
        let note = missing_text("Reference → M30 FOH", IrMissing::NotYet, "S");
        let (wide, size) = note_lines(&note, 700.0, &theme);
        assert_eq!((wide, size), (vec![note.clone()], theme.font_size));
        let (narrow, size) = note_lines(&note, 190.0, &theme);
        assert_eq!(narrow, ["Reference → M30 FOH:", "no IR frame yet"]);
        assert_eq!(size, theme.small_font_size);
        let stopped = missing_text("Reference → M30 FOH", IrMissing::Stopped, "S");
        let (lines, _) = note_lines(&stopped, 190.0, &theme);
        assert_eq!(lines, ["Reference → M30 FOH stopped", "S starts it"]);
        for w in [60.0, 120.0, 190.0] {
            let (lines, size) = note_lines(&stopped, w, &theme);
            for l in lines {
                assert!(crate::canvas::text_width(&l, size) <= w, "{w}: {l}");
            }
        }
    }

    /// A kept IR is dimmed and tagged as its transfer curve: stopped is a final result (not
    /// dimmed), audio stopped and STALE are not current (dimmed).
    #[test]
    fn a_kept_ir_is_tagged_like_its_curve() {
        let f = frame();
        let theme = Theme::dark();
        let draw = |fr: Freshness| {
            ir_scene(
                &f,
                Color::WHITE,
                Some(fr),
                &Status::default(),
                &view(IrMode::Linear),
                &IrAxes::default(),
                PlotChrome::Full,
                &theme,
                SIZE,
            )
        };
        let stopped = draw(Freshness::Stopped { age_s: 30.0 });
        assert_eq!(stopped.tag.as_deref(), Some("stopped"));
        let alpha = |s: &IrScene| s.scene.layers[1].polylines[0].stroke.color.a;
        assert_eq!(alpha(&stopped), Color::WHITE.a);
        let out = draw(Freshness::AudioStopped { age_s: 30.0 });
        assert_eq!(out.tag.as_deref(), Some("audio stopped"));
        assert!(alpha(&out) < Color::WHITE.a);
        assert_eq!(draw(Freshness::Fresh { age_s: 0.1 }).tag, None);
    }

    fn labels(s: &IrScene) -> Vec<String> {
        s.scene
            .layers
            .iter()
            .flat_map(|l| l.labels.iter().map(|x| x.text.clone()))
            .collect()
    }

    /// The cursor reads the sample nearest its time as each view draws it, on the line under
    /// the origin, with a line at that sample's time.
    #[test]
    fn the_cursor_reads_time_and_value_in_every_mode() {
        let mut f = frame();
        f.etc = Some((0..21).map(|i| -10.0 - i as f32).collect());
        let axes = IrAxes {
            // Between samples 8 and 9: snaps to 9, 0.4 ms.
            cursor_ms: Some(0.37),
            ..IrAxes::default()
        };
        let draw = |mode| {
            ir_scene(
                &f,
                Color::WHITE,
                None,
                &Status::default(),
                &view(mode),
                &axes,
                PlotChrome::Full,
                &Theme::dark(),
                SIZE,
            )
        };
        let lin = draw(IrMode::Linear);
        let cur = lin.cursor.clone().expect("cursor");
        assert!((cur.t_ms - 0.4).abs() < 1e-9);
        assert_eq!(cur.text(), "0.4 ms · +0.0500 FS");
        assert!(labels(&lin).contains(&cur.text()));
        let x = lin.x_axis.mapping.to_px(0.4);
        assert!(
            lin.scene.layers[2]
                .polylines
                .iter()
                .any(|p| (p.points[0][0] - x).abs() < 1e-3),
            "cursor line at 0.4 ms"
        );
        assert_eq!(
            draw(IrMode::Log).cursor.expect("log").text(),
            "0.4 ms · −20.0 dB"
        );
        // ETC: sample 9 is −19 dB, 9 under the peak at sample 0.
        assert_eq!(
            draw(IrMode::Etc).cursor.expect("etc").text(),
            "0.4 ms · −9.0 dB"
        );
        // A silent sample in the log view has no level; beyond the IR the cursor stays on
        // its last sample.
        let at = |t: f64, mode| {
            let a = IrAxes {
                cursor_ms: Some(t),
                ..IrAxes::default()
            };
            cursor_reading(&f, mode, &a).expect("reading").text()
        };
        assert_eq!(at(-0.5, IrMode::Log), "−0.5 ms · —");
        assert_eq!(at(9.0, IrMode::Linear), "1.5 ms · −0.250 FS");
        // Off: no reading, no line.
        let off = ir_scene(
            &f,
            Color::WHITE,
            None,
            &Status::default(),
            &view(IrMode::Linear),
            &IrAxes::default(),
            PlotChrome::Full,
            &Theme::dark(),
            SIZE,
        );
        assert_eq!(off.cursor, None);
        assert!(off.scene.layers[2].polylines.is_empty());
    }

    /// The axes as navigated: a chosen amplitude range (linear) or dB range (log / ETC) and
    /// time range are drawn and labelled as chosen; the defaults frame the whole IR.
    #[test]
    fn navigated_axes_are_drawn_as_chosen() {
        let f = frame();
        let axes = IrAxes {
            time_ms: Some(Range::new(0.0, 1.0)),
            amplitude: Some(Range::new(-0.1, 0.1)),
            level_db: Range::new(-30.0, 0.0),
            cursor_ms: None,
        };
        let draw = |mode| {
            ir_scene(
                &f,
                Color::WHITE,
                None,
                &Status::default(),
                &view(mode),
                &axes,
                PlotChrome::Full,
                &Theme::dark(),
                SIZE,
            )
        };
        let lin = draw(IrMode::Linear);
        assert_eq!(lin.x_axis.mapping.range, Range::new(0.0, 1.0));
        assert_eq!(lin.y_axis.mapping.range, Range::new(-0.1, 0.1));
        assert_eq!(
            lin.y_axis.labels(),
            [
                "−0.10", "−0.08", "−0.06", "−0.04", "−0.02", "0.00", "0.02", "0.04", "0.06",
                "0.08", "0.10"
            ]
        );
        assert_eq!(lin.x_axis.labels().first(), Some(&"0.0"));
        let log = draw(IrMode::Log);
        assert_eq!(log.y_axis.mapping.range, Range::new(-30.0, 0.0));
        assert!(
            log.y_axis.labels().contains(&"−30"),
            "{:?}",
            log.y_axis.labels()
        );
        assert_eq!(
            time_range(&f, &IrAxes::default()),
            Range::new(-0.5, 1.5),
            "the whole IR"
        );
        assert_eq!(
            y_range(&f, IrMode::Linear, &IrAxes::default()),
            auto_amplitude(&f)
        );
    }
}

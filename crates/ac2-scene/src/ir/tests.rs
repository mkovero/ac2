use super::*;
use crate::stimulus::Drive;
use crate::view::IrView;
use ac2_proto::frame::IrMeta;
use ac2_proto::units::{Hz, MeasId, Seconds};

const SIZE: Viewport = Viewport {
    width: 800.0,
    height: 300.0,
};

#[test]
fn a_dense_line_keeps_each_pixel_columns_extremes_in_order() {
    // Ten samples per pixel: column 0 dips to −3 then peaks at 5, column 1 is flat 1.
    let xs: Vec<f32> = (0..20).map(|i| i as f32 / 10.0).collect();
    let mut ys = vec![1.0f32; 20];
    ys[2] = -3.0;
    ys[7] = 5.0;
    let (x, y) = per_pixel(&xs, &ys);
    assert_eq!(y, vec![-3.0, 5.0, 1.0]);
    assert_eq!(x, vec![0.2, 0.7, 1.0]);
    // A gap stays a gap; sparse points stay as they are.
    let (x, y) = per_pixel(&[0.0, 1.5, 3.0, 4.0], &[1.0, f32::NAN, 2.0, 3.0]);
    assert_eq!(x, vec![0.0, 1.5, 3.0, 4.0]);
    assert!(y[1].is_nan() && y[3] == 3.0);
}

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
            "−0.10", "−0.08", "−0.06", "−0.04", "−0.02", "0.00", "0.02", "0.04", "0.06", "0.08",
            "0.10"
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

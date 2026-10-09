//! Reducer tests of the impulse-response pictures' navigation: the IR pane and the sweep
//! pane's IR view take the frequency panes' keys and mouse gestures on their time and value
//! axes, each keeping its own.

use super::*;
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;
use ac2_scene::view::{IrAxes, IrPane};

const SIZE: Viewport = Viewport {
    width: 900.0,
    height: 500.0,
};

fn now() -> crate::scenes::Now {
    crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    }
}

/// 21 points 0.1 ms apart from −0.5 ms: the arrival (0.5) at 0, a reflection (−0.25) at
/// 1.5 ms, a little energy (0.05) at 0.4 ms.
pub(super) fn ir_event(meas: u32) -> ConnEvent {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::frame::{IrFrame, IrMeta};
    use ac2_proto::{Frame, FrameData};
    let mut linear = vec![0.0f32; 21];
    linear[5] = 0.5;
    linear[9] = 0.05;
    linear[20] = -0.25;
    let frame = Frame {
        stamp: ac2_proto::samples::stamp(None),
        data: FrameData::Ir(IrFrame {
            meas: MeasId(meas),
            meta: IrMeta {
                sample_rate: Hz(48_000.0),
                t0: Seconds(-0.0005),
                dt: Seconds(0.0001),
                inserted_delay: Seconds(0.0125),
            },
            linear,
            etc: None,
        }),
    };
    let f = TopicFrame {
        topic: frame.data.topic(),
        frame: Arc::new(frame),
        received: Instant::now(),
        since_new: std::time::Duration::ZERO,
        age: Some(0.0),
        stale: false,
    };
    let mut latest = Latest::default();
    latest.frames.insert(f.topic.to_string().into(), f);
    ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids: std::collections::BTreeMap::new(),
        drained: Instant::now(),
    }))
}

fn close(a: Range, lo: f64, hi: f64) -> bool {
    (a.lo - lo).abs() < 1e-9 && (a.hi - hi).abs() < 1e-9
}

fn axes(t: &T, p: IrPane) -> IrAxes {
    *t.st.view.ir_axes(p)
}

fn nav(t: &mut T, p: IrPane, m: IrNavMsg) {
    t.st.update(Msg::IrNav(p, m), &t.keys);
}

/// The IR pane: I / O and ←/→ move time, the wheel zooms about the pointer, a drag pans,
/// limits hold; Ctrl+I/O, Ctrl+↑/↓ and Ctrl / Shift+wheel move the amplitude or dB axis;
/// Home, Shift+Home and Ctrl+Home reset, fit and default. The frequency axis stays.
#[test]
fn the_ir_pane_zooms_and_pans_time_and_value() {
    let mut t = T::new();
    t.conn(ir_event(1));
    t.key("Alt+3");
    let live = |t: &T| axes(t, IrPane::Live);
    let freq = t.st.nav.target;
    // I: 1.5× about the middle of the whole IR (−0.5 … 1.5 ms).
    t.key("I");
    let r = live(&t).time_ms.expect("zoomed");
    assert!(close(r, 0.5 - 2.0 / 3.0, 0.5 + 2.0 / 3.0), "{r:?}");
    t.key("Home");
    assert_eq!(live(&t).time_ms, None);
    // →: a tenth of the span, on a round step.
    t.key("Right");
    assert!(close(live(&t).time_ms.expect("panned"), -0.3, 1.7));
    t.key("Home");
    // The wheel about t = 0, a quarter of the way along: it stays there.
    nav(
        &mut t,
        IrPane::Live,
        IrNavMsg::Zoom {
            about_ms: 0.0,
            factor: 2.0,
        },
    );
    assert!(close(live(&t).time_ms.expect("wheel"), -0.25, 0.75));
    // A drag far right stops one IR length past its end.
    nav(&mut t, IrPane::Live, IrNavMsg::Pan { ms: 1e6 });
    assert!(close(live(&t).time_ms.expect("drag"), 2.5, 3.5));
    // Never narrower than four samples.
    nav(
        &mut t,
        IrPane::Live,
        IrNavMsg::Zoom {
            about_ms: 3.0,
            factor: 1e9,
        },
    );
    assert!((live(&t).time_ms.expect("narrow").span() - 0.4).abs() < 1e-9);
    assert_eq!(t.st.nav.target, freq, "frequency untouched");

    // Linear: Ctrl+I from the automatic ±110 % of the peak, Ctrl+↑ by a round step.
    t.key("Ctrl+I");
    let a = live(&t).amplitude.expect("amplitude");
    assert!(close(a, -0.55 / 1.5, 0.55 / 1.5), "{a:?}");
    t.key("Ctrl+Up");
    assert!(close(
        live(&t).amplitude.expect("up"),
        -0.55 / 1.5 + 0.1,
        0.55 / 1.5 + 0.1
    ));
    // Shift+Home: the whole IR, the amplitude automatic again.
    t.key("Shift+Home");
    assert_eq!((live(&t).time_ms, live(&t).amplitude), (None, None));

    // Log: the dB axis. Ctrl+wheel about −30 dB, Shift+wheel pans.
    t.key("Shift+G");
    assert_eq!(t.st.layout.focused().modes.ir, IrMode::Log);
    nav(
        &mut t,
        IrPane::Live,
        IrNavMsg::ValueZoom {
            about: Some(-30.0),
            factor: 2.0,
        },
    );
    let d = live(&t).level_db;
    assert!((d.span() - 31.5).abs() < 1e-9, "{d:?}");
    nav(&mut t, IrPane::Live, IrNavMsg::ValuePan { by: -10.0 });
    assert!((live(&t).level_db.lo - (d.lo - 10.0)).abs() < 1e-9);
    // Shift+Home frames the decay (−20 dB, −6 dB and the peak) and says so.
    t.key("Shift+Home");
    let fit = live(&t).level_db;
    assert!(
        fit.lo <= -20.0 && fit.hi >= 0.0 && fit.span() < 40.0,
        "{fit:?}"
    );
    assert!(
        t.last_toast().starts_with("Impulse response: level"),
        "{}",
        t.last_toast()
    );
    // Ctrl+Home: the defaults.
    t.key("I");
    t.key("Ctrl+Home");
    assert_eq!(live(&t), IrAxes::default());
    // The sweep view's axes never moved.
    assert_eq!(axes(&t, IrPane::Sweep), IrAxes::default());
}

/// C puts the cursor in the middle of the shown time, Shift+←/→ step it by samples, a
/// click places it; the readout is the scene's.
#[test]
fn the_ir_cursor_reads_time_and_value() {
    let mut t = T::new();
    t.conn(ir_event(1));
    t.key("Alt+3");
    let cursor = |t: &T| axes(t, IrPane::Live).cursor_ms;
    let reading = |t: &T| {
        crate::scenes::ir(&t.st, t.ir_pane(), &t.keys, &Theme::dark(), SIZE, now())
            .expect("scene")
            .cursor
            .map(|c| c.text())
    };
    t.st.update(Msg::Command(CommandId::ToggleCursor), &t.keys);
    assert_eq!(cursor(&t), Some(0.5));
    assert_eq!(reading(&t).as_deref(), Some("0.5 ms · 0 FS"));
    t.key("Shift+Left");
    assert!((cursor(&t).expect("on") - 0.4).abs() < 1e-9);
    assert_eq!(reading(&t).as_deref(), Some("0.4 ms · +0.0500 FS"));
    // A click: on the nearest sample.
    nav(&mut t, IrPane::Live, IrNavMsg::Cursor { t_ms: 1.47 });
    assert_eq!(reading(&t).as_deref(), Some("1.5 ms · −0.250 FS"));
    t.key("Shift+G");
    assert_eq!(reading(&t).as_deref(), Some("1.5 ms · −6.0 dB"));
    // The frequency cursor is the transfer pane's own.
    assert_eq!(t.st.view.cursor_hz, None);
    t.st.update(Msg::Command(CommandId::ToggleCursor), &t.keys);
    assert_eq!(cursor(&t), None);
    assert_eq!(reading(&t), None);
}

fn sweep_with_ir(t: &mut T) {
    let (d, g) = sweep_data(14);
    let mut d = (*d).clone();
    if let Some(s) = d.sweep.as_mut() {
        s.ir.linear[100] = 1.0;
    }
    t.conn(with_traces(vec![sweep_meta(14)]));
    t.conn(ConnEvent::Trace(Arc::new(d), g));
}

/// The sweep pane's IR view (G) navigates its own time axis and cursor; the response view
/// keeps the frequency keys, the room view has no axes and leaves the IR's alone.
#[test]
fn the_sweep_ir_view_has_its_own_navigation() {
    let mut t = T::new();
    sweep_with_ir(&mut t);
    t.go(PaneKind::Distortion);
    assert_eq!(t.st.ir_target(), None, "response & distortion first");
    let freq = t.st.nav.target;
    t.key("G");
    assert_eq!(t.st.ir_target(), Some(IrPane::Sweep));
    // 200 samples 1 ms apart from −100 ms: −100 … 99 ms.
    t.key("I");
    let r = axes(&t, IrPane::Sweep).time_ms.expect("zoomed");
    assert!((r.span() - 199.0 / 1.5).abs() < 1e-9, "{r:?}");
    assert_eq!(t.st.nav.target, freq);
    assert_eq!(axes(&t, IrPane::Live), IrAxes::default());
    t.st.update(Msg::Command(CommandId::ToggleCursor), &t.keys);
    let s = crate::scenes::sweep(
        &t.st,
        t.pane(PaneKind::Distortion),
        &Theme::dark(),
        SIZE,
        now(),
    );
    let crate::scenes::SweepPane::Ir(ir) = s else {
        panic!("the IR view");
    };
    assert_eq!(ir.cursor.expect("cursor").text(), "0 ms · +1.00 FS");
    // The room view: no IR keys; the IR view's axes stay as they were.
    t.key("G");
    assert_eq!(t.st.ir_target(), None);
    let before = axes(&t, IrPane::Sweep);
    t.key("I");
    t.key("Shift+Right");
    assert_eq!(axes(&t, IrPane::Sweep), before);
}

/// The distortion view's cursor reads every order at the cursor in dB and in percent;
/// Ctrl+wheel on the percent axis zooms about the level the pointer's percent stands for.
#[test]
fn the_distortion_cursor_reads_in_db_and_percent() {
    let mut t = T::new();
    sweep_with_ir(&mut t);
    t.go(PaneKind::Distortion);
    t.st.update(Msg::CursorAt(Some(1000.0)), &t.keys);
    let rows = |t: &T| {
        let s = crate::scenes::sweep(
            &t.st,
            t.pane(PaneKind::Distortion),
            &Theme::dark(),
            SIZE,
            now(),
        );
        let crate::scenes::SweepPane::Distortion(d) = s else {
            panic!("the distortion view");
        };
        let cur = d.cursor.expect("cursor");
        (cur.freq, cur.rows)
    };
    let (f, r) = rows(&t);
    assert_eq!(f, "1.00 kHz");
    assert!(
        r.contains(&("H2".to_owned(), "−40.0 dB".to_owned())),
        "{r:?}"
    );
    t.key("U");
    let (_, r) = rows(&t);
    assert!(r.contains(&("H2".to_owned(), "1.00 %".to_owned())), "{r:?}");
    // 1 % under the pointer is −40 dB: zooming there keeps −40 dB where it was.
    let about = ac2_scene::distortion::db_of_percent(1.0);
    assert!((about + 40.0).abs() < 1e-9);
    let before = t.st.view.distortion.range_db;
    t.st.update(
        Msg::LevelZoom {
            pane: PaneKind::Distortion,
            about_db: Some(about),
            factor: 2.0,
        },
        &t.keys,
    );
    let r = t.st.view.distortion.range_db;
    let frac = |r: Range| (-40.0 - r.lo) / r.span();
    assert!((frac(r) - frac(before)).abs() < 1e-9 && (r.span() - before.span() / 2.0).abs() < 1e-9);
}

/// A transfer trace of Main L's group, stored with (or without) the IR it was captured with.
fn stored_tf(id: u32, name: &str, ir: Option<TransferIr>) -> (TraceMeta, Arc<TraceData>) {
    let mut m = stored(id, Some(1), 0);
    m.edit.owner = ac2_proto::model::TraceOwner::Meas { meas: MeasId(1) };
    m.edit.slot = (ir.is_some()).then_some(1);
    m.edit.name = name.into();
    m.delay = Seconds(0.0125);
    let n = 480;
    let d = TraceData {
        meta: m.clone(),
        mag_db: vec![0.0; n],
        phase_deg: Some(vec![0.0; n]),
        coherence: None,
        sweep: None,
        ir,
    };
    (m, Arc::new(d))
}

/// The selected stored transfer trace's IR replaces the live one in the transfer IR view:
/// its 64 samples in the trace's colour, tagged with the trace's name, time zero at its
/// delay, no live banners; Home and the cursor act on it. A trace captured without an IR, or none
/// selected, leaves the view on the live IR.
#[test]
fn the_transfer_ir_view_shows_the_selected_stored_ir() {
    let mut t = T::new();
    t.conn(ir_event(1));
    t.key("Alt+3");
    let mut linear = vec![0.0f32; 64];
    linear[40] = 0.75;
    let ir = TransferIr {
        sample_rate: Hz(48_000.0),
        ir: TraceIr {
            t0: Seconds(-32.0 / 48_000.0),
            dt: Seconds(1.0 / 48_000.0),
            linear,
            etc_db: vec![-60.0; 64],
        },
    };
    let (with, wd) = stored_tf(5, "Main L pre EQ", Some(ir));
    let (bare, bd) = stored_tf(6, "Main L old", None);
    t.conn(with_traces(vec![with, bare]));
    let grid = Arc::new(GridDef::Log {
        ppo: 48,
        k_min: -240,
        k_max: 239,
    });
    t.conn(ConnEvent::Trace(wd, grid.clone()));
    t.conn(ConnEvent::Trace(bd, grid));
    let theme = Theme::dark();
    let scene =
        |t: &T| crate::scenes::ir(&t.st, t.ir_pane(), &t.keys, &theme, SIZE, now()).expect("scene");
    // The curve: the longest polyline (axes and grid lines have two points).
    let curve = |s: &ac2_scene::ir::IrScene| {
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.polylines)
            .max_by_key(|p| p.points.len())
            .cloned()
            .expect("curve")
    };
    let live = scene(&t);
    assert_eq!(curve(&live).points.len(), 21);
    assert_eq!(live.tag, None);

    t.st.select_trace(Some(TraceId(5)));
    let s = scene(&t);
    assert_eq!(s.tag.as_deref(), Some("Main L pre EQ"));
    assert!(
        s.origin.starts_with("t = 0 at inserted delay 12.5"),
        "{}",
        s.origin
    );
    assert!(s.banners.is_empty());
    let c = curve(&s);
    assert_eq!(c.points.len(), 64);
    assert_eq!(
        c.stroke.color,
        t.st.curve_colours(&theme).trace(TraceId(5)),
        "the trace's colour, as its legend row"
    );
    let labels: Vec<String> = s
        .scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|x| x.text.clone()))
        .collect();
    assert!(
        labels.iter().any(|l| l.ends_with(" · Main L pre EQ")),
        "{labels:?}"
    );
    // Navigation and the cursor act on the stored IR: its extent.
    let e = t.st.ir_extent(IrPane::Live).expect("extent");
    assert!((e.t0_ms + 32.0 / 48.0).abs() < 1e-9 && e.n == 64, "{e:?}");
    t.st.update(Msg::Command(CommandId::ToggleCursor), &t.keys);
    // The cursor comes on at the stored IR's sample nearest its middle (the live one's is
    // 0.5 ms).
    let mid_ms = (-32.0 + 31.0) / 2.0 / 48.0;
    assert!((axes(&t, IrPane::Live).cursor_ms.expect("on") - mid_ms).abs() <= 0.5 / 48.0 + 1e-9);
    assert!(scene(&t).cursor.is_some());

    // A trace without an IR, or none selected: the live IR.
    t.st.select_trace(Some(TraceId(6)));
    assert_eq!(curve(&scene(&t)).points.len(), 21);
    t.st.select_trace(None);
    assert_eq!(curve(&scene(&t)).points.len(), 21);
    assert_eq!(scene(&t).tag, None);
}

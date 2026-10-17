//! Reducer tests of the display edits: offset steps, the level axis, hiding and deleting
//! the selected measurement or stored trace, and which pane a selection brings up.

use super::*;
use ac2_scene::primitives::Viewport;
use ac2_scene::theme::Theme;

/// A stored spectrum filed under Sub (2), the spectrum pane's measurement: a pane draws
/// its own group's traces only.
fn spec_meta(id: u32) -> TraceMeta {
    let mut t = TraceMeta {
        kind: TraceKind::Spectrum {
            scale: LevelScale::Dbfs,
            level: ac2_proto::model::SpecLevel::Tone,
        },
        ..stored(id, None, 2)
    };
    t.edit.owner = ac2_proto::model::TraceOwner::Meas { meas: MeasId(2) };
    t
}

fn with_offset(mut t: TraceMeta, db: f64) -> TraceMeta {
    t.edit.offset = Db(db);
    t
}

fn data(meta: &TraceMeta, mag: Vec<f32>) -> ConnEvent {
    let grid = GridDef::Log {
        ppo: 1,
        k_min: -5,
        k_max: -5 + mag.len() as i32 - 1,
    };
    ConnEvent::Trace(
        Arc::new(TraceData {
            meta: meta.clone(),
            mag_db: mag,
            phase_deg: None,
            coherence: None,
            sweep: None,
            ir: None,
        }),
        Arc::new(grid),
    )
}

fn now() -> crate::scenes::Now {
    crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    }
}

const SIZE: Viewport = Viewport {
    width: 900.0,
    height: 500.0,
};

#[test]
fn offset_steps_change_the_selected_trace() {
    let mut t = T::new();
    let mut locked = stored(12, None, 2);
    locked.edit.locked = true;
    t.conn(with_traces(vec![
        stored(13, Some(2), 2),
        locked,
        spec_meta(16),
    ]));
    t.st.update(Msg::SelectTrace(TraceId(13)), &t.keys);
    let (id, edit, what) = trace_update(&t.key("Alt+Shift+Up"));
    assert_eq!((id, edit.offset), (TraceId(13), Db(3.0)));
    assert_eq!(what, "slot 2 (t13): offset +3.0 dB");
    // Steps start from what the daemon holds.
    t.conn(with_traces(vec![with_offset(stored(13, Some(2), 2), 3.0)]));
    let (_, edit, what) = trace_update(&t.key("Alt+Down"));
    assert_eq!(edit.offset, Db(2.0));
    assert_eq!(what, "slot 2 (t13): offset +2.0 dB");
    let (_, edit, _) = trace_update(&t.key("Alt+Shift+Down"));
    assert_eq!(edit.offset, Db(0.0));
    let (_, edit, _) = trace_update(&t.key("Alt+Up"));
    assert_eq!(edit.offset, Db(4.0));
    // Alt+Home: no offset.
    let (_, edit, what) = trace_update(&t.key("Alt+Home"));
    assert_eq!(edit.offset, Db(0.0));
    assert_eq!(what, "slot 2 (t13): no offset");
    assert_eq!(t.st.edit(MeasId(1)), LiveEdit::default(), "live untouched");

    // Tenths stay tenths however many steps.
    t.conn(with_traces(vec![
        with_offset(stored(13, Some(2), 2), 0.3),
        spec_meta(16),
        {
            let mut l = stored(12, None, 2);
            l.edit.locked = true;
            l
        },
    ]));
    let (_, edit, _) = trace_update(&t.key("Alt+Down"));
    assert_eq!(edit.offset, Db(-0.7));

    // Any kind of trace, from any pane: a spectrum capture with the transfer pane focused.
    t.st.update(Msg::SelectTrace(TraceId(16)), &t.keys);
    let (id, edit, _) = trace_update(&t.key("Alt+Up"));
    assert_eq!((id, edit.offset), (TraceId(16), Db(1.0)));
    // J types it, in the spectrum pane too.
    t.key("Alt+2");
    assert_eq!(t.st.selected_trace, Some(TraceId(16)), "drawn there: kept");
    t.type_key("J", "j");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::TraceOffset(TraceId(16))
    ));
    t.key("Esc");
    // A locked trace says so and sends nothing.
    t.st.update(Msg::SelectTrace(TraceId(12)), &t.keys);
    assert!(t.key("Alt+Up").is_empty());
    assert!(
        t.last_toast().contains("t12 is locked"),
        "{}",
        t.last_toast()
    );
}

#[test]
fn offset_steps_change_the_pane_measurement_without_a_trace() {
    let mut t = T::new();
    // The transfer pane: its measurement, display only (nothing is sent).
    assert!(t.key("Alt+Up").is_empty());
    assert_eq!(t.st.edit(MeasId(1)).offset_db, 1.0);
    assert_eq!(t.last_toast(), "Main L: offset +1.0 dB (display)");
    t.key("Alt+Shift+Up");
    assert_eq!(t.st.edit(MeasId(1)).offset_db, 4.0);
    // The spectrum pane: the spectrum.
    t.key("Alt+2");
    assert!(t.key("Alt+Shift+Down").is_empty());
    assert_eq!(t.st.edit(MeasId(2)).offset_db, -3.0);
    assert_eq!(t.last_toast(), "Sub: offset −3.0 dB (display)");
    assert_eq!(t.st.edit(MeasId(1)).offset_db, 4.0);
    t.key("Alt+Home");
    assert_eq!(t.st.edit(MeasId(2)).offset_db, 0.0);
    assert_eq!(t.last_toast(), "Sub: no offset (display)");
    // J types it there too.
    t.type_key("J", "j");
    assert!(matches!(
        &t.st.overlay,
        Overlay::Prompt(p) if p.kind == PromptKind::Offset(MeasId(2))
    ));
    t.text("-2.5");
    assert!(t.key("Enter").is_empty());
    assert_eq!(t.st.edit(MeasId(2)).offset_db, -2.5);
    assert_eq!(t.last_toast(), "Sub: offset −2.5 dB (display)");
}

/// The offset is written next to the curve, so a spread trace is never read as a level
/// difference: the transfer legend's tag, the spectrum legend's.
#[test]
fn offsets_are_named_where_the_curves_are_drawn() {
    let mut t = T::new();
    let tf = with_offset(captured(13, Some(2), 2), 3.0);
    let sp = with_offset(spec_meta(16), -6.0);
    t.conn(with_traces(vec![tf.clone(), sp.clone()]));
    t.conn(data(&tf, vec![-10.0; 8]));
    t.conn(data(&sp, vec![-90.0; 8]));
    let theme = Theme::dark();
    let s = crate::scenes::transfer(&t.st, t.pane(PaneKind::Transfer), &theme, SIZE, now());
    let e = s
        .legend
        .iter()
        .find(|e| e.name == "t13")
        .expect("legend entry");
    assert!(e.tags.contains(&"+3.0 dB".to_owned()), "{e:?}");
    let d = s.traces.iter().find(|d| d.name == "t13").expect("trace");
    assert!(d.magnitude_db.iter().all(|m| *m == -7.0), "the curve moves");
    let s = crate::scenes::spectrum(&t.st, t.pane(PaneKind::Spectrum), &theme, SIZE, now());
    let texts: Vec<&str> = s.legend.iter().map(|e| e.text.as_str()).collect();
    assert_eq!(texts, ["t16 · offset −6.0 dB"]);
}

/// The selected stored trace is marked in the legend of the pane that draws it (the title
/// names it too); V moving the selection moves the mark.
#[test]
fn the_selected_trace_is_marked_in_its_legend() {
    let mut t = T::new();
    let a = captured(13, Some(1), 2);
    let b = captured(14, Some(2), 2);
    let sp = spec_meta(16);
    t.conn(with_traces(vec![a.clone(), b.clone(), sp.clone()]));
    t.conn(data(&a, vec![-10.0; 8]));
    t.conn(data(&b, vec![-12.0; 8]));
    t.conn(data(&sp, vec![-90.0; 8]));
    let theme = Theme::dark();
    let marked = |t: &T| {
        let tf = crate::scenes::transfer(&t.st, t.pane(PaneKind::Transfer), &theme, SIZE, now());
        let spec = crate::scenes::spectrum(&t.st, t.pane(PaneKind::Spectrum), &theme, SIZE, now());
        tf.legend
            .iter()
            .chain(&spec.legend)
            .filter(|e| e.selected)
            .map(|e| e.name.clone())
            .collect::<Vec<_>>()
    };
    assert!(marked(&t).is_empty());
    t.key("V");
    assert_eq!(marked(&t), ["t13"]);
    t.key("V");
    assert_eq!(marked(&t), ["t14"]);
    t.key("V");
    assert_eq!(marked(&t), ["t16"]);
}

fn spec_frame(t: &mut T, meas: u32, level: Vec<f32>) {
    spec_frame_at(t, meas, level, 1234, Instant::now());
}

/// [`spec_frame`] with its `seq`, received and drained at `at`.
fn spec_frame_at(t: &mut T, meas: u32, level: Vec<f32>, seq: u64, at: Instant) {
    let wall = ac2_proto::samples::stamp(None).capture_wall_ns;
    spec_frame_captured(t, meas, level, seq, at, wall);
}

/// [`spec_frame_at`] captured at `wall`.
fn spec_frame_captured(
    t: &mut T,
    meas: u32,
    level: Vec<f32>,
    seq: u64,
    at: Instant,
    wall: ac2_proto::units::WallNs,
) {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::Frame;
    use ac2_proto::FrameData;
    use ac2_proto::frame::{SpecFrame, SpecMeta};
    let grid = GridDef::Linear {
        fs: Hz(48_000.0),
        n: (level.len() as u32 - 1) * 2,
    };
    let data = FrameData::Spec(SpecFrame {
        meas: MeasId(meas),
        meta: SpecMeta {
            window: Window::Hann,
            scale: LevelScale::Dbfs,
            level: ac2_proto::model::SpecLevel::Tone,
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
            smoothing: None,
            math: None,
            bands: None,
        },
        level,
    });
    let stamp = ac2_proto::frame::FrameStamp {
        seq,
        capture_wall_ns: wall,
        ..ac2_proto::samples::stamp(Some(grid.clone()))
    };
    let f = TopicFrame {
        topic: data.topic(),
        frame: Arc::new(Frame { stamp, data }),
        received: at,
        since_new: std::time::Duration::ZERO,
        age: Some(0.0),
        stale: false,
    };
    let mut latest = Latest::default();
    latest.frames.insert(f.topic.to_string().into(), f);
    let mut grids = std::collections::BTreeMap::new();
    grids.insert(grid.id(), Arc::new(grid));
    t.conn(ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained: at,
    })));
}

/// A live curve glides from the drawn levels to each new frame over about the time since
/// the frame before, picks up mid-glide from where it is drawn, and jumps when the operator
/// turns the glide off.
#[test]
fn a_live_curve_glides_to_each_new_frame() {
    let mut t = T::new();
    let ms = |v: u64| std::time::Duration::from_millis(v);
    let t0 = Instant::now();
    spec_frame_at(&mut t, 3, vec![-60.0; 5], 1, t0);
    assert!(
        t.st.spec_glide.is_empty(),
        "a first frame is drawn as received"
    );
    let t1 = t0 + ms(100);
    spec_frame_at(&mut t, 3, vec![-40.0; 5], 2, t1);
    let to =
        t.st.data
            .as_ref()
            .expect("data")
            .latest
            .frames
            .values()
            .next()
            .expect("frame")
            .clone();
    let g = t.st.spec_glide[&to.topic].clone();
    assert_eq!(g.to_seq, 2);
    assert!((g.duration_s - 0.1).abs() < 1e-9);
    assert!(t.st.spectrum_gliding(t1 + ms(50)));
    assert!(!t.st.spectrum_gliding(t1 + ms(100)));
    assert_eq!(g.drawn(&to.frame, t1 + ms(50)), Some(vec![-50.0; 5]));
    assert_eq!(
        g.drawn(&to.frame, t1 + ms(100)),
        None,
        "arrived: the frame itself"
    );
    // A frame half way through the glide starts from the half-way curve.
    let t2 = t1 + ms(50);
    spec_frame_at(&mut t, 3, vec![-60.0; 5], 3, t2);
    let g = t.st.spec_glide[&to.topic].clone();
    assert_eq!(&*g.from, &[-50.0; 5]);
    assert!((g.duration_s - 0.05).abs() < 1e-9);
    // The same levels again (a refresh) move nothing.
    spec_frame_at(&mut t, 3, vec![-60.0; 5], 4, t2 + ms(300));
    assert!(t.st.spec_glide.is_empty());
    // Off: every frame is drawn as received.
    t.st.prefs.spec_ease = false;
    spec_frame_at(&mut t, 3, vec![-30.0; 5], 5, t2 + ms(400));
    assert!(t.st.spec_glide.is_empty());
    assert!(!t.st.spectrum_gliding(t2 + ms(401)));
}

/// The state with measurement `id` running or stopped.
fn with_running(id: u32, running: bool) -> ConnEvent {
    let mut s = daemon_state();
    for m in &mut s.measurements {
        if m.id == MeasId(id) {
            m.running = running;
        }
    }
    mirror(s)
}

fn meas_deleted(r: &[Request]) -> Option<(MeasId, String)> {
    match r {
        [
            Request::Call {
                cmd: Command::MeasDelete { meas, .. },
                what,
            },
        ] => Some((*meas, what.clone())),
        _ => None,
    }
}

/// Frames captured 85 ms apart that arrive 100 ms and then 67 ms apart (a long FFT's hop
/// beating against the daemon's hand-offs) glide by their capture times: the glide to the
/// early frame lasts until the latest seen arrival would have brought it, not 67 ms.
#[test]
fn live_curve_glides_are_paced_by_capture_time() {
    use ac2_proto::units::WallNs;
    let mut t = T::new();
    let ms = |v: u64| std::time::Duration::from_millis(v);
    let t0 = Instant::now();
    let w0 = 1_700_000_000_000_000_000u64;
    let hop = 85_000_000u64;
    spec_frame_captured(&mut t, 3, vec![-60.0; 5], 1, t0, WallNs(w0));
    spec_frame_captured(&mut t, 3, vec![-50.0; 5], 2, t0 + ms(100), WallNs(w0 + hop));
    spec_frame_captured(
        &mut t,
        3,
        vec![-40.0; 5],
        3,
        t0 + ms(167),
        WallNs(w0 + 2 * hop),
    );
    let g = t.st.spec_glide.values().next().expect("gliding").clone();
    assert_eq!(g.to_seq, 3);
    // 85 ms of capture time, plus the 18 ms this frame came earlier than the one before.
    assert!((g.duration_s - 0.103).abs() < 1e-6, "{}", g.duration_s);
}

#[path = "state_display_axis_tests.rs"]
mod axis;

#[path = "state_display_delete_tests.rs"]
mod delete;

#[path = "state_display_select_tests.rs"]
mod select;

#[path = "state_display_banner_tests.rs"]
mod banners;

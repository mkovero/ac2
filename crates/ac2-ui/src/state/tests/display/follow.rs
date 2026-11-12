//! Reducer tests of the spectrum level axis following its content: which ranges follow,
//! what a follow move leaves alone (the layout, `ui.toml`), the hands-off time, groups of
//! panes on one range and the setting.

use super::*;
use ac2_scene::view::follow::HANDS_OFF_S;

/// Frames at 10 a second on the app's clock, each measurement's `seq` its own.
struct Clock {
    now: f64,
    seq: u64,
    incarnation: u64,
}

impl Clock {
    fn new() -> Self {
        Self {
            now: 1.0,
            seq: 10,
            incarnation: 0x5eed_5eed_5eed_5eed,
        }
    }

    /// One frame of each of `meas` (a measurement and its top, dB), the clock ticked to it.
    fn frame(&mut self, t: &mut T, meas: &[(u32, f32)]) {
        self.now += 0.1;
        self.seq += 1;
        t.tick(self.now);
        frames(t, meas, self.seq, self.incarnation);
    }

    /// `s` seconds of frames.
    fn run(&mut self, t: &mut T, s: f64, meas: &[(u32, f32)]) {
        for _ in 0..(s * 10.0).round() as usize {
            self.frame(t, meas);
        }
    }
}

/// Levels on bins at 0, 6, 12, 18 and 24 kHz whose highest in 20 Hz – 20 kHz is `top`.
fn levels(top: f32) -> Vec<f32> {
    vec![0.0, top - 40.0, top - 20.0, top, 0.0]
}

/// One data snapshot with a spectrum frame of each of `meas`.
fn frames(t: &mut T, meas: &[(u32, f32)], seq: u64, incarnation: u64) {
    use ac2_client::{Latest, TopicFrame};
    use ac2_proto::Frame;
    use ac2_proto::FrameData;
    use ac2_proto::frame::{SpecFrame, SpecMeta};
    let grid = GridDef::Linear {
        fs: Hz(48_000.0),
        n: 8,
    };
    let mut latest = Latest::default();
    let at = Instant::now();
    for (m, top) in meas {
        let data = FrameData::Spec(SpecFrame {
            meas: MeasId(*m),
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
            level: levels(*top),
        });
        let stamp = ac2_proto::frame::FrameStamp {
            seq,
            daemon_incarnation: DaemonIncarnation(incarnation),
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
        latest.frames.insert(f.topic.to_string().into(), f);
    }
    let mut grids = std::collections::BTreeMap::new();
    grids.insert(grid.id(), Arc::new(grid));
    t.conn(ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids,
        drained: at,
    })));
}

fn sp(t: &T) -> PaneId {
    t.pane(PaneKind::Spectrum)
}

fn range(t: &T, p: PaneId) -> Range {
    t.st.level_range(p).expect("level axis")
}

/// Settled: a move eases over 0.8 s.
fn settle(c: &mut Clock, t: &mut T, meas: &[(u32, f32)]) {
    c.run(t, 1.0, meas);
}

/// A step of +20 dB on a pane framed on its first frame moves the axis up within the up
/// window, onto the grid, nothing else changing.
#[test]
fn a_level_step_moves_an_automatic_range_up() {
    let mut t = T::new();
    let mut c = Clock::new();
    c.frame(&mut t, &[(2, -30.0)]);
    let p = sp(&t);
    let fit = range(&t, p);
    assert_eq!(fit, Range::new(-80.0, -10.0));
    c.run(&mut t, 5.0, &[(2, -30.0)]);
    assert_eq!(range(&t, p), fit, "steady content: no move");
    c.run(&mut t, 3.5, &[(2, -10.0)]);
    settle(&mut c, &mut t, &[(2, -10.0)]);
    assert_eq!(range(&t, p), Range::new(-60.0, 10.0));
}

/// A range set by hand never follows; Shift+Home fits it, makes it automatic again, and
/// the next step (after the hands-off time the key began) moves it.
#[test]
fn a_hand_set_range_stays_until_a_fit_makes_it_automatic() {
    let mut t = T::new();
    let mut c = Clock::new();
    c.frame(&mut t, &[(2, -30.0)]);
    let p = sp(&t);
    t.st.apply(Msg::FocusPane(p), &t.keys);
    t.key("Ctrl+Up");
    let panned = range(&t, p);
    c.run(&mut t, 60.0, &[(2, -10.0)]);
    assert_eq!(range(&t, p), panned, "the operator's range");
    t.key("Shift+Home");
    let fit = range(&t, p);
    assert_eq!(fit, Range::new(-60.0, 10.0));
    assert!(t.st.operator_levels(p).spectrum_dbfs.is_none(), "automatic");
    c.run(&mut t, HANDS_OFF_S + 1.0, &[(2, -10.0)]);
    assert_eq!(range(&t, p), fit);
    c.run(&mut t, 4.0, &[(2, 10.0)]);
    settle(&mut c, &mut t, &[(2, 10.0)]);
    assert!(range(&t, p).hi > fit.hi, "{:?}", range(&t, p));
}

/// A follow move is no edit: no request, `operator_levels` unchanged, nothing for
/// `ui.toml`.
#[test]
fn a_follow_move_sends_no_layout_and_writes_nothing() {
    let mut t = T::new();
    let mut c = Clock::new();
    c.frame(&mut t, &[(2, -30.0)]);
    let p = sp(&t);
    let before = t.st.operator_levels(p);
    t.st.prefs_dirty = false;
    let toml = t.st.prefs.to_toml();
    let keys = t.keys.clone();
    let mut out = Vec::new();
    for _ in 0..50 {
        c.now += 0.1;
        c.seq += 1;
        out.extend(t.st.apply(
            Msg::Tick {
                now_s: c.now,
                dt_s: 0.1,
            },
            &keys,
        ));
        frames(&mut t, &[(2, -10.0)], c.seq, c.incarnation);
    }
    assert_ne!(range(&t, p), Range::new(-80.0, -10.0), "moved");
    assert!(out.is_empty(), "{out:?}");
    assert_eq!(t.st.operator_levels(p), before);
    // The next message of the operator's finds nothing to save either.
    t.st.apply(Msg::FocusPane(PaneId(1)), &keys);
    assert!(!t.st.prefs_dirty);
    assert_eq!(t.st.prefs.to_toml(), toml);
}

/// A stopped measurement holds its pane's range, in every view: the time without its
/// frames (and any frame still drawn while stopped) is no silence. Started again, the range
/// follows the new run from where it was held: its first frames do not frame it, a lasting
/// louder level moves it up as before.
#[test]
fn a_stopped_run_holds_its_range_and_a_start_follows_from_it() {
    use ac2_scene::view::SpectrumMode;
    for mode in [
        SpectrumMode::Spectrum,
        SpectrumMode::Split,
        SpectrumMode::Spectrograph,
        SpectrumMode::Spectrograph3d,
    ] {
        let mut t = T::new();
        let p = sp(&t);
        if let Some(v) = t.st.layout.view_mut(p) {
            v.modes.spectrum = mode;
        }
        let mut c = Clock::new();
        c.frame(&mut t, &[(2, -30.0)]);
        c.run(&mut t, 5.0, &[(2, -30.0)]);
        let held = range(&t, p);
        assert_eq!(held, Range::new(-80.0, -10.0), "{mode:?}");
        t.conn(with_running(2, false));
        // Minutes stopped: ticks without frames, then the last frames redrawn low.
        for _ in 0..3000 {
            c.now += 0.1;
            t.tick(c.now);
        }
        c.run(&mut t, 300.0, &[(2, -95.0)]);
        assert_eq!(range(&t, p), held, "{mode:?}: moved while stopped");
        t.conn(with_running(2, true));
        // The new run's first frames are low (its average filling): no jump down.
        c.run(&mut t, 2.0, &[(2, -95.0)]);
        assert_eq!(range(&t, p), held, "{mode:?}: a start framed it");
        c.run(&mut t, 3.5, &[(2, -10.0)]);
        settle(&mut c, &mut t, &[(2, -10.0)]);
        assert_eq!(range(&t, p), Range::new(-60.0, 10.0), "{mode:?}");
    }
}

/// A range the operator set stays through a stop and a start, and Ctrl+I while stopped
/// still zooms it.
#[test]
fn a_hand_set_range_works_while_stopped() {
    let mut t = T::new();
    let mut c = Clock::new();
    c.frame(&mut t, &[(2, -30.0)]);
    let p = sp(&t);
    let fit = range(&t, p);
    t.conn(with_running(2, false));
    c.now += 40.0;
    t.tick(c.now);
    t.st.apply(Msg::FocusPane(p), &t.keys);
    t.key("Ctrl+I");
    let zoomed = range(&t, p);
    assert!(zoomed.span() < fit.span(), "{zoomed:?}");
    c.run(&mut t, 60.0, &[(2, -95.0)]);
    t.conn(with_running(2, true));
    c.run(&mut t, 60.0, &[(2, -10.0)]);
    assert_eq!(range(&t, p), zoomed);
}

/// A layout received without a range makes the slot automatic: the follower moves it.
#[test]
fn a_layout_without_a_range_makes_the_slot_automatic() {
    let mut t = T::new();
    let mut c = Clock::new();
    c.frame(&mut t, &[(2, -30.0)]);
    let p = sp(&t);
    t.st.apply(Msg::FocusPane(p), &t.keys);
    t.key("Ctrl+I");
    let zoomed = range(&t, p);
    t.st.set_operator_levels(p, &ac2_proto::model::layout::LayoutLevels::default());
    c.run(&mut t, HANDS_OFF_S, &[(2, -30.0)]);
    c.run(&mut t, 4.0, &[(2, 0.0)]);
    settle(&mut c, &mut t, &[(2, 0.0)]);
    assert!(range(&t, p).hi > zoomed.hi, "followed: {:?}", range(&t, p));
}

/// A step during a drag on the pane moves only once the hands-off time after the drag has
/// passed; the decision waits, it is not dropped.
#[test]
fn a_step_during_a_drag_waits_out_the_hands_off_time() {
    let mut t = T::new();
    let mut c = Clock::new();
    c.frame(&mut t, &[(2, -30.0)]);
    let p = sp(&t);
    let fit = range(&t, p);
    t.st.apply(
        Msg::PanePointer {
            pane: p,
            held: true,
        },
        &t.keys,
    );
    c.run(&mut t, 40.0, &[(2, -10.0)]);
    assert_eq!(range(&t, p), fit, "held");
    t.st.apply(
        Msg::PanePointer {
            pane: p,
            held: false,
        },
        &t.keys,
    );
    c.run(&mut t, HANDS_OFF_S - 1.0, &[(2, -10.0)]);
    assert_eq!(range(&t, p), fit, "hands off");
    c.run(&mut t, 2.0, &[(2, -10.0)]);
    settle(&mut c, &mut t, &[(2, -10.0)]);
    assert_eq!(range(&t, p), Range::new(-60.0, 10.0));
}

/// The spectrum pane split (N): both halves show Sub, on one range through a step.
fn two_panes(t: &mut T) -> (PaneId, PaneId) {
    t.st.pane_area = (1000.0, 600.0);
    let a = sp(t);
    t.st.apply(Msg::FocusPane(a), &t.keys);
    t.key("N");
    let b = t.st.layout.focus;
    assert_ne!(a, b);
    (a, b)
}

#[test]
fn two_panes_of_one_unit_stay_on_one_range() {
    let mut t = T::new();
    let mut c = Clock::new();
    let (a, b) = two_panes(&mut t);
    c.frame(&mut t, &[(2, -30.0)]);
    // The keys that split the pane hold it still for a while.
    c.run(&mut t, HANDS_OFF_S, &[(2, -30.0)]);
    assert_eq!(range(&t, a), range(&t, b));
    c.run(&mut t, 4.0, &[(2, -10.0)]);
    for _ in 0..10 {
        c.frame(&mut t, &[(2, -10.0)]);
        assert_eq!(range(&t, a), range(&t, b), "mid-ease too");
    }
    assert_eq!(range(&t, a), Range::new(-60.0, 10.0));
}

/// A second pane showing content 30 dB higher joins: the group frames once over both,
/// both panes on the new range.
#[test]
fn a_second_pane_joining_refits_the_group_over_both() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements.push(meas(4, "Sub 2", spectrum()));
    t.conn(mirror(s));
    let mut c = Clock::new();
    c.frame(&mut t, &[(2, -60.0), (4, -30.0)]);
    let a = sp(&t);
    assert_eq!(range(&t, a), Range::new(-110.0, -40.0));
    let (_, b) = two_panes(&mut t);
    t.drop_meas(b, MeasId(4));
    c.frame(&mut t, &[(2, -60.0), (4, -30.0)]);
    // The keys that split the pane hold the group's framing for a while.
    c.run(&mut t, HANDS_OFF_S, &[(2, -60.0), (4, -30.0)]);
    let both =
        ac2_scene::view::follow::fit([-100.0, -80.0, -60.0, -70.0, -50.0, -30.0]).expect("finite");
    assert_eq!(range(&t, a), both);
    assert_eq!(range(&t, b), both);
    c.run(&mut t, 5.0, &[(2, -60.0), (4, -30.0)]);
    assert_eq!(range(&t, a), both, "once");
}

/// Shift+Home on one member frames the whole group; hands off on one member holds the
/// other's move too.
#[test]
fn a_fit_on_one_member_refits_the_group_and_hands_off_holds_it() {
    let mut t = T::new();
    let mut c = Clock::new();
    let (a, b) = two_panes(&mut t);
    c.frame(&mut t, &[(2, -30.0)]);
    c.run(&mut t, HANDS_OFF_S, &[(2, -30.0)]);
    t.st.apply(
        Msg::PanePointer {
            pane: a,
            held: true,
        },
        &t.keys,
    );
    c.run(&mut t, 10.0, &[(2, -10.0)]);
    assert_eq!(range(&t, b), Range::new(-80.0, -10.0), "a held holds b");
    t.st.apply(
        Msg::PanePointer {
            pane: a,
            held: false,
        },
        &t.keys,
    );
    t.st.apply(Msg::FocusPane(b), &t.keys);
    t.key("Shift+Home");
    c.frame(&mut t, &[(2, -10.0)]);
    let fit = ac2_scene::view::follow::fit([-50.0, -30.0, -10.0]).expect("finite");
    assert_eq!(range(&t, b), fit);
    assert_eq!(range(&t, a), fit, "the whole group");
}

/// With following off nothing moves, and Shift+Home sets the operator's range as before.
#[test]
fn with_following_off_nothing_moves_and_a_fit_is_the_operators() {
    let mut t = T::new();
    t.st.prefs.level_follow = false;
    let mut c = Clock::new();
    c.frame(&mut t, &[(2, -30.0)]);
    let p = sp(&t);
    let fit = range(&t, p);
    c.run(&mut t, 30.0, &[(2, -10.0)]);
    assert_eq!(range(&t, p), fit);
    t.st.apply(Msg::FocusPane(p), &t.keys);
    t.key("Shift+Home");
    assert!(t.st.operator_levels(p).spectrum_dbfs.is_some());
}

/// The frequency axis zoomed in to where the top column is not shown: the same move.
#[test]
fn a_frequency_zoom_changes_no_decision() {
    let run = |zoom: bool| {
        let mut t = T::new();
        let mut c = Clock::new();
        c.frame(&mut t, &[(2, -30.0)]);
        if zoom {
            t.st.apply(
                Msg::Zoom {
                    about_hz: 6000.0,
                    factor: 30.0,
                },
                &t.keys,
            );
        }
        c.run(&mut t, 4.0, &[(2, -10.0)]);
        settle(&mut c, &mut t, &[(2, -10.0)]);
        (range(&t, sp(&t)), t.st.view.freq)
    };
    let (plain, wide) = run(false);
    let (zoomed, narrow) = run(true);
    assert_ne!(wide, narrow);
    assert_eq!(plain, zoomed);
}

/// A restarted daemon (another incarnation) is a new signal history: the frames before it
/// decide nothing.
#[test]
fn a_restarted_daemon_clears_the_history() {
    let mut t = T::new();
    let mut c = Clock::new();
    c.frame(&mut t, &[(2, -30.0)]);
    let p = sp(&t);
    let fit = range(&t, p);
    c.run(&mut t, 2.5, &[(2, -10.0)]);
    c.incarnation += 1;
    c.run(&mut t, 2.5, &[(2, -10.0)]);
    assert_eq!(range(&t, p), fit, "the old history moved it");
    c.run(&mut t, 1.0, &[(2, -10.0)]);
    settle(&mut c, &mut t, &[(2, -10.0)]);
    assert_eq!(range(&t, p), Range::new(-60.0, 10.0));
}

/// In the 3D view the follower runs as in the others: a step moves the range, and the
/// skyline's heights span the moved range.
#[test]
fn a_step_in_the_3d_view_moves_the_skyline_heights() {
    let mut t = T::new();
    let p = sp(&t);
    if let Some(v) = t.st.layout.view_mut(p) {
        v.modes.spectrum = ac2_scene::view::SpectrumMode::Spectrograph3d;
    }
    let mut c = Clock::new();
    c.frame(&mut t, &[(2, -30.0)]);
    c.run(&mut t, 2.0, &[(2, -30.0)]);
    let heights = |t: &T| {
        let mut sky = ac2_scene::spectrograph_3d::Skyline::default();
        let s = crate::scenes::spectrograph_3d(
            &t.st,
            p,
            &mut sky,
            (ac2_scene::primitives::SkylineTier::Full, false),
            (0.0, 0.0),
            &Theme::dark(),
            Viewport::new(800.0, 500.0),
            now(),
        );
        s.scene
            .layers
            .iter()
            .flat_map(|l| &l.skylines)
            .map(|k| k.range)
            .next()
            .expect("a skyline")
    };
    assert_eq!(heights(&t), [-80.0, -10.0]);
    c.run(&mut t, 4.0, &[(2, -10.0)]);
    settle(&mut c, &mut t, &[(2, -10.0)]);
    assert_eq!(range(&t, p), Range::new(-60.0, 10.0));
    assert_eq!(heights(&t), [-60.0, 10.0]);
}

/// Two panes of one unit, `a` showing Sub (2) and `b` Sub 2 (4), framed as a group: the
/// split's hands-off time waited out.
fn two_panes_apart(t: &mut T, c: &mut Clock, content: &[(u32, f32)]) -> (PaneId, PaneId) {
    let mut s = daemon_state();
    s.measurements.push(meas(4, "Sub 2", spectrum()));
    t.conn(mirror(s));
    c.frame(t, content);
    let (a, b) = two_panes(t);
    t.drop_meas(b, MeasId(4));
    c.run(t, HANDS_OFF_S + 1.0, content);
    (a, b)
}

/// A member joining while the operator works on the group's panes frames the group only
/// once the hands-off time is over: no member's range moves under the operator's hand, and
/// the framing waits, it is not dropped.
#[test]
fn a_group_frame_waits_out_the_hands_off_time() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements.push(meas(4, "Sub 2", spectrum()));
    t.conn(mirror(s));
    let mut c = Clock::new();
    let both_meas = [(2, -60.0), (4, -30.0)];
    c.frame(&mut t, &both_meas);
    let a = sp(&t);
    let own = range(&t, a);
    let (_, b) = two_panes(&mut t);
    t.drop_meas(b, MeasId(4));
    c.run(&mut t, HANDS_OFF_S - 2.0, &both_meas);
    assert_eq!(range(&t, a), own, "a held still while worked on");
    c.run(&mut t, 3.0, &both_meas);
    let both =
        ac2_scene::view::follow::fit([-100.0, -80.0, -60.0, -70.0, -50.0, -30.0]).expect("finite");
    assert_eq!(range(&t, a), both, "framed once hands off");
    assert_eq!(range(&t, b), both);
}

/// Shift+Home on one member while a button is held on the other: the group frames when the
/// button comes up, not before.
#[test]
fn an_operator_fit_waits_for_a_held_member() {
    let mut t = T::new();
    let mut c = Clock::new();
    let both_meas = [(2, -60.0), (4, -30.0)];
    let (a, b) = two_panes_apart(&mut t, &mut c, &both_meas);
    let group = range(&t, a);
    c.run(&mut t, 2.0, &[(2, -60.0), (4, -80.0)]);
    t.st.apply(
        Msg::PanePointer {
            pane: a,
            held: true,
        },
        &t.keys,
    );
    t.st.apply(Msg::FocusPane(b), &t.keys);
    t.key("Shift+Home");
    c.run(&mut t, 2.0, &[(2, -60.0), (4, -80.0)]);
    assert_eq!(range(&t, a), group, "held");
    t.st.apply(
        Msg::PanePointer {
            pane: a,
            held: false,
        },
        &t.keys,
    );
    c.frame(&mut t, &[(2, -60.0), (4, -80.0)]);
    let fit = ac2_scene::view::follow::fit([-100.0, -80.0, -60.0, -120.0, -100.0, -80.0])
        .expect("finite");
    assert_eq!(range(&t, a), fit);
    assert_eq!(range(&t, b), fit);
}

/// A member whose measurement stopped sending holds no top: the group follows the members
/// still live, down when they lie low for the slow window.
#[test]
fn a_stopped_member_does_not_hold_the_group_up() {
    let mut t = T::new();
    let mut c = Clock::new();
    let (a, b) = two_panes_apart(&mut t, &mut c, &[(2, -150.0), (4, 0.0)]);
    let group = range(&t, a);
    assert_eq!(range(&t, b), group);
    c.run(&mut t, 300.0, &[(2, -150.0)]);
    assert!(
        range(&t, a).hi < group.hi,
        "{:?} from {group:?}",
        range(&t, a)
    );
    assert_eq!(range(&t, b), range(&t, a));
}

/// A curve gone from the frames for a moment (a reconnect) and back from a restarted daemon:
/// the history before it is another signal's and decides nothing.
#[test]
fn a_restart_across_a_gap_in_the_frames_clears_the_history() {
    let mut t = T::new();
    let mut c = Clock::new();
    c.frame(&mut t, &[(2, -30.0)]);
    let p = sp(&t);
    let fit = range(&t, p);
    c.run(&mut t, 2.5, &[(2, -10.0)]);
    c.frame(&mut t, &[]);
    c.incarnation += 1;
    c.run(&mut t, 2.5, &[(2, -10.0)]);
    assert_eq!(range(&t, p), fit, "the old history moved it");
    c.run(&mut t, 1.0, &[(2, -10.0)]);
    settle(&mut c, &mut t, &[(2, -10.0)]);
    assert_eq!(range(&t, p), Range::new(-60.0, 10.0));
}

/// With following off no frame is read for it; turned on, the follower starts from what
/// arrives from then on.
#[test]
fn following_turned_on_starts_from_its_own_frames() {
    let mut t = T::new();
    t.st.prefs.level_follow = false;
    let mut c = Clock::new();
    c.frame(&mut t, &[(2, -30.0)]);
    let p = sp(&t);
    let fit = range(&t, p);
    c.run(&mut t, 10.0, &[(2, -10.0)]);
    assert!(!t.st.follow_fed(), "fed while off");
    t.st.prefs.level_follow = true;
    c.run(&mut t, 2.5, &[(2, -10.0)]);
    assert!(t.st.follow_fed());
    assert_eq!(range(&t, p), fit, "the frames before moved it");
    c.run(&mut t, 1.0, &[(2, -10.0)]);
    settle(&mut c, &mut t, &[(2, -10.0)]);
    assert_eq!(range(&t, p), Range::new(-60.0, 10.0));
}

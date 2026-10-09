//! Reducer tests of panes following the selection.

use super::*;
use crate::state::panes_drawing;

use PaneKind::{Distortion, Ir, Spectrum, Spl, Transfer};

fn select(t: &mut T, id: u32) {
    let keys = t.keys.clone();
    t.st.update(Msg::SelectMeas(MeasId(id)), &keys);
}

fn select_trace(t: &mut T, id: u32) {
    let keys = t.keys.clone();
    t.st.update(Msg::SelectTrace(TraceId(id)), &keys);
}

fn follow_on(t: &mut T) {
    let keys = t.keys.clone();
    t.st.prefs_dirty = false;
    t.st.update(Msg::Command(CommandId::PanesFollow), &keys);
    assert!(t.st.prefs.panes_follow);
    assert!(t.st.prefs_dirty);
}

#[test]
fn panes_drawing_each_kind() {
    let sweep = sweep_meta(7);
    let mut spec = stored(8, None, 2);
    spec.kind = TraceKind::Spectrum {
        scale: LevelScale::Dbfs,
    };
    assert_eq!(panes_drawing(&[&transfer()], &[]), [Transfer, Ir]);
    assert_eq!(panes_drawing(&[&spectrum()], &[]), [Spectrum]);
    assert_eq!(panes_drawing(&[&rta()], &[]), [Spectrum]);
    assert_eq!(panes_drawing(&[&spl_meter()], &[]), [Spl]);
    assert_eq!(panes_drawing(&[], &[&sweep]), [Transfer, Ir, Distortion]);
    assert_eq!(panes_drawing(&[], &[&spec]), [Spectrum]);
    assert_eq!(panes_drawing(&[&spl_meter()], &[&spec]), [Spectrum, Spl]);
    assert!(panes_drawing(&[], &[]).is_empty());
}

/// Off (the default) every shown pane is laid out whatever is selected; on, only those
/// that draw the selected measurement, the focus on one of them, and W keeps one of them.
#[test]
fn panes_follow_the_selected_measurement() {
    let mut t = T::new();
    t.conn(mirror(with_spl()));
    assert!(!t.st.prefs.panes_follow);
    select(&mut t, 2);
    assert_eq!(t.visible(), [Transfer, Spectrum, Ir, Spl]);

    follow_on(&mut t);
    assert!(
        t.last_toast().contains("panes follow selection"),
        "{}",
        t.last_toast()
    );
    assert_eq!(t.visible(), [Spectrum]);
    assert_eq!(t.focus_kind(), Spectrum);

    select(&mut t, 1);
    assert_eq!(t.visible(), [Transfer, Ir]);
    assert_eq!(t.focus_kind(), Transfer);
    // Alt+number counts the panes kept.
    t.key("Alt+2");
    assert_eq!(t.focus_kind(), Ir);
    t.key("Alt+1");
    assert_eq!(t.focus_kind(), Transfer);

    // W: the focused pane alone; another selection maximises one of its own panes.
    t.key("W");
    assert_eq!(t.visible(), [Transfer]);
    select(&mut t, 4);
    assert_eq!(t.visible(), [Spl]);
    assert_eq!(t.focus_kind(), Spl);
    t.key("W");
    t.key("W");
    assert!(!t.st.layout.maximized);
    assert_eq!(t.visible(), [Spl]);

    // Off again: every pane.
    let keys = t.keys.clone();
    t.st.update(Msg::Command(CommandId::PanesFollow), &keys);
    assert!(!t.st.prefs.panes_follow);
    assert_eq!(t.visible(), [Transfer, Spectrum, Ir, Spl]);
}

/// A selected stored trace keeps the panes drawing it and its measurement; an imported one
/// those drawing it, the focus moving there.
#[test]
fn panes_follow_a_selected_trace() {
    let mut t = T::new();
    let mut s = tree_state();
    let mut spec = stored(9, None, 2);
    spec.kind = TraceKind::Spectrum {
        scale: LevelScale::Dbfs,
    };
    s.traces.push(spec);
    t.conn(mirror(s));
    follow_on(&mut t);
    select(&mut t, 2);
    assert_eq!(t.visible(), [Spectrum]);
    select_trace(&mut t, 3);
    assert_eq!(t.visible(), [Transfer, Ir]);
    assert_eq!(t.focus_kind(), Transfer);
    select_trace(&mut t, 9);
    assert_eq!(t.visible(), [Spectrum]);
    assert_eq!(t.focus_kind(), Spectrum);
}

/// A sweep measurement keeps the sweep pane and, with runs, the panes drawing them.
#[test]
fn panes_follow_a_sweep() {
    let mut t = T::new();
    let mut s = daemon_state();
    s.measurements.push(meas(
        5,
        "Sweep 1",
        MeasKind::Sweep {
            config: SweepConfig {
                reference_input: 0,
                measurement_input: 1,
                outputs: vec![0],
                level: Dbfs(-50.0),
                sweep: EssSpec::with_fades(Hz(20.0), Hz(20_000.0), Seconds(3.0)),
                repeats: 1,
                gate: None,
                tail: Some(Seconds(1.0)),
                lf_harmonics: ac2_proto::model::LfHarmonics::Standard,
            },
        },
    ));
    t.conn(mirror(s.clone()));
    t.go(Distortion);
    t.key("Alt+1");
    follow_on(&mut t);
    select(&mut t, 5);
    assert_eq!(t.visible(), [Distortion]);
    assert_eq!(t.focus_kind(), Distortion);
    s.traces = vec![sweep_meta(7)];
    t.conn(mirror(s));
    assert_eq!(t.visible(), [Transfer, Ir, Distortion]);
    assert_eq!(t.focus_kind(), Distortion);
}

/// Never an empty screen: no shown pane drawing the selection keeps every shown pane; a
/// focus key past the panes kept stays, saying why.
#[test]
fn panes_follow_never_empties_the_screen() {
    let mut t = T::new();
    follow_on(&mut t);
    select(&mut t, 1);
    assert_eq!(t.visible(), [Transfer, Ir]);

    t.key("Alt+4");
    assert_eq!(t.focus_kind(), Transfer);
    assert!(
        t.last_toast().contains("no pane 4: 2 on screen"),
        "{}",
        t.last_toast()
    );
    assert_eq!(t.visible(), [Transfer, Ir]);
}

//! Reducer tests of the fit a pane's picture gets by itself: another view under G, and
//! new data in a pane, frame it exactly as Shift+Home does; a range the operator set in a
//! view stays when G comes back to it.

use super::*;
use crate::state::tests::ir_nav_tests::ir_event;
use crate::state::tests::meas_display::transfer_frame;
use crate::state::tests::sweep::sweep_once;
use ac2_scene::view::{IrAxes, IrPane};

/// What Shift+Home frames the focused pane's level axis to now: the default first, so the
/// key's own range is what is read.
fn shift_home(t: &mut T, kind: PaneKind) -> Range {
    t.key("Ctrl+Home");
    t.key("Shift+Home");
    t.level(kind)
}

/// A transfer pane's magnitude frames its first data as Shift+Home does; G back to the
/// magnitude frames what it shows then, unless the operator set the range in it.
#[test]
fn g_and_first_data_frame_the_transfer_magnitude() {
    let mut t = T::new();
    t.key("Alt+1");
    let start = t.level(PaneKind::Transfer);
    transfer_frame(&mut t, 1, -10.0, 1.0);
    let first = t.level(PaneKind::Transfer);
    assert_ne!(first, start, "the first data is framed");
    let mut u = T::new();
    u.key("Alt+1");
    transfer_frame(&mut u, 1, -10.0, 1.0);
    assert_eq!(shift_home(&mut u, PaneKind::Transfer), first);

    // Away in the phase view, the curve drops 40 dB; back on the magnitude it is framed.
    t.key("G");
    assert_eq!(t.st.layout.focused().modes.transfer, TransferView::Phase);
    transfer_frame(&mut t, 2, -50.0, 1.0);
    assert_eq!(t.level(PaneKind::Transfer), first, "not while unseen");
    t.key("Shift+G");
    let back = t.level(PaneKind::Transfer);
    assert_ne!(back, first);
    let mut u = T::new();
    u.key("Alt+1");
    transfer_frame(&mut u, 1, -50.0, 1.0);
    assert_eq!(shift_home(&mut u, PaneKind::Transfer), back);

    // A zoom is the operator's: G away and back keeps it, later data too.
    t.key("Ctrl+I");
    let zoomed = t.level(PaneKind::Transfer);
    t.key("G");
    transfer_frame(&mut t, 3, -20.0, 1.0);
    t.key("Shift+G");
    assert_eq!(t.level(PaneKind::Transfer), zoomed);
}

/// G on a spectrum pane frames what it shows in the next view as Shift+Home does; a range
/// the operator set stays through the views.
#[test]
fn g_frames_the_spectrum_pane() {
    let mut t = T::new();
    t.st.prefs.level_follow = false;
    t.key("Alt+2");
    spec_frame(&mut t, 2, vec![0.0, -138.0, -120.0, -95.0, 0.0]);
    let later = vec![0.0, -40.0, -30.0, -20.0, 0.0];
    spec_frame(&mut t, 2, later.clone());
    t.key("G");
    let fit = t.level(PaneKind::Spectrum);
    assert_eq!(fit, shift_home(&mut t, PaneKind::Spectrum));
    assert!(fit.hi > -20.0 && fit.lo < -40.0, "{fit:?}");

    t.key("Ctrl+I");
    let zoomed = t.level(PaneKind::Spectrum);
    t.key("G");
    assert_eq!(t.level(PaneKind::Spectrum), zoomed);
    t.key("Shift+G");
    assert_eq!(t.level(PaneKind::Spectrum), zoomed);
}

/// The IR views frame the impulse response as Shift+Home does when G enters them (their
/// axes are never told apart from a default, so on every entry), and a view entered before
/// any IR is in frames the first one.
#[test]
fn g_frames_the_impulse_response_views() {
    let mut t = T::new();
    t.key("Alt+1");
    // Magnitude → phase → coherence → IR linear → IR log, no IR in yet.
    for _ in 0..4 {
        t.key("G");
    }
    assert_eq!(
        t.st.layout.focused().modes.transfer,
        TransferView::Ir(IrMode::Log)
    );
    assert_eq!(
        t.st.view.ir_axes(IrPane::Live).level_db,
        IrAxes::DEFAULT_LEVEL_DB
    );
    t.conn(ir_event(1));
    let fit = t.st.view.ir_axes(IrPane::Live).level_db;
    assert_ne!(fit, IrAxes::DEFAULT_LEVEL_DB, "the first IR is framed");
    t.key("Ctrl+Home");
    t.key("Shift+Home");
    assert_eq!(t.st.view.ir_axes(IrPane::Live).level_db, fit);

    // A zoom, then the ETC view and back: framed again on entry.
    t.key("Ctrl+I");
    t.key("G");
    t.key("Shift+G");
    assert_eq!(t.st.view.ir_axes(IrPane::Live).level_db, fit);
    // The linear view: the whole IR, the amplitude following the peak.
    t.key("I");
    t.key("Shift+G");
    let a = t.st.view.ir_axes(IrPane::Live);
    assert_eq!((a.time_ms, a.amplitude), (None, None));
}

/// A sweep's IR views frame its impulse response as Shift+Home does when G enters them.
#[test]
fn g_frames_the_sweep_ir_views() {
    let mut t = T::new();
    sweep_once(&mut t);
    let (d, g) = sweep_data(7);
    // An arrival and a reflection: a decay for the log view to frame.
    let mut d = (*d).clone();
    let ir = &mut d.sweep.as_mut().expect("sweep").ir;
    ir.linear[100] = 0.5;
    ir.linear[110] = 0.05;
    t.conn(ConnEvent::Trace(Arc::new(d), g));
    assert_eq!(t.focus_kind(), PaneKind::Distortion);
    // Response → IR linear → IR log.
    t.key("G");
    t.key("G");
    assert_eq!(
        t.st.layout.focused().modes.sweep,
        ac2_scene::view::SweepMode::Ir(IrMode::Log)
    );
    let fit = t.st.view.ir_axes(IrPane::Sweep).level_db;
    assert_ne!(fit, IrAxes::DEFAULT_LEVEL_DB);
    t.key("Ctrl+Home");
    t.key("Shift+Home");
    assert_eq!(t.st.view.ir_axes(IrPane::Sweep).level_db, fit);
}

//! The app's edits of one measurement against the real daemon: keys pressed faster than
//! the replies come each count, and another client's commit in between holds nothing back.

use crate::harness::{Driver, R, on_rig};
use crate::window::{MAIN_L, Rig};
use ac2_proto::Command;
use ac2_proto::model::{MeasConfig, MeasKind, SmoothingFraction};
use ac2_ui::state::{AppState, Msg, PaneKind};

fn smoothing(s: &AppState) -> Option<SmoothingFraction> {
    match &s.rig.meas(MAIN_L).expect("Main L").config.kind {
        MeasKind::Transfer { config } => config.smoothing.map(|s| s.fraction),
        other => panic!("{other:?}"),
    }
}

/// The app's edits of Main L answered and in the mirror: what it makes the next edit on
/// is what the daemon holds.
fn settled(s: &AppState) -> bool {
    s.meas(MAIN_L) == s.rig.meas(MAIN_L)
}

fn on_main_l(rig: &Rig) -> R<Driver> {
    let mut d = on_rig(rig)?;
    let p = crate::common::pane(&d.st, PaneKind::Transfer);
    d.send(Msg::FocusPane(p));
    d.send(Msg::SelectMeas(MAIN_L));
    Ok(d)
}

/// K twice before the first reply, then Shift+K: one step from where it began, as the
/// operator pressed; Shift+K again is back where it began.
#[test]
fn steps_pressed_faster_than_the_replies_each_count() -> R {
    let rig = Rig::start()?;
    let mut d = on_main_l(&rig)?;
    let start = smoothing(&d.st);
    d.key("K");
    d.key("K");
    d.until("both steps answered", settled)?;
    d.key("Shift+K");
    d.until("the step back answered", |s| {
        settled(s) && smoothing(s) != start
    })?;
    d.key("Shift+K");
    d.until("back where it began", |s| {
        settled(s) && smoothing(s) == start
    })?;
    Ok(())
}

/// Another client changes Main L's configuration after the app last saw it, and the app
/// steps twice on what it saw: it says it changed elsewhere, and its next step goes and
/// lands.
#[test]
fn a_change_elsewhere_between_steps_holds_nothing_back() -> R {
    let rig = Rig::start()?;
    let mut d = on_main_l(&rig)?;
    let m = rig.meas(MAIN_L)?;
    rig.call(Command::MeasUpdate {
        meas: MAIN_L,
        base: m.edit_rev,
        config: MeasConfig {
            name: "Main L (FOH)".into(),
            ..m.config
        },
    })?;
    d.key("K");
    d.key("K");
    d.until("the change elsewhere said", |s| {
        s.toasts
            .list
            .iter()
            .any(|t| t.text.contains("changed elsewhere"))
    })?;
    d.until("settled", settled)?;
    let before = smoothing(&d.st);
    d.key("K");
    d.until("the next step lands", |s| {
        settled(s) && smoothing(s) != before
    })?;
    Ok(())
}

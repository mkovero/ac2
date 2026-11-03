//! A pane pick in a measurement's own panes names another measurement (`FOH SPL`'s meter
//! beside `SPL 2`'s): the pick stays that measurement's while the app cannot see it.

use super::*;
use ac2_proto::model::PaneLayout;
use ac2_proto::model::layout as wire;

fn spl_pane(meas: u32) -> PaneLayout {
    PaneLayout::Pane {
        pane: Box::new(wire::LayoutPane {
            kind: wire::PaneKind::Spl,
            transfer_view: wire::TransferView::Magnitude,
            spectrum_view: wire::SpectrumView::Spectrum,
            spl_view: wire::SplView::MeterLeq,
            sweep_view: wire::SweepView::Response,
            programme_view: wire::ProgrammeView::Meters,
            chrome: wire::PlotChrome::Full,
            content: Some(wire::PaneContent::Meas { meas: MeasId(meas) }),
            levels: wire::LayoutLevels::default(),
            plates: wire::LayoutPlates::default(),
            rig: wire::RigRef::ThisRig,
        }),
    }
}

/// `FOH SPL`'s meter over `SPL 2`'s: the two mics side by side, `SPL 2`'s own panes.
fn both_mics() -> PaneLayout {
    PaneLayout::Split {
        axis: wire::SplitAxis::Row,
        ratio: 0.5,
        a: Box::new(spl_pane(4)),
        b: Box::new(spl_pane(5)),
    }
}

/// The daemon with `SPL 2` (5, laid out as [`both_mics`]) and, with `foh`, `FOH SPL` (4).
fn daemon(foh: bool) -> State {
    let mut s = if foh { with_spl() } else { daemon_state() };
    let mut second = meas(5, "SPL 2", spl_meter());
    if let MeasKind::Spl { config } = &mut second.config.kind {
        config.input = 2;
    }
    second.layout = Some(both_mics());
    s.measurements.push(second);
    s
}

/// What each pane shows, in reading order.
fn shown(t: &T) -> Vec<Option<MeasId>> {
    t.st.layout
        .root
        .reading_order()
        .into_iter()
        .map(|p| t.st.pane_meas(p).map(|m| m.id))
        .collect()
}

/// The layouts `r` stores.
fn stored(r: &[Request]) -> Vec<PaneLayout> {
    r.iter()
        .filter_map(|r| match r {
            Request::Call {
                cmd: Command::MeasLayout { layout, .. },
                ..
            } => layout.clone(),
            _ => None,
        })
        .collect()
}

/// Every `meas.layout` call in `r` answered: the next edit goes out at once.
fn answer(t: &mut T, r: &[Request]) {
    for r in r {
        if let Request::Call {
            cmd: Command::MeasLayout { .. },
            what,
        } = r
        {
            t.conn(ConnEvent::Reply {
                what: what.clone(),
                result: Ok(()),
            });
        }
    }
}

fn desk() -> T {
    let mut t = T::new();
    t.conn(mirror(daemon(true)));
    t.st.apply(Msg::SelectMeas(MeasId(5)), &t.keys);
    assert_eq!(
        t.st.pane_layout(),
        crate::state::panes::meas_layout::owner_plain(&both_mics(), MeasId(5)),
        "SPL 2's own panes"
    );
    assert_eq!(shown(&t), [Some(MeasId(4)), Some(MeasId(5))]);
    t
}

/// An edit after `FOH SPL` is back still stores its pick, never `SPL 2`'s id.
fn edit_keeps_the_pick(t: &mut T, what: &str) {
    assert_eq!(
        shown(t),
        [Some(MeasId(4)), Some(MeasId(5))],
        "{what}: FOH SPL back in its pane"
    );
    let r = t.key("T");
    let l = stored(&r);
    assert!(
        !l.is_empty(),
        "{what}: T is an edit of SPL 2's panes: {r:?}"
    );
    for l in l {
        assert_eq!(
            l.panes()[0].content,
            Some(wire::PaneContent::Meas { meas: MeasId(4) }),
            "{what}: {l:?}"
        );
    }
}

/// The daemon restarts and lists `SPL 2` before `FOH SPL`.
#[test]
fn a_meter_pick_waits_through_a_restart_listing_it_late() {
    let mut t = desk();
    t.conn(mirror_of(daemon(false), 2, Some("c1")));
    t.conn(mirror_of(daemon(true), 2, Some("c1")));
    edit_keeps_the_pick(&mut t, "restart");
}

/// A fresh app whose first mirror lacks `FOH SPL` chooses `SPL 2`'s row.
#[test]
fn a_meter_pick_waits_for_a_meter_listed_after_the_row_is_chosen() {
    let mut t = T::new();
    t.conn(mirror(daemon(false)));
    t.st.apply(Msg::SelectMeas(MeasId(5)), &t.keys);
    t.conn(mirror(daemon(true)));
    edit_keeps_the_pick(&mut t, "late meter");
}

/// `FOH SPL` goes away for a while (deleted and made again under its id) with the row
/// chosen.
#[test]
fn a_meter_pick_waits_while_its_meter_is_gone() {
    let mut t = desk();
    t.conn(mirror(daemon(false)));
    t.conn(mirror(daemon(true)));
    edit_keeps_the_pick(&mut t, "meter gone a while");
}

/// An edit while `FOH SPL` is gone stores its pick, not the pane's stand-in.
#[test]
fn an_edit_while_the_meter_is_gone_keeps_its_pick() {
    let mut t = desk();
    t.conn(mirror(daemon(false)));
    let r = t.key("T");
    let l = stored(&r);
    assert!(!l.is_empty(), "T is an edit of SPL 2's panes: {r:?}");
    for l in l {
        assert_eq!(
            l.panes()[0].content,
            Some(wire::PaneContent::Meas { meas: MeasId(4) }),
            "{l:?}"
        );
    }
    answer(&mut t, &r);
    t.conn(mirror(daemon(true)));
    edit_keeps_the_pick(&mut t, "after an edit while gone");
}

/// A split of the waiting pane while `FOH SPL` is gone: both halves keep its pick, neither
/// takes the id of `SPL 2` that the pane shows meanwhile.
#[test]
fn a_split_while_the_meter_is_gone_keeps_its_pick_in_both_halves() {
    let mut t = T::new();
    t.conn(mirror(daemon(false)));
    t.st.apply(Msg::SelectMeas(MeasId(5)), &t.keys);
    let first = t.st.layout.root.reading_order()[0];
    t.st.apply(Msg::FocusPane(first), &t.keys);
    let r = t.key("N");
    let l = stored(&r);
    let l = l.last().expect("the split stored");
    let picks: Vec<_> = l.panes().iter().map(|p| p.content.clone()).collect();
    let foh = Some(wire::PaneContent::Meas { meas: MeasId(4) });
    assert_eq!(picks[..2], [foh.clone(), foh], "{l:?}");
    t.conn(mirror(daemon(true)));
    let mut now = shown(&t);
    now.sort();
    assert_eq!(now, [Some(MeasId(4)), Some(MeasId(4)), Some(MeasId(5))]);
}

//! The stream dialog and the Sessions dialog against a real daemon: a mic named, the
//! loopback detected, kinds and a pair; Save as by name.

use crate::harness::{Driver, NAME, R, connected, measure_from_empty};
use ac2_ui::forms::FormKind;
use ac2_ui::keys::CommandId;
use ac2_ui::state::{AppState, Msg, Overlay};

/// The stream dialog of the shown rig, if open.
fn dialog(s: &AppState) -> Option<&ac2_ui::stream_dialog::StreamDialog> {
    s.overlay.settings().map(|x| &x.stream)
}

/// Shift+O on a daemon without a stream: the dialog with the device and its four inputs
/// metered.
fn open_stream_dialog(d: &mut Driver) -> R {
    d.synced()?;
    d.key("Shift+O");
    d.send(Msg::Text("O".into()));
    d.until("the dialog with the device and its meters", |s| {
        dialog(s).is_some_and(|x| x.device_info().is_some()) && s.input_meters().len() == 4
    })
}

/// ↓ until the dialog's focus is on `row`.
fn down_to(d: &mut Driver, row: ac2_ui::stream_dialog::Row) -> R {
    for _ in 0..16 {
        if dialog(&d.st).is_some_and(|x| x.focus == row) {
            return Ok(());
        }
        d.key("Down");
    }
    Err(format!("row {row:?} never reached").into())
}

/// The New measurement chooser a fresh stream opens.
fn new_meas_chooser(s: &AppState) -> bool {
    matches!(&s.overlay, Overlay::Choose(c)
        if c.purpose == ac2_ui::state::ChoicePurpose::NewMeasurement)
}

/// From an empty daemon, in the stream dialog: in 2 named `M30 FOH` (N), the loopback
/// detected at a typed −30 dBFS (D, Enter: the daemon plays its burst and finds out 1 on in
/// 1, then gives the stimulus back); Enter opens the stream on the reference and the mic,
/// metering those two; the chooser's transfer measurement is named after the mic.
#[test]
fn a_mic_named_and_the_loopback_detected_from_the_stream_dialog() -> R {
    use ac2_ui::stream_dialog::{DetectPhase, Row};
    let (_daemon, mut d) = connected()?;
    open_stream_dialog(&mut d)?;
    down_to(&mut d, Row::Input(1))?;
    d.key("N");
    d.send(Msg::Text("n".into()));
    d.send(Msg::Text("M30 FOH".into()));
    d.key("Enter");
    let named = dialog(&d.st).ok_or("the dialog")?;
    assert_eq!(named.inputs[1].mic(), Some("M30 FOH"));
    assert!(named.edit.is_none());

    d.key("D");
    d.send(Msg::Text("d".into()));
    d.send(Msg::Text("-30".into()));
    assert_eq!(
        dialog(&d.st)
            .and_then(|x| x.detect.as_ref())
            .map(|p| p.level.as_str()),
        Some("-30")
    );
    d.key("Enter");
    d.until("the loopback detected", |s| {
        dialog(s).is_some_and(|x| {
            matches!(
                x.detect.as_ref().map(|p| &p.phase),
                Some(DetectPhase::Done(_))
            )
        })
    })?;
    let Some(DetectPhase::Done(found)) = dialog(&d.st)
        .and_then(|x| x.detect.as_ref())
        .map(|p| p.phase.clone())
    else {
        return Err("no detection".into());
    };
    assert_eq!(found.level, ac2_proto::units::Dbfs(-30.0));
    assert_eq!(
        found.ranked.first().map(|c| (found.output, c.input)),
        Some((0, 0)),
        "out 1 loops back to in 1: {found:?}"
    );
    d.until("the burst's lease given back", |s| {
        s.daemon().is_some_and(|x| x.generator.owner.is_none())
    })?;

    d.key("Enter");
    d.until("the stream, the chooser, its two inputs metered", |s| {
        s.open_stream().is_some() && new_meas_chooser(s) && s.input_meters().len() == 2
    })?;
    let open = d.st.open_stream().cloned().ok_or("stream")?;
    assert_eq!(open.config.input_channels, vec![0, 1]);
    assert_eq!(
        open.config.loopback.map(|l| (l.output, l.input)),
        Some((0, 0))
    );
    d.key("Enter");
    d.until(
        "the transfer form",
        |s| matches!(&s.overlay, Overlay::Form(f) if f.kind == FormKind::Transfer),
    )?;
    d.key("Enter");
    d.until("Reference → M30 FOH running", |s| {
        s.measurements()
            .iter()
            .any(|m| m.config.name == "Reference → M30 FOH" && m.running)
    })?;
    // The transfer dialog meters the stream's inputs too.
    d.send(Msg::Command(CommandId::NewTransfer));
    d.until("the transfer dialog with its meters", |s| {
        matches!(s.overlay, Overlay::Form(_)) && s.input_meters().len() == 2
    })?;
    d.key("Escape");
    assert_eq!(d.st.overlay, Overlay::None);
    Ok(())
}

/// Alt+S, Save as…: the name typed key by key (what the keys type, not what they do
/// elsewhere: W, Q, G, N and Space are all commands), Tab keeps it; Enter saves the open
/// session under that name, which the daemon then gives as the working session's origin.
#[test]
fn sessions_dialog_save_as() -> R {
    use ac2_proto::model::SessionOrigin;
    use ac2_ui::sessions::Page;
    // A session directory of the test's own: a save never lands in the operator's sessions.
    let dir = tempfile::tempdir()?;
    let daemon = ac2_e2e::sim_daemon_with(ac2_e2e::Setup::Empty, |c| {
        c.session_dir = dir.path().join("sessions");
    })?;
    let mut d = Driver::connect(daemon.client_config(NAME), &daemon.describe())?;
    measure_from_empty(&mut d)?;
    d.key("Alt+S");
    d.until("the dialog, listed", |s| {
        s.sessions_dialog().is_some_and(|x| x.daemon.is_some())
    })?;
    let save_as =
        d.st.sessions_rows(ac2_proto::units::WallNs(0))
            .iter()
            .position(|r| r.title == "Save as…")
            .ok_or("a Save as row")?;
    for _ in 0..save_as {
        d.key("Down");
    }
    d.key("Enter");
    assert!(
        d.st.sessions_dialog()
            .is_some_and(|x| matches!(x.page, Page::Name(_))),
        "{:?}",
        d.st.overlay
    );
    for _ in 0..40 {
        d.key("Backspace");
    }
    for (k, t) in [
        ("F", "F"),
        ("A", "a"),
        ("W", "w"),
        ("Q", "q"),
        ("G", "g"),
        ("N", "n"),
        ("Space", " "),
        ("2", "2"),
    ] {
        d.key(k);
        d.send(Msg::Text(t.into()));
    }
    d.key("Tab");
    {
        let Some(Page::Name(n)) = d.st.sessions_dialog().map(|x| &x.page) else {
            return Err(format!("{:?}", d.st.overlay).into());
        };
        assert_eq!(n.text, "Fawqgn 2");
    }
    d.key("Enter");
    d.until("saved under its name", |s| {
        s.working().origin
            == Some(SessionOrigin::Name {
                name: "Fawqgn 2".into(),
            })
            && !s.working().dirty
    })?;
    for _ in 0..2 {
        if d.st.sessions_dialog().is_some() {
            d.key("Escape");
        }
    }
    assert_eq!(d.st.overlay, Overlay::None);
    Ok(())
}

/// In the stream dialog: in 3 and in 4 paired (Shift+P on each), the pair named `Main`, A
/// makes both sides digital and L line again. Enter opens the stream with the pair: the
/// daemon keeps the kinds and the sides, the Inputs list shows the pair as one row. An SPL
/// meter on the line input (in 1) is refused: it needs a microphone.
#[test]
fn kinds_and_a_pair_from_the_stream_dialog() -> R {
    use ac2_proto::model::{InputKind, Pair, PairSide};
    use ac2_ui::prefs::KindPref;
    use ac2_ui::stream_dialog::Row;
    let (_daemon, mut d) = connected()?;
    open_stream_dialog(&mut d)?;
    let kinds = |d: &Driver, a: usize, b: usize| {
        dialog(&d.st).map(|x| (x.inputs[a].kind_pref(), x.inputs[b].kind_pref()))
    };
    assert_eq!(kinds(&d, 2, 3).map(|k| k.0), Some(KindPref::Line));
    down_to(&mut d, Row::Input(2))?;
    d.key("Shift+P");
    d.send(Msg::Text("P".into()));
    assert_eq!(dialog(&d.st).and_then(|x| x.pair_pick), Some(2));
    down_to(&mut d, Row::Input(3))?;
    d.key("Shift+P");
    d.send(Msg::Text("P".into()));
    {
        let x = dialog(&d.st).ok_or("the dialog")?;
        assert_eq!(x.pairs.len(), 1);
        assert!(x.inputs[2].in_stream && x.inputs[3].in_stream);
    }
    d.key("Up");
    d.key("N");
    d.send(Msg::Text("n".into()));
    d.send(Msg::Text("Main".into()));
    d.key("Enter");
    assert_eq!(
        dialog(&d.st)
            .and_then(|x| x.pairs[0].name.clone())
            .as_deref(),
        Some("Main")
    );
    d.key("A");
    assert_eq!(
        kinds(&d, 2, 3),
        Some((KindPref::Digital, KindPref::Digital))
    );
    d.key("L");
    assert_eq!(kinds(&d, 2, 3), Some((KindPref::Line, KindPref::Line)));
    assert_eq!(
        dialog(&d.st).map(|x| x.pairs.clone()),
        Some(vec![Pair {
            left: 2,
            right: 3,
            name: Some("Main".into()),
        }])
    );

    d.key("Enter");
    d.until(
        "the stream with the pair, the chooser, four inputs metered",
        |s| {
            s.daemon()
                .is_some_and(|x| x.stream.open.is_some() && x.pairs.len() == 1)
                && new_meas_chooser(s)
                && s.input_meters().len() == 4
        },
    )?;
    let state = d.st.daemon().ok_or("state")?.clone();
    let setup = |c: u16| state.inputs.iter().find(|i| i.channel == c).cloned();
    assert_eq!(setup(0).map(|i| i.kind), Some(InputKind::Line));
    assert!(matches!(
        setup(1).map(|i| i.kind),
        Some(InputKind::Mic { .. })
    ));
    assert_eq!(
        setup(2).map(|i| (i.kind, i.pair)),
        Some((InputKind::Line, Some(PairSide::Left)))
    );
    assert_eq!(setup(3).and_then(|i| i.pair), Some(PairSide::Right));
    assert_eq!(state.pairs[0].name.as_deref(), Some("Main"));
    d.key("Escape");
    assert_eq!(d.st.overlay, Overlay::None);
    let rows = d.st.stream_inputs();
    let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels.len(), 3, "{labels:?}");
    assert!(labels[0].ends_with(" · reference (in 1)"), "{labels:?}");
    assert!(labels[1].contains(" · mic (in 2)"), "{labels:?}");
    assert_eq!(labels[2], "Main · line · pair (in 3, 4)");
    assert_eq!(rows[2].right.as_ref().map(|(c, _)| *c), Some(3));

    d.key("Ctrl+K");
    d.send(Msg::Text("new spl".into()));
    d.key("Enter");
    assert!(
        matches!(&d.st.overlay, Overlay::Form(f)
            if f.kind == FormKind::Spl
                && f.channel(ac2_ui::forms::FieldId::Input) == Some(1)),
        "the SPL dialog on the mic: {:?}",
        d.st.overlay
    );
    d.key("Left");
    assert!(
        matches!(&d.st.overlay, Overlay::Form(f)
            if f.channel(ac2_ui::forms::FieldId::Input) == Some(0)),
        "the line input picked"
    );
    d.key("Enter");
    d.until("the refusal", |s| {
        s.toasts.list.iter().any(|t| {
            t.text
                .contains("input 1 is a line input; an SPL meter needs a microphone")
        })
    })?;
    assert!(d.st.measurements().is_empty());
    Ok(())
}

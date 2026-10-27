//! A pane's title strip in the accessibility tree, without a GPU: per-pane widgets stay
//! distinct after a split, and a long measurement name's chip stays inside its pane with the
//! whole name as its label.

use std::sync::Arc;
use std::time::Instant;

use ac2_client::fake::empty_state;
use ac2_client::{MirrorView, Phase};
use ac2_proto::model::*;
use ac2_proto::units::*;
use ac2_scene::theme::ThemeName;
use ac2_ui::conn::ConnEvent;
use ac2_ui::keys::{CommandId, Keymap};
use ac2_ui::state::{Layout, Msg, PaneKind};
use ac2_ui::{App, AppOptions};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{By, Queryable};

const W: f32 = 1280.0;

fn transfer(id: u32, name: &str) -> Measurement {
    Measurement {
        id: MeasId(id),
        config: MeasConfig {
            scope: ac2_proto::model::MeasScope::Session,
            name: name.into(),
            kind: MeasKind::Transfer {
                config: TransferConfig {
                    reference_input: 0,
                    measurement_input: 1,
                    averaging: TfAveraging::Exponential {
                        time_constant: Seconds(1.0),
                    },
                    resolution: Resolution::FortyEighth,
                    smoothing: None,
                    depth: DepthPolicy::EqualConfidence,
                },
            },
        },
        config_rev: Rev(1),
        running: false,
        delay: None,
        grid_id: None,
        colour: None,
        layout: None,
        compared: Vec::new(),
        offset: ac2_proto::units::Db(0.0),
        eq: Vec::new(),
    }
}

fn mirror(ms: Vec<Measurement>) -> Msg {
    let mut s = empty_state();
    s.measurements = ms;
    // The app as started: its rig not connected, so not yet named by its hello.
    Msg::Conn(
        ac2_ui::state::UNKNOWN_RIG,
        Box::new(ConnEvent::Mirror(Arc::new(MirrorView {
            client_id: Some(ClientId("pane-title".into())),
            phase: Phase::Live,
            incarnation: Some(DaemonIncarnation(1)),
            session_epoch: Some(SessionEpoch(1)),
            rev: Rev(1),
            state: Some(Arc::new(s)),
            last_ka: Some(Instant::now()),
            connected: Instant::now(),
            generator: None,
            timing: None,
            health: None,
            ka_rev: Some(Rev(1)),
            clock_offset_ns: Some(0),
            snapshots: 1,
            since_requests: 0,
            incarnation_changes: 0,
        }))),
    )
}

fn harness() -> Harness<'static, App> {
    Harness::builder()
        .with_size(egui::vec2(W, 800.0))
        .with_pixels_per_point(1.0)
        .build_eframe(|cc| {
            App::new(
                cc,
                AppOptions {
                    target: None,
                    theme: ThemeName::Dark,
                    keymap: Keymap::default(),
                    keymap_path: None,
                    prefs: Default::default(),
                    prefs_path: None,
                    notices: vec![],
                    started: Instant::now(),
                    bench_startup: false,
                    smoke: false,
                    open_session_dialog: false,
                    embedded_autosave: None,
                    client_key: None,
                    connect: None,
                    rasterizer: None,
                },
            )
        })
}

/// The chips naming `name` in the panes (the measurement list's row sits left of them).
fn chips(h: &Harness<'static, App>, name: &str, panes_left: f32) -> Vec<egui::Rect> {
    h.query_all(By::new().label(name))
        .map(|n| n.rect())
        .filter(|r| r.min.x > panes_left)
        .collect()
}

#[test]
fn split_sweep_panes_have_their_own_unit_controls() {
    let mut h = harness();
    h.state_mut().state.layout = Layout::single(PaneKind::Distortion);
    h.state_mut().state.pane_area = (1000.0, 600.0);
    h.state_mut().dispatch(Msg::Command(CommandId::SplitPane));
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(h.state().state.layout.panes().len(), 2);
    let db: Vec<egui::Rect> = h
        .query_all(By::new().label("dB"))
        .map(|n| n.rect())
        .collect();
    assert_eq!(db.len(), 2, "one dB control per pane: {db:?}");
    assert_eq!(h.query_all(By::new().label("%")).count(), 2);
    assert!(!db[0].intersects(db[1]), "{db:?}");
}

#[test]
fn long_name_chip_stays_in_its_pane() {
    let mut h = harness();
    let name = "Long measurement name ".repeat(20);
    let name = name.trim_end();
    h.state_mut().dispatch(mirror(vec![transfer(1, name)]));
    h.state_mut().dispatch(Msg::SelectMeas(MeasId(1)));
    for _ in 0..4 {
        h.step();
    }
    let one = chips(&h, name, 230.0);
    assert_eq!(
        one.len(),
        1,
        "the chip, labelled with the whole name: {one:?}"
    );
    assert!(one[0].max.x <= W, "chip past the window: {:?}", one[0]);

    // Split side by side: each pane's chip ends inside its own pane, before the next starts.
    h.state_mut().state.layout.focus = h.state().state.layout.panes()[0];
    h.state_mut().dispatch(Msg::Command(CommandId::SplitPane));
    for _ in 0..4 {
        h.step();
    }
    let mut two = chips(&h, name, 230.0);
    assert_eq!(two.len(), 2, "{two:?}");
    two.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
    assert!(two[0].min.y == two[1].min.y, "panes side by side: {two:?}");
    assert!(two[0].max.x < two[1].min.x, "chips overlap: {two:?}");
    assert!(two[1].max.x <= W, "chip past the window: {:?}", two[1]);
}

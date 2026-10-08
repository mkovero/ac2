//! The connect dialog: lists the local daemon, embedded daemons and discovered rigs (only a
//! paired rig is connectable), and choosing the simulated rig starts an embedded daemon and
//! connects to it. Same GPU requirements as `tests/it/ui.rs`.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use ac2_client::KeyDir;
use ac2_discovery::{Rig, Update};
use ac2_scene::theme::ThemeName;
use ac2_ui::connect::ConnectDialog;
use ac2_ui::embedded::EmbeddedBackend;
use ac2_ui::keys::Keymap;
use ac2_ui::state::ConnState;
use ac2_ui::{App, AppOptions};
use ac2_zmq::PublicKey;
use eframe::egui;
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, SnapshotOptions};

fn have_gpu(test: &str) -> bool {
    match ac2_plot::Gpu::new() {
        Ok(_) => true,
        Err(e) if std::env::var("AC2_REQUIRE_GPU").is_ok_and(|v| v == "1") => {
            panic!("{test}: {e} (AC2_REQUIRE_GPU=1)")
        }
        Err(e) => {
            eprintln!("SKIP {test}: {e}");
            false
        }
    }
}

fn rig(name: &str, key: &PublicKey, addr: &str, port: u16) -> Rig {
    let fp = key.fingerprint();
    let name_s = name.to_owned();
    let txt = move |k: &str| -> Option<String> {
        match k {
            "txtvers" => Some("1".into()),
            "name" => Some(name_s.clone()),
            "v" => Some("1.0.0".into()),
            "proto" => Some(ac2_proto::PROTO_VERSION.to_string()),
            "fp" => Some(fp.clone()),
            _ => None,
        }
    };
    let addrs: Vec<IpAddr> = addr.parse().into_iter().collect();
    Rig::from_parts(
        &format!("{name}._ac2._tcp.local."),
        "rig.local.",
        port,
        addrs,
        txt,
    )
    .unwrap_or_else(|e| panic!("{e}"))
}

fn harness(dialog: ConnectDialog) -> Harness<'static, App> {
    let opts = AppOptions {
        target: None,
        theme: ThemeName::Dark,
        keymap: Keymap::default(),
        keymap_path: None,
        prefs: ac2_ui::prefs::UiPrefs::default(),
        prefs_path: None,
        notices: vec![],
        started: Instant::now(),
        bench_startup: false,
        open_session_dialog: false,
        client_key: None,
        connect: None,
    };
    let mut dialog = Some(dialog);
    Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(move |cc| {
            let mut app = App::new(cc, opts);
            if let Some(d) = dialog.take() {
                app.open_connect(d);
            }
            app
        })
}

#[test]
fn connect_dialog_lists_rigs_and_starts_the_simulated_rig() {
    if !have_gpu("connect_dialog") {
        return;
    }
    ac2_ui::keys::set_label_style(ac2_ui::keys::LabelStyle::Pc);
    let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let kd = KeyDir::new(tmp.path());
    let paired = PublicKey::from_bytes([7; 32]);
    let stranger = PublicKey::from_bytes([9; 32]);
    kd.ensure_client_keypair().unwrap_or_else(|e| panic!("{e}"));
    kd.pin_server("10.0.0.20", paired)
        .unwrap_or_else(|e| panic!("{e}"));
    let mut d = ConnectDialog::new(
        kd,
        "ac2-ui test",
        vec![EmbeddedBackend::Cpal, EmbeddedBackend::Fake],
        false,
        None,
    );
    d.table.apply(Update::Resolved(rig(
        "FOH rack",
        &paired,
        "10.0.0.20",
        47_820,
    )));
    d.table.apply(Update::Resolved(rig(
        "Stage left",
        &stranger,
        "10.0.0.31",
        47_820,
    )));
    let mut h = harness(d);
    for _ in 0..4 {
        h.step();
    }
    h.snapshot_options(
        "connect_dialog",
        &SnapshotOptions::new()
            .threshold(1.0)
            .max_failed_pixels(egui_kittest::OsThreshold::new(0).macos(16)),
    );

    h.get_by_label("Simulated rig (no audio)").click();
    h.step();
    h.get_by_label("Connect").click();
    let t0 = Instant::now();
    loop {
        h.step();
        let app = h.state();
        if !app.connect_open() && matches!(app.state.conn, ConnState::Connected { .. }) {
            break;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(15),
            "embedded fake daemon did not connect"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

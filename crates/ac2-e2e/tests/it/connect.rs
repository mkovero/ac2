//! The connect dialog's *Simulated rig* starts the `ac2d` beside the app, as the app runs it
//! (`ac2d --backend fake --ephemeral` on loopback port 0), connects and shows it measuring;
//! the app owns it, so it stops when the app goes. Needs the `ac2d` binary built (a
//! workspace build or `cargo build -p ac2d`) and the GPU `ac2-ui`'s window tests need.

use std::time::{Duration, Instant};

use ac2_client::KeyDir;
use ac2_scene::theme::ThemeName;
use ac2_ui::connect::ConnectDialog;
use ac2_ui::keys::Keymap;
use ac2_ui::state::ConnState;
use ac2_ui::{App, AppOptions};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

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

/// The `ac2d` the app finds beside its executable, put beside this test binary: the build's
/// own (`target/<profile>/ac2d`, a level above the test binaries). `false` when it was not
/// built.
fn ac2d_beside_the_test(test: &str) -> bool {
    let exe = std::env::current_exe().unwrap_or_else(|e| panic!("{e}"));
    let name = ac2_ui::spawn::exe_name();
    let built = exe
        .parent()
        .and_then(|deps| deps.parent())
        .map(|profile| profile.join(&name));
    let Some(built) = built.filter(|p| p.is_file()) else {
        eprintln!("SKIP {test}: no {name} built (cargo build -p ac2d)");
        return false;
    };
    std::fs::copy(&built, ac2_ui::spawn::beside(&exe)).unwrap_or_else(|e| panic!("{e}"));
    true
}

#[test]
fn the_simulated_rig_is_an_ac2d_the_app_starts() {
    let test = "the_simulated_rig_is_an_ac2d_the_app_starts";
    if !have_gpu(test) || !ac2d_beside_the_test(test) {
        return;
    }
    let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let d = ConnectDialog::new(KeyDir::new(tmp.path()), "ac2-e2e test", false, None);
    let opts = AppOptions {
        target: None,
        target_addr: None,
        start: None,
        at_exit: None,
        theme: ThemeName::Dark,
        keymap: Keymap::default(),
        keymap_path: None,
        prefs: crate::common::grid_ui_prefs(),
        prefs_path: None,
        notices: vec![],
        started: Instant::now(),
        bench_startup: false,
        smoke: false,
        open_session_dialog: false,
        client_key: None,
        connect: None,
        rasterizer: Some(ac2_plot::Rasterizer::Gpu),
    };
    let mut dialog = Some(d);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(move |cc| {
            let mut app = App::new(cc, opts);
            app.health_measured = false;
            if let Some(d) = dialog.take() {
                app.open_connect(d);
            }
            app
        });
    h.step();
    h.get_by_label("Simulated rig (no audio)").click();
    h.step();
    h.get_by_label("Connect").click();
    let t0 = Instant::now();
    loop {
        h.step();
        let app = h.state();
        if !app.connect_open()
            && matches!(app.state.rig.conn, ConnState::Connected { .. })
            && app.state.open_session().is_some()
        {
            break;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "the simulated rig did not connect: {:?} {:?}",
            app.state.rig.conn,
            app.state
                .toasts
                .list
                .iter()
                .map(|t| &t.text)
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let app = h.state();
    assert_eq!(app.state.rig_name(app.state.active()), "Simulated rig");
    // Another rig each run: never remembered.
    assert_eq!(app.state.rig.addr, None);
    assert!(app.state.rig.quit_loses);
}

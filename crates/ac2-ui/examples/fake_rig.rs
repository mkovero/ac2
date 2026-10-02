//! The desktop UI against an in-process fake daemon publishing synthetic frames: two
//! transfer functions with an IR, a spectrum and an SPL meter. No audio anywhere; the fake's
//! generator is bookkeeping only.
//!
//! ```text
//! cargo run -p ac2-ui --example fake_rig [-- --bench-startup] [-- --theme light]
//! ```

#[path = "../tests/common/mod.rs"]
mod common;

use std::time::Instant;

use ac2_client::ClientConfig;
use ac2_scene::theme::ThemeName;
use ac2_ui::conn::Target;
use ac2_ui::keys::{Keymap, config_path};
use ac2_ui::{App, AppOptions};
use eframe::egui;

fn main() -> Result<(), eframe::Error> {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().collect();
    let has = |a: &str| args.iter().any(|x| x == a);
    let theme = match args
        .iter()
        .position(|a| a == "--theme")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
    {
        Some("light") => ThemeName::Light,
        Some("high-contrast") => ThemeName::HighContrast,
        _ => ThemeName::Dark,
    };
    let rig = common::Rig::start();
    let keymap_path = config_path();
    let (keymap, err) = Keymap::load(Some(&keymap_path));
    let opts = AppOptions {
        target: Some(Target {
            config: ClientConfig::new(rig.fake.endpoints(), "ac2-ui fake rig"),
            describe: "fake rig".into(),
        }),
        theme,
        keymap,
        keymap_path: Some(keymap_path),
        // The fake rig's device is not hardware worth remembering outputs for.
        prefs: ac2_ui::prefs::UiPrefs::default(),
        prefs_path: None,
        notices: err.into_iter().collect(),
        started,
        bench_startup: has("--bench-startup"),
    };
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ac2 (fake rig)")
            .with_inner_size([1280.0, 800.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let r = eframe::run_native(
        "ac2-fake-rig",
        native,
        Box::new(move |cc| Ok(Box::new(App::new(cc, opts)))),
    );
    drop(rig);
    r
}

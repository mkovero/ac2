//! `ac2-ui`: the desktop app.

use std::process::ExitCode;
use std::time::Instant;

use ac2_client::{ClientConfig, Endpoints, KeyDir, RemoteAddr};
use ac2_scene::theme::ThemeName;
use ac2_ui::conn::Target;
use ac2_ui::keys::{Keymap, config_path};
use ac2_ui::{App, AppOptions};
use clap::{Parser, ValueEnum};
use eframe::egui;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ThemeArg {
    Dark,
    Light,
    HighContrast,
}

/// ac2 desktop analyzer UI.
#[derive(Debug, Parser)]
#[command(name = "ac2-ui", version)]
struct Args {
    /// Connect to a remote daemon over CURVE: `host` or `host:port`.
    #[arg(long, value_name = "HOST[:PORT]", conflicts_with_all = ["ctrl", "embedded"])]
    remote: Option<RemoteAddr>,
    /// Explicit ctrl endpoint (with --data), e.g. `tcp://127.0.0.1:47820`.
    #[arg(long, requires = "data", value_name = "ENDPOINT")]
    ctrl: Option<String>,
    /// Explicit data endpoint (with --ctrl).
    #[arg(long, requires = "ctrl", value_name = "ENDPOINT")]
    data: Option<String>,
    /// Host an embedded daemon instead of connecting to one.
    #[arg(long, conflicts_with = "ctrl")]
    embedded: bool,
    /// Key directory for CURVE (default: the per-user ac2 config dir).
    #[arg(long, value_name = "DIR")]
    key_dir: Option<std::path::PathBuf>,
    /// Key bindings file (default: `~/.config/ac2/keys.toml`).
    #[arg(long, value_name = "FILE")]
    keys: Option<std::path::PathBuf>,
    #[arg(long, value_enum, default_value = "dark")]
    theme: ThemeArg,
    /// Print the time to the first presented frame and exit.
    #[arg(long)]
    bench_startup: bool,
}

const NAME: &str = concat!("ac2-ui ", env!("CARGO_PKG_VERSION"));

fn target(a: &Args, notices: &mut Vec<String>) -> Result<Target, String> {
    if let Some(r) = &a.remote {
        let kd = KeyDir::new(
            a.key_dir
                .clone()
                .unwrap_or_else(ac2_client::keys::default_key_dir),
        );
        let curve = kd.curve_client(&r.host).map_err(|e| e.to_string())?;
        let mut config = ClientConfig::new(Endpoints::remote(r), NAME);
        config.curve = Some(curve);
        return Ok(Target {
            config,
            describe: format!("daemon at {r}"),
        });
    }
    if let (Some(c), Some(d)) = (&a.ctrl, &a.data) {
        return Ok(Target {
            config: ClientConfig::new(
                Endpoints {
                    ctrl: c.clone(),
                    data: d.clone(),
                },
                NAME,
            ),
            describe: format!("daemon at {c}"),
        });
    }
    if a.embedded {
        match ac2_ui::embedded::start_embedded() {
            Ok(ep) => {
                return Ok(Target {
                    config: ClientConfig::new(ep, NAME),
                    describe: "embedded daemon".into(),
                });
            }
            Err(e) => notices.push(format!("embedded daemon: {e}")),
        }
    }
    Ok(Target {
        config: ClientConfig::local(NAME),
        describe: "local daemon".into(),
    })
}

fn main() -> ExitCode {
    let started = Instant::now();
    let args = Args::parse();
    let mut notices = Vec::new();
    let target = match target(&args, &mut notices) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ac2-ui: {e}");
            return ExitCode::from(2);
        }
    };
    let keymap_path = args.keys.clone().or_else(config_path);
    let (keymap, err) = Keymap::load(keymap_path.as_deref());
    notices.extend(err);
    let theme = match args.theme {
        ThemeArg::Dark => ThemeName::Dark,
        ThemeArg::Light => ThemeName::Light,
        ThemeArg::HighContrast => ThemeName::HighContrast,
    };
    let opts = AppOptions {
        target: Some(target),
        theme,
        keymap,
        keymap_path,
        notices,
        started,
        bench_startup: args.bench_startup,
    };
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ac2")
            .with_app_id("ac2")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([720.0, 480.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    match eframe::run_native(
        "ac2",
        native,
        Box::new(move |cc| Ok(Box::new(App::new(cc, opts)))),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ac2-ui: {e}");
            ExitCode::FAILURE
        }
    }
}

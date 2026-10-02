//! `ac2-ui`: the desktop app.

use std::process::ExitCode;
use std::time::Instant;

use ac2_client::{ClientConfig, Endpoints, KeyDir, RemoteAddr};
use ac2_scene::theme::ThemeName;
use ac2_ui::conn::Target;
use ac2_ui::embedded::{Embedded, EmbeddedBackend, start_embedded};
use ac2_ui::keys::{Keymap, config_path};
use ac2_ui::prefs::UiPrefs;
use ac2_ui::{App, AppOptions};
use clap::{Parser, ValueEnum};
use eframe::egui;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum BackendArg {
    /// Platform audio (ALSA / CoreAudio / WASAPI).
    Cpal,
    /// JACK (Linux, feature `jack`).
    Jack,
    /// Simulated rig: out 1 → in 1 loopback, out 1 → in 2 acoustic path. No real audio.
    Fake,
}

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
    /// Audio backend of the embedded daemon (fake only when named here).
    #[arg(long, value_enum, default_value = "cpal", requires = "embedded")]
    backend: BackendArg,
    /// Key directory for CURVE (default: the per-user ac2 config dir).
    #[arg(long, value_name = "DIR")]
    key_dir: Option<std::path::PathBuf>,
    /// Key bindings file (default: `keys.toml` in the ac2 config directory, `~/.config/ac2` on
    /// Linux).
    #[arg(long, value_name = "FILE")]
    keys: Option<std::path::PathBuf>,
    #[arg(long, value_enum, default_value = "dark")]
    theme: ThemeArg,
    /// Print the time to the first presented frame and exit.
    #[arg(long)]
    bench_startup: bool,
}

const NAME: &str = concat!("ac2-ui ", env!("CARGO_PKG_VERSION"));

fn target(
    a: &Args,
    embedded: &mut Option<Embedded>,
    notices: &mut Vec<String>,
) -> Result<Target, String> {
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
        let backend = match a.backend {
            BackendArg::Cpal => EmbeddedBackend::Cpal,
            BackendArg::Jack => EmbeddedBackend::Jack,
            BackendArg::Fake => EmbeddedBackend::Fake,
        };
        // An embedded daemon that cannot start is an error, never a silent switch to
        // another daemon (the operator asked for this one, on this backend).
        let e = start_embedded(backend).map_err(|e| format!("embedded daemon: {e}"))?;
        let t = Target {
            config: ClientConfig::new(e.endpoints(), NAME),
            describe: e.describe(),
        };
        notices.push(format!("{} running in this process", e.describe()));
        *embedded = Some(e);
        return Ok(t);
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
    // Lives until the window closes; dropping it shuts the daemon down.
    let mut embedded = None;
    let target = match target(&args, &mut embedded, &mut notices) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ac2-ui: {e}");
            return ExitCode::from(2);
        }
    };
    let keymap_path = Some(args.keys.clone().unwrap_or_else(config_path));
    let (keymap, err) = Keymap::load(keymap_path.as_deref());
    notices.extend(err);
    let theme = match args.theme {
        ThemeArg::Dark => ThemeName::Dark,
        ThemeArg::Light => ThemeName::Light,
        ThemeArg::HighContrast => ThemeName::HighContrast,
    };
    let prefs_path = ac2_paths::ui_prefs();
    let (prefs, err) = UiPrefs::load(Some(&prefs_path));
    notices.extend(err);
    let opts = AppOptions {
        target: Some(target),
        theme,
        keymap,
        keymap_path,
        prefs,
        prefs_path: Some(prefs_path),
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
    let r = eframe::run_native(
        "ac2",
        native,
        Box::new(move |cc| Ok(Box::new(App::new(cc, opts)))),
    );
    // The app (and its link, which stops our stimulus) is gone; now the daemon.
    drop(embedded);
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ac2-ui: {e}");
            ExitCode::FAILURE
        }
    }
}

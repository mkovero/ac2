//! `ac2-ui`: the desktop app.

use std::process::ExitCode;
use std::time::Instant;

use ac2_client::{ClientConfig, Endpoints, KeyDir, RemoteAddr};
use ac2_scene::theme::ThemeName;
use ac2_ui::conn::Target;
use ac2_ui::connect::ConnectDialog;
use ac2_ui::embedded::{Embedded, EmbeddedBackend, start_embedded};
use ac2_ui::keys::{Keymap, config_path};
use ac2_ui::prefs::UiPrefs;
use ac2_ui::{App, AppOptions};
use clap::{Parser, ValueEnum};
use eframe::egui;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum BackendArg {
    /// The Core Audio HAL directly; macOS.
    #[value(name = "coreaudio")]
    CoreAudio,
    /// JACK (JACK2, or PipeWire through pipewire-jack); Linux.
    Jack,
    /// ASIO drivers; Windows.
    Asio,
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
    /// Audio backend of the embedded daemon: this platform's audio (jack on Linux,
    /// coreaudio on macOS, asio on Windows) unless named; fake only when named here.
    #[arg(long, value_enum, requires = "embedded")]
    backend: Option<BackendArg>,
    /// Key directory for CURVE (default: the per-user ac2 config dir).
    #[arg(long, value_name = "DIR")]
    key_dir: Option<std::path::PathBuf>,
    /// Key bindings file (default: `keys.toml` in the ac2 config directory, `~/.config/ac2` on
    /// Linux).
    #[arg(long, value_name = "FILE")]
    keys: Option<std::path::PathBuf>,
    /// Theme for this run (default: the one last chosen in Settings › Display, else dark).
    #[arg(long, value_enum)]
    theme: Option<ThemeArg>,
    /// Open the connect dialog (local daemon, embedded, or a rig found on the network). Also
    /// opens on its own when no target is given and no local daemon is running.
    #[arg(long, conflicts_with_all = ["remote", "ctrl", "embedded"])]
    connect: bool,
    /// Do not browse the network for rigs in the connect dialog.
    #[arg(long)]
    no_discovery: bool,
    /// Print the time to the first presented frame and exit.
    #[arg(long)]
    bench_startup: bool,
    /// Wait for the daemon's device list, print it ("smoke: ok" last) and exit; a link
    /// that brings none within a minute prints "smoke: failed". For scripted checks.
    #[arg(long)]
    smoke: bool,
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
            None => EmbeddedBackend::platform(),
            Some(BackendArg::CoreAudio) => EmbeddedBackend::CoreAudio,
            Some(BackendArg::Jack) => EmbeddedBackend::Jack,
            Some(BackendArg::Asio) => EmbeddedBackend::Asio,
            Some(BackendArg::Fake) => EmbeddedBackend::Fake,
        };
        // An embedded daemon that cannot start is an error, never a silent switch to
        // another daemon (the operator asked for this one, on this backend).
        let e = start_embedded(backend).map_err(|e| format!("embedded daemon: {e}"))?;
        let t = Target {
            config: e.client_config(NAME),
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

fn embedded_backends() -> Vec<EmbeddedBackend> {
    if !cfg!(feature = "embedded") {
        return Vec::new();
    }
    vec![EmbeddedBackend::platform(), EmbeddedBackend::Fake]
}

fn main() -> ExitCode {
    let started = Instant::now();
    let args = Args::parse();
    let _timer = fine_timer::begin();
    // A GUI launch has no terminal, so the file is what a tester can send back; the embedded
    // daemon logs through the same subscriber.
    if let Some(path) = ac2_paths::log::init("ac2-ui") {
        tracing::info!("{NAME}: logging to {}", path.display());
    }
    ac2_paths::log::install_panic_hook("ac2-ui");
    let mut notices = Vec::new();
    // Lives until the window closes; dropping it shuts the daemon down.
    let mut embedded = None;
    let explicit = args.remote.is_some() || args.ctrl.is_some() || args.embedded;
    let local_running = ac2_ui::connect::local_daemon_running();
    let dialog = (args.connect || (!explicit && !local_running)).then(|| {
        let kd = KeyDir::new(
            args.key_dir
                .clone()
                .unwrap_or_else(ac2_client::keys::default_key_dir),
        );
        let mdns = ac2_discovery::Options::default();
        ConnectDialog::new(
            kd,
            NAME,
            embedded_backends(),
            local_running,
            (!args.no_discovery).then_some(&mdns),
        )
    });
    let target = if dialog.is_some() {
        None
    } else {
        match target(&args, &mut embedded, &mut notices) {
            Ok(t) => Some(t),
            // The window opens anyway, saying why it is not connected: a program that
            // exits here is restarted into the same error by its unit.
            Err(e) => {
                tracing::error!("ac2-ui: {e}");
                notices.push(e);
                None
            }
        }
    };
    let keymap_path = Some(args.keys.clone().unwrap_or_else(config_path));
    let (keymap, err) = Keymap::load(keymap_path.as_deref());
    notices.extend(err);
    let prefs_path = ac2_paths::ui_prefs();
    let (prefs, err) = UiPrefs::load(Some(&prefs_path));
    notices.extend(err);
    let theme = match args.theme {
        Some(ThemeArg::Dark) => ThemeName::Dark,
        Some(ThemeArg::Light) => ThemeName::Light,
        Some(ThemeArg::HighContrast) => ThemeName::HighContrast,
        None => prefs.theme.unwrap_or(ThemeName::Dark),
    };
    let key_dir = args
        .key_dir
        .clone()
        .unwrap_or_else(ac2_client::keys::default_key_dir);
    // Shown, never made here: a key is made when the operator pairs with a rig.
    let client_key = KeyDir::new(key_dir.clone())
        .client_keypair()
        .ok()
        .map(|kp| ac2_ui::settings::ClientKey {
            fingerprint: kp.public.fingerprint(),
            key: kp.public.to_z85(),
        });
    let opts = AppOptions {
        target,
        theme,
        keymap,
        keymap_path,
        prefs,
        prefs_path: Some(prefs_path),
        notices,
        started,
        bench_startup: args.bench_startup,
        smoke: args.smoke,
        // A daemon without an audio I/O (an embedded one on real audio always starts
        // so) lands on the Sessions dialog once; one with audio open (a rig, a kiosk) does not.
        // The simulated rig starts measuring.
        open_session_dialog: !(embedded.is_some()
            && matches!(args.backend, Some(BackendArg::Fake))),
        embedded_autosave: embedded.as_ref().map(Embedded::autosaves),
        client_key,
        connect: Some(ac2_ui::app::ConnectSetup {
            key_dir,
            client_name: NAME.into(),
            backends: embedded_backends(),
            discovery: !args.no_discovery,
        }),
        rasterizer: None,
    };
    // The window as last left; the app makes it fit the screen if that changed.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("ac2")
        .with_app_id("ac2")
        .with_inner_size(
            opts.prefs
                .window
                .map_or([1280.0, 800.0], |w| [w.width as f32, w.height as f32]),
        )
        .with_min_inner_size([
            ac2_ui::prefs::WindowPrefs::MIN.0 as f32,
            ac2_ui::prefs::WindowPrefs::MIN.1 as f32,
        ]);
    if let Some((x, y)) = opts.prefs.window.and_then(|w| w.pos) {
        viewport = viewport.with_position([x as f32, y as f32]);
    }
    let native = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: ac2_ui::gpu::wgpu_options(),
        ..Default::default()
    };
    let r = eframe::run_native(
        "ac2",
        native,
        Box::new(move |cc| {
            let mut app = App::new(cc, opts);
            if let Some(d) = dialog {
                app.open_connect(d);
            }
            Ok(Box::new(app))
        }),
    );
    // The app (and its link, which stops our stimulus) is gone; now the daemon.
    drop(embedded);
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("ac2-ui: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Millisecond timer resolution for the app's lifetime (Windows; elsewhere nothing).
///
/// Windows ends a timed wait that is not a high-resolution waitable timer on its 15.6 ms
/// tick. The link's display clock (33 ms) and the client's data poll (8 ms) are tokio
/// timers, which wait in the I/O completion port with a millisecond timeout and so on that
/// tick: frames would be fetched 33 to 47 ms apart, unevenly, and each up to 16 ms late.
/// An embedded daemon asks the same; a remote or local one does not cover this process.
mod fine_timer {
    #[cfg(windows)]
    pub(crate) struct Guard(bool);

    #[cfg(windows)]
    #[allow(unsafe_code)]
    pub(crate) fn begin() -> Guard {
        // SAFETY: no pointers; ended in `Drop` only when it succeeded.
        Guard(unsafe { windows_sys::Win32::Media::timeBeginPeriod(1) } == 0)
    }

    #[cfg(windows)]
    #[allow(unsafe_code)]
    impl Drop for Guard {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: pairs with the successful `timeBeginPeriod(1)` above.
                unsafe {
                    windows_sys::Win32::Media::timeEndPeriod(1);
                }
            }
        }
    }

    #[cfg(not(windows))]
    pub(crate) struct Guard;

    #[cfg(not(windows))]
    pub(crate) fn begin() -> Guard {
        Guard
    }
}

//! `ac2-ui`: the desktop app.

use std::process::ExitCode;
use std::time::Instant;

use ac2_client::{ClientConfig, Endpoints, KeyDir, RemoteAddr};
use ac2_proto::model::BackendKind;
use ac2_scene::theme::ThemeName;
use ac2_ui::conn::Target;
use ac2_ui::connect::{Choice, ConnectDialog};
use ac2_ui::keys::{Keymap, config_path};
use ac2_ui::prefs::RigAddr;
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
    /// Connect to a remote daemon over CURVE: `host` or `host:port`. Starts no ac2d.
    #[arg(long, value_name = "HOST[:PORT]", conflicts_with_all = ["ctrl", "backend"])]
    remote: Option<RemoteAddr>,
    /// Explicit ctrl endpoint (with --data), e.g. `tcp://127.0.0.1:47820`. Starts no ac2d.
    #[arg(
        long,
        requires = "data",
        value_name = "ENDPOINT",
        conflicts_with = "backend"
    )]
    ctrl: Option<String>,
    /// Explicit data endpoint (with --ctrl).
    #[arg(long, requires = "ctrl", value_name = "ENDPOINT")]
    data: Option<String>,
    /// This computer's ac2d on this backend: started as `ac2d --backend NAME` unless one
    /// runs, and one running on another backend refused. `fake`: the simulated rig.
    #[arg(long, value_enum)]
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
    /// Open the connect dialog (this computer, the simulated rig, or a rig found on the
    /// network) instead of connecting at start.
    #[arg(long, conflicts_with_all = ["remote", "ctrl", "backend"])]
    connect: bool,
    /// Do not browse the network for rigs in the connect dialog.
    #[arg(long)]
    no_discovery: bool,
    /// Print the time to the first presented frame and exit.
    #[arg(long)]
    bench_startup: bool,
    /// Wait for the daemon's device list, print it ("smoke: ok" last) and exit; a link
    /// that brings none within a minute, or a daemon that did not start, prints "smoke:
    /// failed". For scripted checks.
    #[arg(long)]
    smoke: bool,
}

const NAME: &str = concat!("ac2-ui ", env!("CARGO_PKG_VERSION"));

/// The daemon named by `--remote` or `--ctrl/--data`, if any.
fn target(a: &Args) -> Option<Result<(Target, RigAddr), String>> {
    if let Some(r) = &a.remote {
        let kd = KeyDir::new(
            a.key_dir
                .clone()
                .unwrap_or_else(ac2_client::keys::default_key_dir),
        );
        return Some(
            kd.curve_client(&r.host)
                .map_err(|e| e.to_string())
                .map(|curve| {
                    let mut config = ClientConfig::new(Endpoints::remote(r), NAME);
                    let addr = RigAddr::Network {
                        host: r.host.clone(),
                        port: r.port,
                        fingerprint: Some(curve.server_key.fingerprint()),
                    };
                    config.curve = Some(curve);
                    let t = Target {
                        config,
                        describe: format!("daemon at {r}"),
                    };
                    (t, addr)
                }),
        );
    }
    let (c, d) = (a.ctrl.as_ref()?, a.data.as_ref()?);
    Some(Ok((
        Target {
            config: ClientConfig::new(
                Endpoints {
                    ctrl: c.clone(),
                    data: d.clone(),
                },
                NAME,
            ),
            describe: format!("daemon at {c}"),
        },
        RigAddr::Endpoints {
            ctrl: c.clone(),
            data: d.clone(),
        },
    )))
}

/// `--backend`: this computer's ac2d on it, or the simulated rig.
fn backend_choice(b: BackendArg) -> Choice {
    let kind = match b {
        BackendArg::CoreAudio => BackendKind::CoreAudio,
        BackendArg::Jack => BackendKind::Jack,
        BackendArg::Asio => BackendKind::Asio,
        BackendArg::Fake => return Choice::Simulated,
    };
    Choice::Local {
        backend: Some(kind),
    }
}

fn main() -> ExitCode {
    let started = Instant::now();
    let args = Args::parse();
    let _timer = fine_timer::begin();
    // A GUI launch has no terminal, so the file is what a tester can send back; the daemon
    // logs to its own file (`ac2d.log`).
    if let Some(path) = ac2_paths::log::init("ac2-ui") {
        tracing::info!("{NAME}: logging to {}", path.display());
    }
    ac2_paths::log::install_panic_hook("ac2-ui");
    let mut notices = Vec::new();
    let prefs_path = ac2_paths::ui_prefs();
    let (prefs, err) = UiPrefs::load(Some(&prefs_path));
    notices.extend(err);
    if prefs.app_daemon_dropped {
        notices
            .push("the app's own daemon is gone; this computer's ac2d is connected instead".into());
    }
    let named = target(&args);
    // The rigs held last time are connected again (by the app); this computer's ac2d among
    // them when it was.
    let remembered =
        named.is_none() && args.backend.is_none() && !args.connect && !prefs.rigs.is_empty();
    let local_remembered = prefs.rigs.iter().any(|r| r.addr == RigAddr::Local);
    let dialog = args.connect.then(|| {
        let kd = KeyDir::new(
            args.key_dir
                .clone()
                .unwrap_or_else(ac2_client::keys::default_key_dir),
        );
        let mdns = ac2_discovery::Options::default();
        ConnectDialog::new(
            kd,
            NAME,
            ac2_ui::connect::local_daemon_running(),
            (!args.no_discovery).then_some(&mdns),
        )
    });
    let mut start = None;
    let target = match named {
        Some(Ok(t)) => Some(t),
        // The window opens anyway, saying why it is not connected: a program that exits
        // here is restarted into the same error by its unit.
        Some(Err(e)) => {
            tracing::error!("ac2-ui: {e}");
            notices.push(e);
            None
        }
        None => {
            if let Some(b) = args.backend {
                start = Some(backend_choice(b));
            } else if !args.connect
                && (!remembered || (prefs.app_daemon_dropped && !local_remembered))
            {
                start = Some(Choice::Local { backend: None });
            }
            None
        }
    };
    let keymap_path = Some(args.keys.clone().unwrap_or_else(config_path));
    let (keymap, err) = Keymap::load(keymap_path.as_deref());
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
    let (target, target_addr) = target.unzip();
    let at_exit = ac2_ui::spawn::AtExit::default();
    let opts = AppOptions {
        target,
        target_addr,
        start,
        at_exit: Some(at_exit.clone()),
        theme,
        keymap,
        keymap_path,
        prefs,
        prefs_path: Some(prefs_path),
        notices,
        started,
        bench_startup: args.bench_startup,
        smoke: args.smoke,
        // A daemon without an audio I/O (one just started has none) lands on the Sessions
        // dialog once; one with audio open (a rig, a kiosk) does not. The simulated rig
        // starts measuring.
        open_session_dialog: true,
        client_key,
        connect: Some(ac2_ui::app::ConnectSetup {
            key_dir,
            client_name: NAME.into(),
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
    // The window and the links (which stop our stimulus) are gone; now the ac2d children
    // this app owns.
    at_exit.stop();
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
/// A daemon asks the same for its own process, which does not cover this one.
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

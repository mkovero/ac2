//! The app in its window (rendered through egui-wgpu, headless) against a real `ac2d` on the
//! simulated rig: what the panes, lists and dialogs say and where they sit, never pixels.
//!
//! Needs a wgpu adapter: locally `AC2_GPU_FALLBACK=1 WGPU_BACKEND=vulkan` (lavapipe). Without
//! one the tests print SKIP and pass, unless `AC2_REQUIRE_GPU=1`.

use std::time::{Duration, Instant};

use ac2_e2e::{Setup, SimDaemon, sim_daemon_with};
use ac2_proto::model::{
    BackendKind, DeviceSelector, LeqConfig, LoopbackRoute, MeasConfig, MeasKind, MeasScope,
    PeakWeighting, SessionConfig, SpecAveraging, SpectrumConfig, SplConfig, TimeWeighting,
    TransferConfig, Weighting, Window,
};
use ac2_proto::topic::{Stream, Topic};
use ac2_proto::units::MeasId;
use ac2_proto::{Command, ReplyBody};
use ac2_scene::theme::ThemeName;
use ac2_ui::conn::Target;
use ac2_ui::keys::Keymap;
use ac2_ui::state::{ConnState, Overlay, PaneKind};
use ac2_ui::{App, AppOptions};
use eframe::egui::{self, Event, Key, Modifiers};
use egui_kittest::Harness;

use crate::harness::{Cli, R};

mod export;
mod latency;
mod overlays;
mod programme;
mod rigs;
mod settings;
mod top_bar;
mod traces;

pub const SIZE: egui::Vec2 = egui::vec2(1280.0, 800.0);

/// How long a wait for the UI may take. Only a ceiling: every wait ends as soon as its
/// condition holds, and on a loaded machine (software rasteriser, every core busy) the link
/// thread and the daemon can each fall seconds behind.
const WAIT: Duration = Duration::from_secs(60);

/// `true` when a wgpu adapter exists; otherwise prints why and skips (or fails under
/// `AC2_REQUIRE_GPU=1`).
pub fn have_gpu(test: &str) -> bool {
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

/// How the app describes the rig's link in its top bar.
pub const DESCRIBE: &str = "simulated rig";

/// A simulated rig set up as an operator's: a session on in 1–4 and out 1–2 (out 1 loops
/// back to in 1 and reaches the room mics on in 2 and in 3), transfer measurements `Main L`
/// (in 1 → in 2) and `Delay tower` (in 1 → in 3), the spectrum `Mic 1 FFT` and the SPL meter
/// `FOH SPL` on in 2, every one running; and a client of the test's own on it.
pub struct Rig {
    pub cli: Cli,
    // Dropped after the client.
    pub daemon: SimDaemon,
}

/// The measurement ids the rig's four measurements get, in the order made.
pub const MAIN_L: MeasId = MeasId(1);
pub const DELAY_TOWER: MeasId = MeasId(2);
pub const MIC_FFT: MeasId = MeasId(3);
pub const FOH_SPL: MeasId = MeasId(4);

impl Rig {
    pub fn start() -> R<Self> {
        Self::start_with(|_| {})
    }

    /// [`Rig::start`] with the daemon's configuration changed by `edit`.
    pub fn start_with(edit: impl FnOnce(&mut ac2d::DaemonConfig)) -> R<Self> {
        let daemon = sim_daemon_with(Setup::Empty, edit)?;
        let cli = Cli::connect(daemon.endpoints())?;
        cli.call(Command::SessionOpen { config: session() })?;
        for (want, config) in [MAIN_L, DELAY_TOWER, MIC_FFT, FOH_SPL]
            .into_iter()
            .zip(measurements())
        {
            let ReplyBody::Measurement(m) = cli.call(Command::MeasCreate { config })? else {
                return Err("meas.create: no measurement".into());
            };
            assert_eq!(m.id, want, "{}", m.config.name);
            cli.call(Command::MeasStart { meas: m.id })?;
        }
        Ok(Self { cli, daemon })
    }

    pub fn call(&self, cmd: Command) -> R<ReplyBody> {
        self.cli.call(cmd)
    }
}

fn session() -> SessionConfig {
    SessionConfig {
        backend: Some(BackendKind::Fake),
        input_device: DeviceSelector::Default,
        output_device: DeviceSelector::Default,
        input_channels: vec![0, 1, 2, 3],
        spare_inputs: 0,
        output_channels: 2,
        sample_rate_hz: None,
        buffer_frames: None,
        loopback: Some(LoopbackRoute {
            output: 0,
            input: 0,
        }),
        remote_inputs: Vec::new(),
        link_offset: None,
    }
}

fn measurements() -> [MeasConfig; 4] {
    let meas = |name: &str, kind| MeasConfig {
        scope: MeasScope::Session,
        name: name.into(),
        kind,
    };
    [
        meas(
            "Main L",
            MeasKind::Transfer {
                config: TransferConfig::with_inputs(0, 1),
            },
        ),
        meas(
            "Delay tower",
            MeasKind::Transfer {
                config: TransferConfig::with_inputs(0, 2),
            },
        ),
        meas(
            "Mic 1 FFT",
            MeasKind::Spectrum {
                config: SpectrumConfig {
                    input: 1,
                    resolution: ac2_proto::model::SpecResolution::Fixed { fft_len: 4096 },
                    window: Window::Hann,
                    averaging: SpecAveraging::Off,
                    smoothing: None,
                    level: ac2_proto::model::SpecLevel::Tone,
                    display: Default::default(),
                },
            },
        ),
        meas(
            "FOH SPL",
            MeasKind::Spl {
                config: SplConfig {
                    cue: None,
                    bands: None,
                    input: 1,
                    weighting: Weighting::A,
                    time_weighting: TimeWeighting::Fast,
                    peak_weighting: PeakWeighting::C,
                    leq: LeqConfig::default_windows(),
                    position: None,
                },
            },
        ),
    ]
}

/// The app on `rig`, its panes as the shared grid with each given the rig's measurement of
/// its kind (by name, as the preferences keep it).
pub fn options(rig: &Rig) -> AppOptions {
    options_at(Some(rig.daemon.client_config("ac2-ui test")))
}

pub fn options_at(config: Option<ac2_client::ClientConfig>) -> AppOptions {
    AppOptions {
        target: config.map(|config| Target {
            config,
            describe: DESCRIBE.into(),
        }),
        target_addr: None,
        theme: ThemeName::Dark,
        keymap: Keymap::default(),
        keymap_path: Some("~/.config/ac2/keys.toml".into()),
        prefs: {
            let mut p = crate::common::grid_ui_prefs();
            p.layout.panes = p.layout.panes.map(crate::common::fake_rig_picks);
            p
        },
        prefs_path: None,
        notices: vec![],
        started: Instant::now(),
        bench_startup: false,
        smoke: false,
        open_session_dialog: false,
        start: None,
        at_exit: None,
        client_key: None,
        connect: None,
        rasterizer: Some(ac2_plot::Rasterizer::Gpu),
    }
}

pub fn harness(opts: AppOptions) -> Harness<'static, App> {
    // Labels as on a PC on every OS: macOS would otherwise label keys with glyphs.
    ac2_ui::keys::set_label_style(ac2_ui::keys::LabelStyle::Pc);
    let mut h = Harness::builder()
        .with_size(SIZE)
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(move |cc| App::new(cc, opts));
    // Local times in UTC on every machine.
    h.state_mut().state.local_zone = ac2_ui::scenes::LocalZone::Fixed { offset_s: 0 };
    h.state_mut().health_measured = false;
    h
}

/// Steps the UI (in real time, the link runs on its own thread) until `cond` holds.
pub fn step_until(h: &mut Harness<'_, App>, what: &str, cond: impl Fn(&App) -> bool) {
    let t0 = Instant::now();
    // The sidebar's input labels take the device's channel names once the list arrives.
    let named = |a: &App| a.state.open_session().is_none() || a.state.rig.devices.is_some();
    while !(cond(h.state()) && named(h.state())) {
        if t0.elapsed() > WAIT {
            let a = h.state();
            let toasts: Vec<&str> = a
                .state
                .toasts
                .list
                .iter()
                .map(|t| t.text.as_str())
                .collect();
            let mut held: Vec<String> = a
                .state
                .rig
                .data
                .as_ref()
                .map(|d| d.latest.frames.keys().map(|k| k.to_string()).collect())
                .unwrap_or_default();
            held.sort();
            panic!(
                "timed out waiting for {what}; conn {:?}; overlay {:?}; stimulus {:?}; \
                 toasts {toasts:?}; {} measurements; frames held {held:?}",
                a.state.rig.conn,
                a.state.overlay,
                a.state.rig.stimulus.phase,
                a.state.measurements().len(),
            );
        }
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    // A few more passes so layout settles and the plots are prepared.
    for _ in 0..3 {
        h.step();
    }
}

/// Connected, in step, the rig's four measurements listed and a frame of the transfer,
/// spectrum and SPL panes held (with its grid). The impulse response is left out: the daemon
/// sends it once the stimulus has given it something to show.
pub fn live(app: &App) -> bool {
    let st = &app.state;
    let synced = st.rig.mirror.as_ref().is_some_and(|m| m.synced());
    let have = |m: MeasId, s: Stream| {
        st.rig.data.as_ref().is_some_and(|d| {
            d.latest
                .get(&Topic::Data { meas: m, stream: s })
                .is_some_and(|f| {
                    f.frame
                        .stamp
                        .grid_id
                        .is_none_or(|g| d.grids.contains_key(&g))
                })
        })
    };
    matches!(st.rig.conn, ConnState::Connected { .. })
        && synced
        && st.measurements().len() == 4
        && have(MAIN_L, Stream::Tf)
        && have(MIC_FFT, Stream::Spec)
        && have(FOH_SPL, Stream::Spl)
}

/// Steps the UI a few frames, with real time passing (the wheel's travel is smoothed over
/// frames).
pub fn settle(h: &mut Harness<'_, App>, frames: usize) {
    for _ in 0..frames {
        h.step();
        std::thread::sleep(Duration::from_millis(16));
    }
}

/// Runs a palette command by typing `what`.
pub fn palette(h: &mut Harness<'_, App>, what: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.event(Event::Text(what.into()));
    step_until(h, "palette typed", |a| {
        matches!(&a.state.overlay, Overlay::Palette(_))
    });
    h.key_press(Key::Enter);
    step_until(h, "palette closed", |a| a.state.overlay == Overlay::None);
}

/// Measurement `id`'s row dropped in the middle of the pane of `kind` focused last, as the
/// mouse would: what a pane shows is what was dropped on it, never the selection.
pub fn drop_on_kind(h: &mut Harness<'_, App>, kind: PaneKind, id: MeasId) {
    use ac2_scene::meas_list::TreeKey;
    use ac2_ui::state::{DragItem, DragMsg, DropTarget, Msg};
    let a = h.state_mut();
    let pane = crate::common::pane(&a.state, kind);
    let rig = a.state.active();
    let key = TreeKey::Meas(id);
    a.dispatch(Msg::RowDrag(DragMsg::Start(DragItem::Row { rig, key })));
    a.dispatch(Msg::RowDrag(DragMsg::Over(Some(DropTarget {
        pane,
        edge: None,
    }))));
    a.dispatch(Msg::RowDrag(DragMsg::Drop));
    settle(h, 2);
}

/// Tab until the list's row of measurement `id` is selected.
pub fn tab_to(h: &mut Harness<'_, App>, id: MeasId) {
    for _ in 0..8 {
        h.key_press(Key::Tab);
        settle(h, 2);
        let s = &h.state().state;
        if s.rig.selected == Some(id) && s.rig.selected_trace.is_none() {
            return;
        }
    }
    panic!("Tab never reached {id:?}");
}

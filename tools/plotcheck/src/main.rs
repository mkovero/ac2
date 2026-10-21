//! `plotcheck render`: one frequency response, drawn the way the app draws a transfer
//! trace, to a PNG, with the geometry the drawing claims (viewport, each pane's plot
//! rectangle and axis range, the trace colour) in a JSON file beside it.
//!
//! The cross-check suite (`tools/crosscheck`, `visual`) feeds it the export REW drew its own
//! graph from, so both pictures hold the same numbers and the only thing left to differ is
//! the drawing. The suite reads the curve back out of each picture and holds it against the
//! numbers; the JSON lets it map pixels to values without guessing where the plot is.
//!
//! The scene is `ac2_scene::tf::transfer_scene`, rendered by `ac2_plot` on whatever adapter
//! wgpu finds (lavapipe on a headless host, as the golden-image tests). The app draws its
//! transfer panes through the same two calls; what it adds around them (tree, dialogs,
//! toasts) is not a plot.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use ac2_plot::{Gpu, RenderShared, Renderer, offscreen::OFFSCREEN_FORMAT, wgpu};
use ac2_proto::model::Polarity;
use ac2_proto::units::{MeasId, Seconds, SessionEpoch};
use ac2_scene::axis::{Range, Scale};
use ac2_scene::banner::Status;
use ac2_scene::primitives::Viewport;
use ac2_scene::tf::{TfPaneKind, transfer_scene};
use ac2_scene::theme::Theme;
use ac2_scene::trace::{DisplayCache, TfTrace, TimeBase, TraceKey};
use ac2_scene::view::{FreqRange, PhaseView, PlotChrome, ViewState};
use ac2_testkit::image::Image;
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "plotcheck",
    about = "Draw a frequency response as the app does, for comparison"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Render one response to a PNG and write its geometry as JSON.
    Render(RenderArgs),
}

#[derive(Clone, Copy, ValueEnum)]
enum ThemeName {
    Dark,
    Light,
    HighContrast,
}

#[derive(Clone, Copy, ValueEnum)]
enum Chrome {
    Full,
    NoGrid,
    Bare,
}

#[derive(clap::Args)]
struct RenderArgs {
    /// Text table `freq_hz,mag_db[,phase_deg]` (comma, tab or space separated; `#` comments).
    #[arg(long)]
    input: PathBuf,
    /// PNG to write.
    #[arg(long)]
    out: PathBuf,
    /// Geometry JSON to write (default: `--out` with `.json`).
    #[arg(long)]
    meta: Option<PathBuf>,
    /// Viewport width in logical pixels.
    #[arg(long, default_value_t = 1200.0)]
    width: f32,
    /// Viewport height in logical pixels.
    #[arg(long, default_value_t = 620.0)]
    height: f32,
    /// Device pixels per logical pixel.
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    /// Frequency axis, Hz.
    #[arg(long, default_value_t = 20.0)]
    freq_lo: f64,
    #[arg(long, default_value_t = 20_000.0)]
    freq_hi: f64,
    /// Magnitude axis, dB.
    #[arg(long, default_value_t = -30.0)]
    db_lo: f64,
    #[arg(long, default_value_t = 30.0)]
    db_hi: f64,
    /// Also draw the phase pane (wrapped, −180…180°), when the input has a phase column.
    #[arg(long)]
    phase: bool,
    #[arg(long, value_enum, default_value_t = ThemeName::Dark)]
    theme: ThemeName,
    #[arg(long, value_enum, default_value_t = Chrome::Full)]
    chrome: Chrome,
    /// Trace name (legend; the legend is hidden unless `--legend`).
    #[arg(long, default_value = "REW export")]
    name: String,
    #[arg(long)]
    legend: bool,
}

#[derive(Serialize)]
struct MetaRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

#[derive(Serialize)]
struct MetaRange {
    lo: f64,
    hi: f64,
}

#[derive(Serialize)]
struct MetaAxis {
    range: MetaRange,
    scale: &'static str,
    /// Pixel of `range.lo` and of `range.hi` along the axis, logical pixels.
    px_lo: f32,
    px_hi: f32,
    ticks: Vec<MetaTick>,
}

#[derive(Serialize)]
struct MetaTick {
    value: f64,
    pos: f32,
    label: Option<String>,
}

#[derive(Serialize)]
struct MetaPane {
    kind: &'static str,
    plot: MetaRect,
    y_axis: MetaAxis,
}

#[derive(Serialize)]
struct Meta {
    /// Logical viewport.
    width: f32,
    height: f32,
    /// Device pixels per logical pixel: the PNG is `width × scale` by `height × scale`.
    scale: f32,
    x_axis: MetaAxis,
    panes: Vec<MetaPane>,
    /// The trace's colour, 0…1 RGB, and the background it is drawn on.
    trace_rgb: [f32; 3],
    background_rgb: [f32; 3],
    /// Rows read from the input.
    rows: usize,
    adapter: String,
}

struct Curve {
    freqs: Vec<f64>,
    mag: Vec<f32>,
    phase: Option<Vec<f32>>,
}

fn read_curve(text: &str) -> Result<Curve, String> {
    let mut freqs = Vec::new();
    let mut mag = Vec::new();
    let mut phase = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with('*') {
            continue;
        }
        let cols: Vec<&str> = t
            .split([',', '\t', ' ', ';'])
            .filter(|s| !s.is_empty())
            .collect();
        let num = |k: usize| -> Result<f64, String> {
            cols.get(k)
                .ok_or_else(|| format!("line {}: {} columns, need {}", i + 1, cols.len(), k + 1))?
                .parse::<f64>()
                .map_err(|e| format!("line {}: {e}", i + 1))
        };
        let f = match num(0) {
            Ok(f) => f,
            // A header line names the columns.
            Err(_) if freqs.is_empty() => continue,
            Err(e) => return Err(e),
        };
        freqs.push(f);
        mag.push(num(1)? as f32);
        if cols.len() > 2 {
            phase.push(num(2)? as f32);
        }
    }
    if freqs.len() < 2 {
        return Err("fewer than two data rows".into());
    }
    if !freqs.windows(2).all(|w| w[1] > w[0]) {
        return Err("frequencies must ascend".into());
    }
    let phase = (phase.len() == freqs.len()).then_some(phase);
    Ok(Curve { freqs, mag, phase })
}

fn axis_meta(m: &ac2_scene::axis::Mapping, ticks: &[ac2_scene::axis::Tick]) -> MetaAxis {
    MetaAxis {
        range: MetaRange {
            lo: m.range.lo,
            hi: m.range.hi,
        },
        scale: match m.scale {
            Scale::Linear => "linear",
            Scale::Log => "log",
        },
        px_lo: m.px_lo,
        px_hi: m.px_hi,
        ticks: ticks
            .iter()
            .map(|t| MetaTick {
                value: t.value,
                pos: t.pos,
                label: t.label.clone(),
            })
            .collect(),
    }
}

fn render(a: &RenderArgs) -> Result<(), String> {
    let text = fs::read_to_string(&a.input).map_err(|e| format!("{}: {e}", a.input.display()))?;
    let curve = read_curve(&text)?;
    let theme = match a.theme {
        ThemeName::Dark => Theme::dark(),
        ThemeName::Light => Theme::light(),
        ThemeName::HighContrast => Theme::high_contrast(),
    };
    let color = theme.trace_color(0);
    let trace = TfTrace {
        key: TraceKey::Live(MeasId(1)),
        name: a.name.clone(),
        color,
        freqs: &curve.freqs,
        mag_db: &curve.mag,
        phase_deg: curve.phase.as_deref(),
        coherence: None,
        validity: None,
        offset_db: 0.0,
        polarity: Polarity::Normal,
        nudge: Seconds(0.0),
        delay_nudge: Seconds(0.0),
        time_base: TimeBase::Shared {
            epoch: SessionEpoch(1),
            delay: Seconds(0.0),
        },
        freshness: None,
        smoothing: None,
        note: None,
        stored: None,
        selected: false,
        compared: false,
        unresolved: None,
    };
    let mut view = ViewState {
        freq: FreqRange {
            lo: a.freq_lo,
            hi: a.freq_hi,
        },
        chrome: match a.chrome {
            Chrome::Full => PlotChrome::Full,
            Chrome::NoGrid => PlotChrome::NoGrid,
            Chrome::Bare => PlotChrome::Bare,
        },
        unresolved: false,
        ..ViewState::default()
    };
    view.tf.show_magnitude = true;
    view.tf.show_phase = a.phase && curve.phase.is_some();
    view.tf.show_coherence = false;
    view.tf.magnitude_db = Range::new(a.db_lo, a.db_hi);
    view.tf.phase = PhaseView::Wrapped;
    view.tf.legend.hidden = !a.legend;
    let size = Viewport {
        width: a.width,
        height: a.height,
    };
    let s = transfer_scene(
        &[trace],
        &DisplayCache::default(),
        &Status::default(),
        &view,
        &theme,
        size,
    );

    let gpu = Gpu::new().map_err(|e| format!("no GPU adapter: {e}"))?;
    let shared = Arc::new(RenderShared::new(
        &gpu.device,
        OFFSCREEN_FORMAT,
        wgpu::MultisampleState::default(),
    ));
    let mut r = Renderer::new(&gpu.device, &gpu.queue, &shared);
    let px_size = [
        (a.width * a.scale).round() as u32,
        (a.height * a.scale).round() as u32,
    ];
    let px = ac2_plot::render_to_image(&gpu, &mut r, &s.scene, px_size, a.scale, theme.background)
        .map_err(|e| format!("render: {e}"))?;
    let img = Image::new(px.width, px.height, px.rgba).map_err(|e| format!("image: {e}"))?;
    img.save_png(&a.out)
        .map_err(|e| format!("{}: {e}", a.out.display()))?;

    let meta = Meta {
        width: a.width,
        height: a.height,
        scale: a.scale,
        x_axis: axis_meta(&s.x_axis.mapping, &s.x_axis.ticks),
        panes: s
            .panes
            .iter()
            .map(|p| MetaPane {
                kind: match p.kind {
                    TfPaneKind::Magnitude => "magnitude",
                    TfPaneKind::Phase => "phase",
                    TfPaneKind::Coherence => "coherence",
                },
                plot: MetaRect {
                    x: p.plot.x,
                    y: p.plot.y,
                    w: p.plot.w,
                    h: p.plot.h,
                },
                y_axis: axis_meta(&p.y_axis.mapping, &p.y_axis.ticks),
            })
            .collect(),
        trace_rgb: [color.r, color.g, color.b],
        background_rgb: [theme.background.r, theme.background.g, theme.background.b],
        rows: curve.freqs.len(),
        adapter: gpu.describe(),
    };
    let meta_path = a
        .meta
        .clone()
        .unwrap_or_else(|| a.out.with_extension("json"));
    fs::write(
        &meta_path,
        serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{}: {e}", meta_path.display()))?;
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let r = match &cli.cmd {
        Cmd::Render(a) => render(a),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("plotcheck: {e}");
            ExitCode::FAILURE
        }
    }
}

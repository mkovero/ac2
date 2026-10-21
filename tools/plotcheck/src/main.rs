//! `plotcheck`: one curve of each pane kind, drawn the way the app draws it, to a PNG, with
//! the geometry the drawing claims (viewport, each pane's plot rectangle and axis mapping,
//! the trace colour) in a JSON file beside it.
//!
//! The cross-check suite (`tools/crosscheck`, `visual`) feeds it the export REW drew its own
//! graph from, so both pictures hold the same numbers and the only thing left to differ is
//! the drawing. The suite reads the curve back out of each picture and holds it against the
//! numbers; the JSON lets it map pixels to values without guessing where the plot is.
//!
//! Each subcommand is one scene builder of `ac2_scene` (`tf::transfer_scene`,
//! `ir::ir_scene`, `spectrum::spectrum_scene`, `leq::leq_scene`) rendered by `ac2_plot` on
//! whatever adapter wgpu finds (lavapipe on a headless host, as the golden-image tests). The
//! app draws its panes through the same calls; what it adds around them (tree, dialogs,
//! toasts) is not a plot.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use ac2_plot::{Gpu, RenderShared, Renderer, offscreen::OFFSCREEN_FORMAT, wgpu};
use ac2_proto::frame::{
    IrFrame, IrMeta, LeqFlags, LeqFrame, LeqMeta, RtaFrame, RtaMeta, ValidityMask,
};
use ac2_proto::model::{
    BandFraction, CalBasis, CalStatus, LeqConfig, LeqWindow, LevelScale, Polarity, SpectrumStyle,
    Weighting,
};
use ac2_proto::units::{Db, DbSpl, Hz, MeasId, Seconds, SessionEpoch, WallNs};
use ac2_scene::axis::{Range, Scale};
use ac2_scene::banner::Status;
use ac2_scene::ir::{IrCtx, ir_scene};
use ac2_scene::leq::{
    LeqHistory, LeqStripView, LeqView, StripSpan, history_span, leq_scene, leq_tiles,
};
use ac2_scene::primitives::{Color, Scene, Viewport};
use ac2_scene::spectrum::{LiveMeta, SpectrumTrace, spectrum_scene};
use ac2_scene::tf::{TfPaneKind, transfer_scene};
use ac2_scene::theme::Theme;
use ac2_scene::time::Freshness;
use ac2_scene::trace::{DisplayCache, TfTrace, TimeBase, TraceKey, display_traces};
use ac2_scene::view::{
    FreqRange, IrAxes, IrMode, LeqLayout, LeqStyle, PhaseView, PlotChrome, Style, ViewState,
};
use ac2_testkit::image::Image;
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "plotcheck",
    about = "Draw a curve as the app does, for comparison"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// A frequency response in the transfer panes (magnitude, and phase or group delay).
    Tf(TfArgs),
    /// An impulse response in the IR pane (linear or log).
    Ir(IrArgs),
    /// A spectrum in the spectrum pane (line, fill or bars).
    Spectrum(SpectrumArgs),
    /// One window's Leq over time in the Leq view's history strip.
    Leq(LeqArgs),
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

/// What every subcommand takes: the input table, the files to write, the viewport and the
/// look.
#[derive(clap::Args)]
struct Common {
    /// Text table (comma, tab, space or `;` separated; `#` comments; a header line).
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
    #[arg(long, value_enum, default_value_t = ThemeName::Dark)]
    theme: ThemeName,
    #[arg(long, value_enum, default_value_t = Chrome::Full)]
    chrome: Chrome,
    /// Trace (or meter) name.
    #[arg(long, default_value = "REW export")]
    name: String,
}

impl Common {
    fn theme(&self) -> Theme {
        match self.theme {
            ThemeName::Dark => Theme::dark(),
            ThemeName::Light => Theme::light(),
            ThemeName::HighContrast => Theme::high_contrast(),
        }
    }

    fn chrome(&self) -> PlotChrome {
        match self.chrome {
            Chrome::Full => PlotChrome::Full,
            Chrome::NoGrid => PlotChrome::NoGrid,
            Chrome::Bare => PlotChrome::Bare,
        }
    }

    fn size(&self) -> Viewport {
        Viewport {
            width: self.width,
            height: self.height,
        }
    }

    fn read(&self) -> Result<String, String> {
        fs::read_to_string(&self.input).map_err(|e| format!("{}: {e}", self.input.display()))
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum PhaseViewName {
    Wrapped,
    GroupDelay,
}

#[derive(clap::Args)]
struct TfArgs {
    #[command(flatten)]
    common: Common,
    /// Frequency axis, Hz.
    #[arg(long, default_value_t = 20.0, allow_negative_numbers = true)]
    freq_lo: f64,
    #[arg(long, default_value_t = 20_000.0, allow_negative_numbers = true)]
    freq_hi: f64,
    /// Magnitude axis, dB.
    #[arg(long, default_value_t = -30.0, allow_negative_numbers = true)]
    db_lo: f64,
    #[arg(long, default_value_t = 30.0, allow_negative_numbers = true)]
    db_hi: f64,
    /// Also draw the phase pane, when the input has a phase column (`freq_hz,mag_db,phase_deg`).
    #[arg(long)]
    phase: bool,
    /// What the phase pane shows: wrapped phase (−180…180°) or group delay (`--gd-lo/--gd-hi`).
    #[arg(long, value_enum, default_value_t = PhaseViewName::Wrapped)]
    phase_view: PhaseViewName,
    /// Group-delay axis, ms.
    #[arg(long, default_value_t = -5.0, allow_negative_numbers = true)]
    gd_lo: f64,
    #[arg(long, default_value_t = 20.0, allow_negative_numbers = true)]
    gd_hi: f64,
    /// Write a CSV `freq_hz,value` of what the drawn trace carries in the phase pane (wrapped
    /// degrees, or group delay in ms as ac2 computed it from the phase column); without a
    /// phase pane, the magnitude in dB.
    #[arg(long)]
    dump: Option<PathBuf>,
    #[arg(long)]
    legend: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum IrModeName {
    Linear,
    Log,
}

/// Table `t_s,value`: seconds (uniform, ascending) and linear amplitude (FS).
#[derive(clap::Args)]
struct IrArgs {
    #[command(flatten)]
    common: Common,
    #[arg(long, value_enum, default_value_t = IrModeName::Linear)]
    mode: IrModeName,
    /// Time axis, ms re t = 0 of the input.
    #[arg(long, allow_negative_numbers = true)]
    t_lo: f64,
    #[arg(long, allow_negative_numbers = true)]
    t_hi: f64,
    /// Amplitude axis of the linear view, FS (default: ±110 % of the peak, as the app).
    #[arg(long, allow_negative_numbers = true)]
    amp_lo: Option<f64>,
    #[arg(long, allow_negative_numbers = true)]
    amp_hi: Option<f64>,
    /// Level axis of the log view, dB re the peak.
    #[arg(long, default_value_t = -60.0, allow_negative_numbers = true)]
    db_lo: f64,
    #[arg(long, default_value_t = 3.0, allow_negative_numbers = true)]
    db_hi: f64,
}

#[derive(Clone, Copy, ValueEnum)]
enum StyleName {
    Line,
    Fill,
    Bars,
}

#[derive(clap::Args)]
struct SpectrumArgs {
    #[command(flatten)]
    common: Common,
    /// Frequency axis, Hz.
    #[arg(long, default_value_t = 20.0, allow_negative_numbers = true)]
    freq_lo: f64,
    #[arg(long, default_value_t = 20_000.0, allow_negative_numbers = true)]
    freq_hi: f64,
    /// Level axis, dBFS.
    #[arg(long, default_value_t = -100.0, allow_negative_numbers = true)]
    db_lo: f64,
    #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
    db_hi: f64,
    #[arg(long, value_enum, default_value_t = StyleName::Line)]
    style: StyleName,
}

#[derive(Clone, Copy, ValueEnum)]
enum StripName {
    /// The live strip: time ago, the newest point at the right edge.
    Live,
    /// Looking back over the whole input: clock time along the axis.
    Span,
}

/// Table `t_s,level_db`: one window's Leq at each second (uniform, ascending).
#[derive(clap::Args)]
struct LeqArgs {
    #[command(flatten)]
    common: Common,
    /// The window's length, seconds.
    #[arg(long, allow_negative_numbers = true)]
    window_s: f64,
    #[arg(long, value_enum, default_value_t = StripName::Live)]
    strip: StripName,
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
    /// What the values are in: `Hz`, `dB`, `°`, `ms`, `FS`, `s`, `min`.
    unit: &'static str,
    /// Pixel of `range.lo` and of `range.hi` along the axis, logical pixels. On a vertical
    /// axis `px_lo` is the plot bottom (the larger row) and `px_hi` the plot top.
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

/// How the IR view transformed the input: the log view is dB re `peak_abs`.
#[derive(Serialize)]
struct IrTransform {
    mode: &'static str,
    peak_abs: f64,
    /// Time of the peak, seconds (the input's own clock).
    peak_t_s: f64,
    t0_s: f64,
    dt_s: f64,
}

#[derive(Serialize)]
struct SpectrumMeta {
    style: &'static str,
}

/// How the strip's time axis reads the input's clock: `x = (t_s − t_ref_s) / x_unit_s`.
#[derive(Serialize)]
struct LeqMetaOut {
    strip: &'static str,
    window_s: f64,
    t_ref_s: f64,
    x_unit_s: f64,
    /// The newest point's time, seconds (the live strip's right edge).
    newest_s: f64,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    ir: Option<IrTransform>,
    #[serde(skip_serializing_if = "Option::is_none")]
    spectrum: Option<SpectrumMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    leq: Option<LeqMetaOut>,
}

/// Numeric rows of a text table: comment lines (`#`, `*`) and blank lines skipped, a
/// header line (first column not a number) before the data skipped, at least `min_cols`
/// columns per row.
fn read_table(text: &str, min_cols: usize) -> Result<Vec<Vec<f64>>, String> {
    let mut rows: Vec<Vec<f64>> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with('*') {
            continue;
        }
        let cols: Vec<&str> = t
            .split([',', '\t', ' ', ';'])
            .filter(|s| !s.is_empty())
            .collect();
        if cols.len() < min_cols {
            return Err(format!(
                "line {}: {} columns, need {min_cols}",
                i + 1,
                cols.len()
            ));
        }
        let parsed: Result<Vec<f64>, _> = cols.iter().map(|c| c.parse::<f64>()).collect();
        match parsed {
            Ok(v) => rows.push(v),
            // A header line names the columns.
            Err(_) if rows.is_empty() && cols[0].parse::<f64>().is_err() => continue,
            Err(e) => return Err(format!("line {}: {e}", i + 1)),
        }
    }
    if rows.len() < 2 {
        return Err("fewer than two data rows".into());
    }
    Ok(rows)
}

fn column(rows: &[Vec<f64>], k: usize) -> Vec<f64> {
    rows.iter().map(|r| r[k]).collect()
}

fn ascending(v: &[f64], what: &str) -> Result<(), String> {
    if v.windows(2).all(|w| w[1] > w[0]) {
        Ok(())
    } else {
        Err(format!("{what} must ascend"))
    }
}

/// The step of a uniform series: every step within a millionth of the first.
fn uniform_step(t: &[f64], what: &str) -> Result<f64, String> {
    let dt = t[1] - t[0];
    if dt <= 0.0 {
        return Err(format!("{what} must ascend"));
    }
    // A text table carries a finite number of digits: a step that wobbles by a thousandth
    // is the same sample clock, and invisible at any plot scale.
    let ok = t
        .windows(2)
        .all(|w| ((w[1] - w[0]) - dt).abs() <= dt * 1e-3);
    if ok {
        Ok(dt)
    } else {
        Err(format!("{what} must be uniformly spaced"))
    }
}

fn axis_meta(
    m: &ac2_scene::axis::Mapping,
    ticks: &[ac2_scene::axis::Tick],
    unit: &'static str,
) -> MetaAxis {
    MetaAxis {
        range: MetaRange {
            lo: m.range.lo,
            hi: m.range.hi,
        },
        scale: match m.scale {
            Scale::Linear => "linear",
            Scale::Log => "log",
        },
        unit,
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

fn rect_meta(r: ac2_scene::primitives::Rect) -> MetaRect {
    MetaRect {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
    }
}

/// Renders `scene` to `--out` and returns the adapter's description.
fn render_png(c: &Common, scene: &Scene, theme: &Theme) -> Result<String, String> {
    let gpu = Gpu::new().map_err(|e| format!("no GPU adapter: {e}"))?;
    let shared = Arc::new(RenderShared::new(
        &gpu.device,
        OFFSCREEN_FORMAT,
        wgpu::MultisampleState::default(),
    ));
    let mut r = Renderer::new(&gpu.device, &gpu.queue, &shared);
    let px_size = [
        (c.width * c.scale).round() as u32,
        (c.height * c.scale).round() as u32,
    ];
    let px = ac2_plot::render_to_image(&gpu, &mut r, scene, px_size, c.scale, theme.background)
        .map_err(|e| format!("render: {e}"))?;
    let img = Image::new(px.width, px.height, px.rgba).map_err(|e| format!("image: {e}"))?;
    img.save_png(&c.out)
        .map_err(|e| format!("{}: {e}", c.out.display()))?;
    Ok(gpu.describe())
}

fn write_meta(c: &Common, meta: &Meta) -> Result<(), String> {
    let meta_path = c
        .meta
        .clone()
        .unwrap_or_else(|| c.out.with_extension("json"));
    fs::write(
        &meta_path,
        serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{}: {e}", meta_path.display()))
}

/// The common part of every meta: the viewport, the colours, the adapter.
fn meta_base(c: &Common, theme: &Theme, color: Color, rows: usize, adapter: String) -> Meta {
    Meta {
        width: c.width,
        height: c.height,
        scale: c.scale,
        x_axis: MetaAxis {
            range: MetaRange { lo: 0.0, hi: 0.0 },
            scale: "linear",
            unit: "",
            px_lo: 0.0,
            px_hi: 0.0,
            ticks: vec![],
        },
        panes: vec![],
        trace_rgb: [color.r, color.g, color.b],
        background_rgb: [theme.background.r, theme.background.g, theme.background.b],
        rows,
        adapter,
        ir: None,
        spectrum: None,
        leq: None,
    }
}

fn write_dump(path: &PathBuf, what: &str, freqs: &[f64], values: &[f64]) -> Result<(), String> {
    let mut s = format!("# value: {what}\nfreq_hz,value\n");
    for (f, v) in freqs.iter().zip(values) {
        s.push_str(&format!("{f},{v}\n"));
    }
    fs::write(path, s).map_err(|e| format!("{}: {e}", path.display()))
}

fn tf(a: &TfArgs) -> Result<(), String> {
    let c = &a.common;
    let rows = read_table(&c.read()?, 2)?;
    let freqs = column(&rows, 0);
    ascending(&freqs, "frequencies")?;
    let mag: Vec<f32> = rows.iter().map(|r| r[1] as f32).collect();
    let phase: Option<Vec<f32>> = rows
        .iter()
        .all(|r| r.len() > 2)
        .then(|| rows.iter().map(|r| r[2] as f32).collect());
    let theme = c.theme();
    let color = theme.trace_color(0);
    let trace = TfTrace {
        key: TraceKey::Live(MeasId(1)),
        name: c.name.clone(),
        color,
        freqs: &freqs,
        mag_db: &mag,
        phase_deg: phase.as_deref(),
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
        chrome: c.chrome(),
        unresolved: false,
        ..ViewState::default()
    };
    let show_phase = a.phase && phase.is_some();
    view.tf.show_magnitude = true;
    view.tf.show_phase = show_phase;
    view.tf.show_coherence = false;
    view.tf.magnitude_db = Range::new(a.db_lo, a.db_hi);
    view.tf.phase = match a.phase_view {
        PhaseViewName::Wrapped => PhaseView::Wrapped,
        PhaseViewName::GroupDelay => PhaseView::GroupDelay {
            range_ms: Range::new(a.gd_lo, a.gd_hi),
        },
    };
    view.tf.legend.hidden = !a.legend;
    let cache = DisplayCache::default();
    let s = transfer_scene(
        std::slice::from_ref(&trace),
        &cache,
        &Status::default(),
        &view,
        &theme,
        c.size(),
    );

    if let Some(path) = &a.dump {
        // The same display math the scene ran: the phase pane draws these very numbers
        // (group delay scaled to ms at the axis).
        let (_, shown) = display_traces(
            std::slice::from_ref(&trace),
            &cache,
            view.tf.phase_reference,
            &view.tf.coherence,
        );
        let d = shown.first().ok_or("no display trace")?;
        let (what, values): (&str, Vec<f64>) = match (show_phase, a.phase_view) {
            (true, PhaseViewName::GroupDelay) => (
                "group_delay_ms",
                d.group_delay_s.iter().map(|g| g * 1000.0).collect(),
            ),
            (true, PhaseViewName::Wrapped) => ("phase_wrapped_deg", d.phase_wrapped_deg.clone()),
            (false, _) => ("magnitude_db", d.magnitude_db.clone()),
        };
        write_dump(path, what, &d.freqs, &values)?;
    }

    let adapter = render_png(c, &s.scene, &theme)?;
    let mut meta = meta_base(c, &theme, color, rows.len(), adapter);
    meta.x_axis = axis_meta(&s.x_axis.mapping, &s.x_axis.ticks, "Hz");
    meta.panes = s
        .panes
        .iter()
        .map(|p| {
            let (kind, unit) = match p.kind {
                TfPaneKind::Magnitude => ("magnitude", "dB"),
                TfPaneKind::Phase => match view.tf.phase {
                    PhaseView::GroupDelay { .. } => ("group_delay", "ms"),
                    _ => ("phase", "°"),
                },
                TfPaneKind::Coherence => ("coherence", ""),
            };
            MetaPane {
                kind,
                plot: rect_meta(p.plot),
                y_axis: axis_meta(&p.y_axis.mapping, &p.y_axis.ticks, unit),
            }
        })
        .collect();
    write_meta(c, &meta)
}

fn ir(a: &IrArgs) -> Result<(), String> {
    let c = &a.common;
    let rows = read_table(&c.read()?, 2)?;
    let t = column(&rows, 0);
    let dt = uniform_step(&t, "times")?;
    let linear: Vec<f32> = rows.iter().map(|r| r[1] as f32).collect();
    let (peak_i, peak_abs) = linear
        .iter()
        .enumerate()
        .filter(|(_, v)| v.is_finite())
        .map(|(i, v)| (i, f64::from(v.abs())))
        .fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a });
    let frame = IrFrame {
        meas: MeasId(1),
        meta: IrMeta {
            sample_rate: Hz(1.0 / dt),
            t0: Seconds(t[0]),
            dt: Seconds(dt),
            inserted_delay: Seconds(0.0),
        },
        linear,
        etc: None,
    };
    let theme = c.theme();
    let color = theme.trace_color(0);
    let mode = match a.mode {
        IrModeName::Linear => IrMode::Linear,
        IrModeName::Log => IrMode::Log,
    };
    let mut view = ViewState {
        chrome: c.chrome(),
        ..ViewState::default()
    };
    view.ir.mode = mode;
    view.ir.axes = IrAxes {
        time_ms: Some(Range::new(a.t_lo, a.t_hi)),
        amplitude: match (a.amp_lo, a.amp_hi) {
            (Some(lo), Some(hi)) => Some(Range::new(lo, hi)),
            (None, None) => None,
            _ => return Err("give both --amp-lo and --amp-hi, or neither".into()),
        },
        level_db: Range::new(a.db_lo, a.db_hi),
        cursor_ms: None,
    };
    let status = Status::default();
    let ctx = IrCtx {
        status: &status,
        view: &view,
        axes: &view.ir.axes,
        style: Style {
            chrome: view.chrome,
            theme: &theme,
        },
        size: c.size(),
    };
    let s = ir_scene(&frame, color, None, &ctx);
    if let Some(n) = &s.note {
        return Err(format!("nothing drawn: {n}"));
    }
    let adapter = render_png(c, &s.scene, &theme)?;
    let mut meta = meta_base(c, &theme, color, rows.len(), adapter);
    meta.x_axis = axis_meta(&s.x_axis.mapping, &s.x_axis.ticks, "ms");
    meta.panes = vec![MetaPane {
        kind: "ir",
        plot: rect_meta(s.plot),
        y_axis: axis_meta(
            &s.y_axis.mapping,
            &s.y_axis.ticks,
            if mode == IrMode::Linear { "FS" } else { "dB" },
        ),
    }];
    meta.ir = Some(IrTransform {
        mode: match mode {
            IrMode::Linear => "linear",
            _ => "log",
        },
        peak_abs,
        peak_t_s: t[peak_i],
        t0_s: t[0],
        dt_s: dt,
    });
    write_meta(c, &meta)
}

/// The band fraction whose spacing is nearest the columns' median spacing (the caption
/// names it; the columns themselves are the input's).
fn nearest_fraction(freqs: &[f64]) -> BandFraction {
    let mut ratios: Vec<f64> = freqs.windows(2).map(|w| (w[1] / w[0]).log2()).collect();
    ratios.sort_by(f64::total_cmp);
    let per_oct = 1.0 / ratios[ratios.len() / 2].max(1e-9);
    [
        (1.0, BandFraction::Octave),
        (3.0, BandFraction::Third),
        (6.0, BandFraction::Sixth),
        (12.0, BandFraction::Twelfth),
        (24.0, BandFraction::TwentyFourth),
    ]
    .into_iter()
    .min_by(|a, b| (a.0 - per_oct).abs().total_cmp(&(b.0 - per_oct).abs()))
    .map_or(BandFraction::Third, |(_, f)| f)
}

/// Band edges at the geometric midpoints between columns, the ends extended by the same
/// log ratio as their inner edge.
fn geometric_edges(freqs: &[f64]) -> Vec<(f64, f64)> {
    let n = freqs.len();
    let mid: Vec<f64> = freqs.windows(2).map(|w| (w[0] * w[1]).sqrt()).collect();
    (0..n)
        .map(|i| {
            let lo = if i == 0 {
                freqs[0] * freqs[0] / mid[0]
            } else {
                mid[i - 1]
            };
            let hi = if i == n - 1 {
                freqs[n - 1] * freqs[n - 1] / mid[n - 2]
            } else {
                mid[i]
            };
            (lo, hi)
        })
        .collect()
}

fn spectrum(a: &SpectrumArgs) -> Result<(), String> {
    let c = &a.common;
    let rows = read_table(&c.read()?, 2)?;
    let freqs = column(&rows, 0);
    ascending(&freqs, "frequencies")?;
    if freqs[0] <= 0.0 {
        return Err("frequencies must be positive".into());
    }
    let level: Vec<f32> = rows.iter().map(|r| r[1] as f32).collect();
    let edges = geometric_edges(&freqs);
    let frame = RtaFrame {
        meas: MeasId(1),
        meta: RtaMeta {
            fraction: nearest_fraction(&freqs),
            weighting: Weighting::Z,
            scale: LevelScale::Dbfs,
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
            math: None,
        },
        validity: vec![ValidityMask::NONE; level.len()],
        level,
    };
    let theme = c.theme();
    let color = theme.trace_color(0);
    let style = match a.style {
        StyleName::Line => SpectrumStyle::Line,
        StyleName::Fill => SpectrumStyle::Fill,
        StyleName::Bars => SpectrumStyle::Bars,
    };
    let mut trace = SpectrumTrace::rta(
        &frame,
        LiveMeta {
            captured: WallNs(0),
            curve: None,
            freqs: &freqs,
            edges: &edges,
            name: c.name.clone(),
            color,
            freshness: Freshness::from_age(0.0),
        },
    );
    trace.style = style;
    let mut view = ViewState {
        freq: FreqRange {
            lo: a.freq_lo,
            hi: a.freq_hi,
        },
        chrome: c.chrome(),
        unresolved: false,
        ..ViewState::default()
    };
    view.spectrum.level = Range::new(a.db_lo, a.db_hi);
    let s = spectrum_scene(
        std::slice::from_ref(&trace),
        &Status::default(),
        &view,
        &theme,
        c.size(),
    );
    let adapter = render_png(c, &s.scene, &theme)?;
    let mut meta = meta_base(c, &theme, color, rows.len(), adapter);
    meta.x_axis = axis_meta(&s.x_axis.mapping, &s.x_axis.ticks, "Hz");
    meta.panes = vec![MetaPane {
        kind: "spectrum",
        plot: rect_meta(s.plot),
        y_axis: axis_meta(&s.y_axis.mapping, &s.y_axis.ticks, "dB"),
    }];
    meta.spectrum = Some(SpectrumMeta {
        style: match style {
            SpectrumStyle::Line => "line",
            SpectrumStyle::Fill => "fill",
            SpectrumStyle::Bars => "bars",
        },
    });
    write_meta(c, &meta)
}

fn leq(a: &LeqArgs) -> Result<(), String> {
    let c = &a.common;
    let rows = read_table(&c.read()?, 2)?;
    let t = column(&rows, 0);
    uniform_step(&t, "times")?;
    if a.window_s < 1.0 {
        return Err("--window-s must be at least 1".into());
    }
    let cfg = LeqConfig {
        windows: vec![LeqWindow {
            duration: Seconds(a.window_s),
            weighting: Weighting::A,
            limit: None,
            warn_margin: Db(0.0),
            cue: None,
        }],
        horizon: Seconds(60.0),
        peaks: Default::default(),
    };
    // One frame per row, as the daemon publishes the window once a second: the history
    // keeps each as a point, the tiles show the last.
    let frame_at = |v: f64| LeqFrame {
        meas: MeasId(1),
        meta: LeqMeta {
            scale: LevelScale::DbSpl,
            cal: CalStatus::Verified {
                calibrated_at: WallNs(1),
                basis: CalBasis::Acoustic {
                    calibrator_level: DbSpl(94.0),
                },
            },
            mic_curve: false,
            horizon: Seconds(60.0),
            logged: 0,
            run: None,
            lcpeak: None,
            lafmax: None,
            position: None,
        },
        leq: vec![v as f32],
        elapsed: vec![a.window_s as f32],
        measured: vec![a.window_s as f32],
        allowed: vec![f32::NAN],
        recover: vec![f32::NAN],
        least: vec![v as f32],
        over_in: vec![f32::NAN],
        flags: vec![LeqFlags::NONE],
    };
    let mut h = LeqHistory::default();
    for r in &rows {
        h.push(&cfg, &frame_at(r[1]), r[0]);
    }
    let last = frame_at(rows[rows.len() - 1][1]);
    let newest = h.newest().ok_or("no history points")?;
    let (first, last_t) = (t[0], t[t.len() - 1]);
    let strip = match a.strip {
        StripName::Live => StripSpan::default(),
        StripName::Span => StripSpan {
            span: Some((first, last_t)),
            utc_offset_s: 0,
        },
    };
    let theme = c.theme();
    let color = theme.trace_color(0);
    let v = LeqView {
        meter: c.name.clone(),
        cal: String::new(),
        cfg: &cfg,
        tiles: leq_tiles(&cfg, &last),
        history: Some(&h),
        stale: None,
        scale: LevelScale::DbSpl,
        layout: LeqLayout {
            style: LeqStyle::Columns,
            history: true,
        },
        run: None,
        stage: false,
        chrome: c.chrome(),
        strip: LeqStripView {
            span: strip,
            ..LeqStripView::default()
        },
    };
    let s = leq_scene(&v, &Status::default(), &theme, c.size());
    let hist = s
        .history
        .as_ref()
        .ok_or("no room for the history strip: a bigger viewport is needed")?;
    let adapter = render_png(c, &s.scene, &theme)?;
    let (strip_name, t_ref_s, x_unit_s, unit) = match a.strip {
        StripName::Live => {
            let in_minutes = history_span(&cfg) > 180.0;
            let unit_s = if in_minutes { 60.0 } else { 1.0 };
            ("live", newest, unit_s, if in_minutes { "min" } else { "s" })
        }
        StripName::Span => ("span", 0.0, 1.0, "s"),
    };
    let mut meta = meta_base(c, &theme, color, rows.len(), adapter);
    meta.x_axis = axis_meta(&hist.x.mapping, &hist.x.ticks, unit);
    meta.panes = vec![MetaPane {
        kind: "leq_history",
        plot: rect_meta(hist.plot),
        y_axis: axis_meta(&hist.y.mapping, &hist.y.ticks, "dB"),
    }];
    meta.leq = Some(LeqMetaOut {
        strip: strip_name,
        window_s: a.window_s,
        t_ref_s,
        x_unit_s,
        newest_s: newest,
    });
    write_meta(c, &meta)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let r = match &cli.cmd {
        Cmd::Tf(a) => tf(a),
        Cmd::Ir(a) => ir(a),
        Cmd::Spectrum(a) => spectrum(a),
        Cmd::Leq(a) => leq(a),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("plotcheck: {e}");
            ExitCode::FAILURE
        }
    }
}

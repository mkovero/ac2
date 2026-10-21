//! End to end: an `ac2-scene` view built from synthetic traces, rendered and compared with
//! a golden image. The values are asserted headless in `ac2-scene`; this checks that the
//! renderer draws what the scene says (panes, gaps, alpha, legend, banner strip above the
//! panes, coherence in its own pane or overlaid on magnitude).

use crate::common;

use ac2_plot::Viewport;
use ac2_proto::frame::ValidityMask;
use ac2_proto::model::Polarity;
use ac2_proto::model::SpectrumStyle;
use ac2_proto::units::{MeasId, Seconds, SessionEpoch};
use ac2_scene::banner::Status;
use ac2_scene::theme::Theme;
use ac2_scene::time::Freshness;
use ac2_scene::trace::{TfTrace, TimeBase, TraceKey, wrap_deg};
use ac2_scene::view::{CoherencePlacement, ViewState};
use ac2_testkit::image::ImageTolerance;
use common::{golden, gpu, render_on, renderer};

const TEXT: ImageTolerance = ImageTolerance {
    channel: 32,
    max_bad_fraction: 0.005,
};

struct Cols {
    freqs: Vec<f64>,
    mag: Vec<f32>,
    phase: Vec<f32>,
    coh: Vec<f32>,
    validity: Vec<ValidityMask>,
}

/// 1/12-octave columns, 20 Hz–20 kHz. A resonance-like bump, a path that arrives `tau`
/// after the delay its own frame was compensated for, coherence falling at the low end.
fn cols(gain_db: f32, tau: f64) -> Cols {
    let freqs: Vec<f64> = (-120..=120)
        .map(|k| 1000.0 * 2f64.powf(f64::from(k) / 12.0))
        .filter(|f| (20.0..=20_000.0).contains(f))
        .collect();
    let mag = freqs
        .iter()
        .map(|f| {
            let x = (f / 2000.0).log2();
            (gain_db as f64 + 6.0 * (-x * x * 2.0).exp() - 3.0 * (f / 60.0).powi(-2).min(4.0))
                as f32
        })
        .collect();
    let phase = freqs
        .iter()
        .map(|f| wrap_deg(-360.0 * f * tau) as f32)
        .collect();
    let coh = freqs
        .iter()
        .map(|f| (1.0 - (40.0 / f).powi(2)).clamp(0.0, 1.0) as f32)
        .collect();
    let mut validity = vec![ValidityMask::NONE; freqs.len()];
    // A thinned stretch: the line must break there.
    for v in validity.iter_mut().skip(30).take(3) {
        *v = ValidityMask::THINNED;
    }
    Cols {
        freqs,
        mag,
        phase,
        coh,
        validity,
    }
}

fn trace<'a>(c: &'a Cols, meas: u32, color: usize, delay: f64) -> TfTrace<'a> {
    TfTrace {
        key: TraceKey::Live(MeasId(meas)),
        name: format!("Meas {meas}"),
        color: Theme::dark().trace_color(color),
        freqs: &c.freqs,
        mag_db: &c.mag,
        phase_deg: Some(&c.phase),
        coherence: Some(&c.coh),
        validity: Some(&c.validity),
        offset_db: 0.0,
        polarity: Polarity::Normal,
        nudge: Seconds(0.0),
        delay_nudge: Seconds(0.0),
        time_base: TimeBase::Shared {
            epoch: SessionEpoch(1),
            delay: Seconds(delay),
        },
        freshness: Some(Freshness::from_age(0.1)),
        smoothing: None,
        note: None,
        stored: None,
        selected: false,
        compared: false,
        unresolved: None,
    }
}

/// Two traces with a cursor under `status`'s banners (the first reading `first`), rendered
/// and compared with `name`.
fn transfer_golden(name: &str, view: ViewState, status: Status, first: &str) {
    let Some(gpu) = gpu(name) else {
        return;
    };
    let a = cols(0.0, 0.0);
    let b = cols(-4.0, 0.0);
    // Second path 0.25 ms later: shows as a phase slope relative to the first.
    let traces = [trace(&a, 1, 0, 0.010), trace(&b, 2, 1, 0.010_25)];
    let view = ViewState {
        cursor_hz: Some(1000.0),
        ..view
    };
    let theme = Theme::dark();
    let size = Viewport {
        width: 560.0,
        height: 360.0,
    };
    let s = ac2_scene::tf::transfer_scene(
        &traces,
        &ac2_scene::trace::DisplayCache::default(),
        &status,
        &view,
        &theme,
        size,
    );
    assert_eq!(s.banners[0].text, first);
    // The strip sits above every pane.
    assert!(s.panes.iter().all(|p| p.plot.y > s.strip.bottom()));
    let mut r = renderer(gpu);
    let img = render_on(gpu, &mut r, &s.scene, 1.0, theme.background);
    golden(name, &img, TEXT);
}

fn stale() -> Status {
    Status {
        frame_age_s: Some(2.4),
        ..Status::default()
    }
}

#[test]
fn transfer_view() {
    transfer_golden(
        "transfer_view",
        ViewState::default(),
        stale(),
        "STALE · 2.4 s",
    );
}

/// The cursor read against its mark on the bump's top: the mark's dashed line in every
/// pane, its plate in a bottom corner of the magnitude view.
#[test]
fn transfer_view_cursor_mark() {
    let view = ViewState {
        cursor_mark_hz: Some(2000.0),
        ..ViewState::default()
    };
    transfer_golden("transfer_view_cursor_mark", view, stale(), "STALE · 2.4 s");
}

/// The cursor values dragged to the bottom-left at 1.5× the text size, the legend to the
/// top-right at 1.25× and the mark's readout to the bottom-right: each plate where it was
/// dropped, grown with its text, inside the plot, with its grip.
#[test]
fn transfer_view_moved_plates() {
    let mut view = ViewState {
        cursor_mark_hz: Some(2000.0),
        ..ViewState::default()
    };
    view.plates.readout.move_to(0.0, 1.0);
    view.plates.readout.set_scale(1.5);
    view.plates.legend.move_to(1.0, 0.0);
    view.plates.legend.set_scale(1.25);
    view.plates.mark.move_to(1.0, 1.0);
    transfer_golden("transfer_view_moved_plates", view, stale(), "STALE · 2.4 s");
}

#[test]
fn transfer_view_coherence_overlay() {
    let mut view = ViewState::default();
    view.tf.coherence_placement = CoherencePlacement::OverlayOnMagnitude;
    // Two banners: the strip grows by a row.
    let status = Status {
        no_delay_estimate: Some(ac2_scene::banner::NoDelayEstimate::NotFound),
        ..stale()
    };
    transfer_golden(
        "transfer_view_coherence_overlay",
        view,
        status,
        "STALE · 2.4 s",
    );
}

/// Headroom advice: an info banner under the delay notice, the cap by the max level said.
#[test]
fn transfer_view_advice() {
    use ac2_scene::advice::{Advice, Stimulus};
    let status = Status {
        no_delay_estimate: Some(ac2_scene::banner::NoDelayEstimate::NotFound),
        advice: Some(Advice::Raise {
            want_db: 18.0,
            coherence: 0.42,
            meas_rms_dbfs: -64.3,
            stimulus: Some(Stimulus {
                level_dbfs: -54.5,
                max_level_dbfs: -50.0,
            }),
            meas_calibrated: false,
        }),
        ..Status::default()
    };
    transfer_golden(
        "transfer_view_advice",
        ViewState::default(),
        status,
        "NO DELAY ESTIMATE",
    );
}

/// The valid-resolution marker of a 1/96-oct curve at 96 kHz: faint shade and dashed edges
/// under the grid and the curves, in every pane, in the dark and the light theme.
#[test]
fn transfer_view_unresolved() {
    use ac2_proto::model::{FreqRange, Resolution, Unresolved};
    use ac2_proto::units::Hz;
    let un = Unresolved {
        resolution: Resolution::NinetySixth,
        ranges: [(19.9, 135.4), (203.0, 406.0), (1625.0, 3250.0)]
            .iter()
            .map(|&(lo, hi)| FreqRange {
                lo: Hz(lo),
                hi: Hz(hi),
            })
            .collect(),
    };
    let a = cols(0.0, 0.0);
    let mut t = trace(&a, 1, 0, 0.010);
    t.unresolved = Some(ac2_scene::unresolved::Source {
        unresolved: &un,
        fill: ac2_scene::unresolved::Fill::Gaps,
    });
    for theme in [Theme::dark(), Theme::light()] {
        let name = format!("transfer_view_unresolved_{:?}", theme.name).to_lowercase();
        let Some(gpu) = gpu(&name) else {
            return;
        };
        let view = ViewState {
            cursor_hz: Some(300.0),
            ..ViewState::default()
        };
        let size = Viewport {
            width: 560.0,
            height: 360.0,
        };
        let s = ac2_scene::tf::transfer_scene(
            std::slice::from_ref(&t),
            &ac2_scene::trace::DisplayCache::default(),
            &Status::default(),
            &view,
            &theme,
            size,
        );
        assert_eq!(s.unresolved.len(), 3);
        let mut r = renderer(gpu);
        let img = render_on(gpu, &mut r, &s.scene, 1.0, theme.background);
        golden(&name, &img, TEXT);
    }
}

/// A live multi-resolution spectrum at 96 kHz: a faint dashed line at each window change,
/// labelled with the window's length at the plot foot, and the shade below what its
/// longest window resolves, in the valid-resolution marker's style.
#[test]
fn spectrum_multires_stitches() {
    spectrum_golden("spectrum_multires_stitches", SpectrumStyle::Line);
}

/// The same spectrum filled: a faint area in the trace colour under the line, down to the
/// plot bottom, with the 1 kHz tone still a spike.
#[test]
fn spectrum_fill() {
    spectrum_golden("spectrum_fill", SpectrumStyle::Fill);
}

fn spectrum_golden(name: &str, style: SpectrumStyle) {
    use ac2_proto::GridDef;
    use ac2_proto::frame::{SpecBand, SpecBands, SpecFrame, SpecMeta};
    use ac2_proto::model::{CalStatus, FreqRange, LevelScale, Resolution, Unresolved, Window};
    use ac2_proto::units::{Hz, WallNs};
    use ac2_scene::spectrum::{SpectrumTrace, spectrum_scene};
    let Some(gpu) = gpu(name) else {
        return;
    };
    let fs = 96_000.0;
    let kappa = 1.0 / (2f64.powf(1.0 / 96.0) - 2f64.powf(-1.0 / 96.0));
    let bands = SpecBands {
        bands: [65_536u32, 32_768, 16_384, 8192, 4096, 2048]
            .iter()
            .zip([8192u32, 4096, 2048, 2048, 2048, 1024])
            .map(|(&n, hop)| SpecBand {
                window: Seconds(f64::from(n) / fs),
                hop: Seconds(f64::from(hop) / fs),
                sole: Hz(2.0 * kappa * fs / f64::from(n)),
                age: Some(Seconds(0.0)),
            })
            .collect(),
        unresolved: Unresolved {
            resolution: Resolution::FortyEighth,
            ranges: vec![FreqRange {
                lo: Hz(7.8125),
                hi: Hz(101.5),
            }],
        },
    };
    let grid = GridDef::Log {
        ppo: 48,
        k_min: -7 * 48,
        k_max: 263,
    };
    let freqs = grid.centres();
    let edges = grid.edges();
    // Pink-like noise floor with a sine at 1 kHz.
    let level = freqs
        .iter()
        .map(|&f| {
            let floor = -70.0 - 3.0 * (f / 1000.0).log2() as f32;
            if (f / 1000.0).log2().abs() < 0.5 / 48.0 {
                -20.0
            } else {
                floor
            }
        })
        .collect();
    let spec = SpecFrame {
        meas: MeasId(1),
        meta: SpecMeta {
            window: Window::Hann,
            scale: LevelScale::Dbfs,
            level: ac2_proto::model::SpecLevel::Tone,
            cal: CalStatus::Uncalibrated,
            mic_curve: false,
            smoothing: None,
            math: None,
            bands: Some(Box::new(bands)),
        },
        level,
    };
    let mut t = SpectrumTrace::spectrum(
        &spec,
        ac2_scene::spectrum::LiveMeta {
            captured: WallNs(0),
            curve: None,
            freqs: &freqs,
            edges: &edges,
            name: "Main L".into(),
            color: Theme::dark().trace_color(0),
            freshness: Freshness::from_age(0.0),
        },
    );
    let theme = Theme::dark();
    t.style = style;
    let view = ViewState {
        cursor_hz: Some(1000.0),
        ..ViewState::default()
    };
    let size = Viewport {
        width: 560.0,
        height: 360.0,
    };
    let s = spectrum_scene(
        std::slice::from_ref(&t),
        &Status::default(),
        &view,
        &theme,
        size,
    );
    assert_eq!(s.stitches.len(), 6);
    let mut r = renderer(gpu);
    let img = render_on(gpu, &mut r, &s.scene, 1.0, theme.background);
    golden(name, &img, TEXT);
}

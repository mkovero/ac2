//! End to end: an `ac2-scene` view built from synthetic traces, rendered and compared with
//! a golden image. The values are asserted headless in `ac2-scene`; this checks that the
//! renderer draws what the scene says (panes, gaps, alpha, legend, banner strip above the
//! panes, coherence in its own pane or overlaid on magnitude).

mod common;

use ac2_plot::Viewport;
use ac2_proto::frame::ValidityMask;
use ac2_proto::model::Polarity;
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
        time_base: TimeBase::Shared {
            epoch: SessionEpoch(1),
            delay: Seconds(delay),
        },
        freshness: Some(Freshness::from_age(0.1)),
    }
}

/// Two traces with a cursor and a STALE banner, rendered and compared with `name`.
fn transfer_golden(name: &str, view: ViewState, status: Status) {
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
    let s = ac2_scene::tf::transfer_scene(&traces, &status, &view, &theme, size);
    assert_eq!(s.banners[0].text, "STALE · 2.4 s");
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
    transfer_golden("transfer_view", ViewState::default(), stale());
}

#[test]
fn transfer_view_coherence_overlay() {
    let mut view = ViewState::default();
    view.tf.coherence_placement = CoherencePlacement::OverlayOnMagnitude;
    // Two banners: the strip grows by a row.
    let status = Status {
        no_delay_estimate: true,
        ..stale()
    };
    transfer_golden("transfer_view_coherence_overlay", view, status);
}

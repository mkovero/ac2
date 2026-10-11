/// The plot steps full → no grid → traces only and wraps; each step names itself.
#[test]
fn plot_chrome_cycle_wraps() {
    let mut c = PlotChrome::default();
    assert_eq!(c, PlotChrome::Full);
    let mut seen = Vec::new();
    for _ in 0..3 {
        seen.push((c.name(), c.grid(), c.labels(), c.cursor()));
        c = c.next();
    }
    assert_eq!(c, PlotChrome::Full);
    assert_eq!(
        seen,
        [
            ("grid, labels and cursor", true, true, true),
            ("no grid", false, true, true),
            ("traces only", false, false, false),
        ]
    );
}

/// The time axis of an IR: zoom keeps the time under the pointer, never narrower than a
/// few samples; pan and zoom out stop one IR length outside it.
#[test]
fn ir_time_axis_bounds() {
    // 48 kHz from −1 ms, 4801 samples: −1 … 99 ms.
    let e = IrExtent::of(-0.001, 1.0 / 48_000.0, 4801);
    assert!(close(e.full.lo, -1.0) && close(e.full.hi, 99.0), "{e:?}");
    let b = e.bounds();
    assert_eq!(b.limits, Range::new(-101.0, 199.0));
    let z = b.zoom(e.full, 9.0, 4.0);
    assert!(close(z.span(), 25.0));
    assert!(close((9.0 - z.lo) / z.span(), 0.1));
    // Never narrower than four samples.
    let z = b.zoom(e.full, 0.0, 1e12);
    assert!(close(z.span(), 4.0 / 48.0), "{z:?}");
    // Out and away: the limits hold.
    assert_eq!(b.zoom(e.full, 0.0, 1e-9), b.limits);
    assert_eq!(b.pan(e.full, 1e6), Range::new(99.0, 199.0));
    assert_eq!(b.pan(e.full, -1e6), Range::new(-101.0, -1.0));
    // The nearest sample, clamped to the IR.
    assert_eq!(e.nearest(0.0), Some(48));
    assert_eq!(e.nearest(-50.0), Some(0));
    assert_eq!(e.nearest(1e9), Some(4800));
    assert_eq!(e.nearest(f64::NAN), None);
    assert!(close(e.snap(0.03).expect("snapped"), 1.0 / 48.0));
    // Cursor steps: a sample zoomed in, a hundredth of the span in whole samples out.
    assert!(close(e.cursor_step(Range::new(0.0, 0.1)), 1.0 / 48.0));
    assert!(close(e.cursor_step(e.full), 48.0 / 48.0));
    // A single sample has no span: an axis around zero to draw on.
    assert_eq!(IrExtent::of(0.0, 0.001, 1).full, Range::new(-1.0, 1.0));
}

/// The linear amplitude axis keeps a sign-symmetric picture's centre under the pointer
/// and stays inside ±100 FS.
#[test]
fn ir_amplitude_axis_bounds() {
    let b = IR_AMPLITUDE_BOUNDS;
    let r = Range::new(-0.55, 0.55);
    let z = b.zoom(r, 0.0, 2.0);
    assert!(close(z.lo, -0.275) && close(z.hi, 0.275), "{z:?}");
    assert!(close(b.pan(r, 0.1).lo, -0.45));
    assert_eq!(b.zoom(r, 0.0, 1e-9), Range::new(-100.0, 100.0));
    assert!(close(b.zoom(r, 0.0, 1e12).span(), IR_AMPLITUDE_MIN_SPAN));
}

/// dBFS and dB SPL keep their own level ranges: a calibrated spectrum starts on a
/// dB SPL-sized axis, and zooming one scale leaves the other as it was.
#[test]
fn spectrum_level_range_per_scale() {
    use ac2_proto::model::LevelScale;
    let mut v = SpectrumView::default();
    assert_eq!(v.range(LevelScale::Dbfs), Range::new(-100.0, 0.0));
    assert_eq!(v.range(LevelScale::DbSpl), Range::new(20.0, 120.0));
    *v.range_mut(LevelScale::DbSpl) = Range::new(40.0, 100.0);
    assert_eq!(v.range(LevelScale::Dbfs), Range::new(-100.0, 0.0));
    assert_eq!(v.range(LevelScale::DbSpl), Range::new(40.0, 100.0));
}

use super::*;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs())
}

#[test]
fn zoom_keeps_the_anchor() {
    let r = FreqRange::default().zoom(1000.0, 2.0);
    // 1 kHz sits at the same fraction of the log axis before and after.
    let t = |r: FreqRange| (1000f64.ln() - r.lo.ln()) / (r.hi.ln() - r.lo.ln());
    assert!((t(r) - t(FreqRange::default())).abs() < 1e-12);
    assert!(close(r.hi / r.lo, 1000f64.sqrt()));
}

/// Every view chain: next walks every view once and wraps, prev undoes next.
#[test]
fn view_chains_wrap_both_ways() {
    let mut v = SweepMode::Response;
    let mut seen = vec![];
    for _ in 0..SweepMode::ALL.len() {
        seen.push(v);
        assert_eq!(v.next().prev(), v);
        v = v.next();
    }
    assert_eq!(
        (v, seen.as_slice()),
        (SweepMode::Response, &SweepMode::ALL[..])
    );
    assert_eq!(SweepMode::Response.prev(), SweepMode::Room);
    for m in [
        SpectrumMode::Spectrum,
        SpectrumMode::Split,
        SpectrumMode::Spectrograph,
    ] {
        assert_eq!(m.next().prev(), m);
    }
    assert_eq!(SpectrumMode::Spectrum.prev(), SpectrumMode::Spectrograph);
    for bands in [false, true] {
        let modes: &[SplMode] = if bands {
            &[
                SplMode::Meter,
                SplMode::Leq,
                SplMode::MeterLeq,
                SplMode::Bands,
            ]
        } else {
            &[SplMode::Meter, SplMode::Leq, SplMode::MeterLeq]
        };
        for m in modes {
            assert_eq!(m.next(bands).prev(bands), *m, "{m:?} {bands}");
        }
    }
    assert_eq!(SplMode::Meter.prev(true), SplMode::Bands);
    assert_eq!(SplMode::Meter.prev(false), SplMode::MeterLeq);
}

#[test]
fn zoom_and_pan_are_clamped() {
    let r = FreqRange::default().zoom(1000.0, 1e9);
    assert!(close(r.hi / r.lo, FREQ_MIN_RATIO));
    let r = FreqRange::default().zoom(1000.0, 1e-9);
    assert!(close(r.lo, FREQ_LIMIT_LO) && close(r.hi, FREQ_LIMIT_HI));
    let r = FreqRange::default().pan(20.0);
    assert!(close(r.hi, FREQ_LIMIT_HI));
    assert!(close(r.hi / r.lo, 1000.0));
    let r = FreqRange::default().pan(1.0);
    assert!(close(r.lo, 40.0) && close(r.hi, 40_000.0));
}

#[test]
fn level_zoom_keeps_the_anchor_and_limits() {
    let r = Range::new(-100.0, 0.0);
    let z = level::zoom(r, -80.0, 2.0);
    assert!(close(z.span(), 50.0));
    // −80 dB stays a fifth of the way up.
    assert!(close((-80.0 - z.lo) / z.span(), 0.2));
    let back = level::zoom(z, -80.0, 0.5);
    assert!(
        close(back.lo, r.lo) && close(back.hi + 1.0, r.hi + 1.0),
        "{back:?}"
    );
    // The narrowest span and the limits hold however far it goes.
    let z = level::zoom(r, -50.0, 1e9);
    assert!(close(z.span(), LEVEL_MIN_SPAN));
    let z = level::zoom(r, -50.0, 1e-9);
    assert_eq!(z, Range::new(LEVEL_LIMIT_LO, LEVEL_LIMIT_HI));
    // Nonsense leaves it alone.
    assert_eq!(level::zoom(r, f64::NAN, 2.0), r);
    assert_eq!(level::zoom(r, -50.0, 0.0), r);
}

#[test]
fn level_pan_steps_on_round_values() {
    let r = Range::new(-100.0, 0.0);
    assert_eq!(level::pan_step(r), 10.0);
    assert_eq!(level::pan_step(Range::new(-30.0, 30.0)), 10.0);
    assert_eq!(level::pan_step(Range::new(-12.0, 0.0)), 2.0);
    assert!(close(level::pan_step(Range::new(-1.0, 0.0)), 0.1));
    assert_eq!(level::pan(r, -40.0), Range::new(-140.0, -40.0));
    // Against a limit the span stays.
    assert_eq!(level::pan(r, -1000.0), Range::new(-300.0, -200.0));
    assert_eq!(level::pan(r, 1000.0), Range::new(200.0, 300.0));
}

#[test]
fn level_fit_frames_low_signals() {
    // A spectrum of very low levels: noise near −135 dBFS, a tone at −82 dBFS, and a few
    // empty bins far below that a fit must not stretch to.
    let mut v: Vec<f64> = (0..2000).map(|i| -135.0 + f64::from(i % 7)).collect();
    v.push(-82.0);
    v.extend([-300.0, f64::NEG_INFINITY, f64::NAN, -280.0]);
    let r = level::fit(v).expect("finite values");
    assert!(r.lo <= -135.0 && r.lo >= -150.0, "{r:?}");
    assert!(r.hi >= -82.0 && r.hi <= -70.0, "{r:?}");
    // On the grid: whole multiples of the step it picked (5 dB here).
    assert_eq!((r.lo % 5.0, r.hi % 5.0), (0.0, 0.0), "{r:?}");
    // A flat trace still gets a few dB.
    let r = level::fit([3.0; 10]).expect("finite");
    assert!(
        r.span() >= LEVEL_FIT_MIN_SPAN && r.lo < 3.0 && r.hi > 3.0,
        "{r:?}"
    );
    assert_eq!(level::fit([f64::NAN]), None);
    assert_eq!(level::fit(std::iter::empty()), None);
}

#[test]
fn level_ticks_are_readable_at_any_span() {
    use crate::axis::{Steps, axis_with_density};
    let labels = |r: Range| {
        axis_with_density(r, 200.0, 0.0, "dB", Steps::Decimal, 200.0)
            .labels()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    // A 1 dB span: tenths, with a decimal.
    let l = labels(Range::new(-1.0, 0.0));
    assert!(l.contains(&"\u{2212}0.4".to_owned()), "{l:?}");
    assert!((4..=9).contains(&l.len()), "{l:?}");
    // Very low levels: whole dB at a round step.
    let l = labels(Range::new(-145.0, -75.0));
    assert!(l.contains(&"\u{2212}100".to_owned()), "{l:?}");
    assert!(l.iter().all(|s| !s.contains('.')), "{l:?}");
    // The whole limit span: a few labels, hundreds apart.
    let l = labels(Range::new(LEVEL_LIMIT_LO, LEVEL_LIMIT_HI));
    assert!(l.contains(&"0".to_owned()) && l.len() <= 8, "{l:?}");
}

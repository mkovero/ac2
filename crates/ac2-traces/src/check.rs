//! What stored trace data may hold: the checks a session or file read applies before its
//! data reaches the store, so a crafted file cannot make later math index past a column,
//! allocate without bound, divide by zero or carry a NaN the live path never produces.
//!
//! NaN in a column is the protocol's "no value" (a gap, a column whose validity mask was
//! set at capture) and passes; ±∞ never comes from a measurement and is refused, except
//! as the −∞ dB of an impulse response's all-zero stretch.

use ac2_proto::GridDef;
use ac2_proto::frame::MAX_N;
use ac2_proto::model::{DistortionCurve, SweepData, SweepInfo, TraceIr, TransferIr};

use crate::columns::{Columns, StoredTrace};

/// Largest magnitude a column may hold, ±dB. Within it every power (10^(L/10)) and
/// amplitude a trace's math forms stays finite in f64, summed over a whole grid of
/// [`MAX_N`] columns too (10^300 · 2^16 < f64::MAX); a level from f32 samples never comes
/// near it.
pub const MAX_ABS_DB: f64 = 3000.0;

/// Room-parameter bands a sweep keeps at most: octaves 63 Hz … 8 kHz are 8, thirds
/// 50 Hz … 10 kHz 24; twice that bounds a file without fixing the band plan.
const MAX_ROOM_BANDS: usize = 48;

/// Checks a grid before anything is formed on it: between 1 and [`MAX_N`] columns (the
/// most a frame or a trace table carries), counted without allocating, then centre
/// frequencies that are finite, ascending and positive (0 Hz only as the first, the DC bin
/// of an FFT grid).
pub fn grid(g: &GridDef) -> Result<(), String> {
    let max = u64::from(MAX_N);
    let fs_ok = |fs: f64| fs.is_finite() && fs > 0.0;
    let columns = match g {
        GridDef::Log { ppo, k_min, k_max } => {
            if *ppo == 0 {
                return Err("log grid with 0 points per octave".into());
            }
            u64::try_from(i64::from(*k_max) - i64::from(*k_min) + 1).unwrap_or(0)
        }
        GridDef::IecBands { centres, .. } => centres.len() as u64,
        GridDef::Linear { fs, n } | GridDef::LogBins { fs, n, .. } => {
            if !fs_ok(fs.0) || *n < 2 {
                return Err("FFT grid needs a positive finite sample rate and n ≥ 2".into());
            }
            if let GridDef::LogBins { ppo: 0, .. } = g {
                return Err("log-bins grid with 0 points per octave".into());
            }
            // Bins; a log-bins grid has at most one column per bin.
            u64::from(*n / 2 + 1)
        }
    };
    if !(1..=max).contains(&columns) {
        return Err(format!("grid of {columns} columns (1 … {max})"));
    }
    let f = g.centres();
    if f.is_empty() {
        return Err("grid without columns".into());
    }
    let mut prev: Option<f64> = None;
    for &x in &f {
        let ok = x.is_finite() && (x > 0.0 || (x == 0.0 && prev.is_none()));
        if !ok || prev.is_some_and(|p| x <= p) {
            return Err("grid frequencies are not positive, finite and ascending".into());
        }
        prev = Some(x);
    }
    Ok(())
}

/// Whether `v` is a magnitude a column may hold: NaN (no value) or finite within
/// ±[`MAX_ABS_DB`].
pub fn db_ok(v: f32) -> bool {
    v.is_nan() || (v.is_finite() && f64::from(v).abs() <= MAX_ABS_DB)
}

/// Checks columns against `len` grid columns: every array that long, magnitudes per
/// [`db_ok`], phases NaN or within ±180°, coherences NaN or within 0 … 1.
pub fn columns(c: &Columns, len: usize) -> Result<(), String> {
    if c.mag_db.len() != len || !c.consistent() {
        return Err(format!("columns are not {len} long, as the grid is"));
    }
    if !c.mag_db.iter().all(|v| db_ok(*v)) {
        return Err(format!(
            "a magnitude is infinite or beyond ±{MAX_ABS_DB} dB"
        ));
    }
    if let Some(p) = &c.phase_deg
        && !p.iter().all(|v| v.is_nan() || (-180.0..=180.0).contains(v))
    {
        return Err("a phase is outside ±180°".into());
    }
    if let Some(k) = &c.coherence
        && !k.iter().all(|v| v.is_nan() || (0.0..=1.0).contains(v))
    {
        return Err("a coherence is outside 0 … 1".into());
    }
    Ok(())
}

/// Checks a declared impulse-response length before anything is allocated for it.
pub fn ir_points(points: u32) -> Result<(), String> {
    if (1..=TraceIr::MAX_POINTS).contains(&points) {
        Ok(())
    } else {
        Err(format!(
            "impulse response of {points} points (1 … {})",
            TraceIr::MAX_POINTS
        ))
    }
}

/// Checks the timing of an impulse response: finite `t0`, finite positive `dt` (time and
/// sample index convert through it).
pub fn ir_timing(t0: f64, dt: f64) -> Result<(), String> {
    if t0.is_finite() && dt.is_finite() && dt > 0.0 {
        Ok(())
    } else {
        Err("impulse response t0 must be finite and dt finite and above 0".into())
    }
}

/// Whether `v` is an ETC value: finite, or −∞ for an all-zero stretch.
fn etc_ok(v: f32) -> bool {
    v.is_finite() || v == f32::NEG_INFINITY
}

/// Checks an impulse response: length, timing, as many ETC points as samples, finite
/// samples (the live IR zeroes non-finite bins before its inverse FFT) and ETC per
/// [`etc_ok`].
pub fn ir(ir: &TraceIr) -> Result<(), String> {
    ir_points(u32::try_from(ir.linear.len()).unwrap_or(u32::MAX))?;
    ir_timing(ir.t0.0, ir.dt.0)?;
    if ir.etc_db.len() != ir.linear.len() {
        return Err("impulse response ETC and samples differ in length".into());
    }
    if !ir.linear.iter().all(|v| v.is_finite()) {
        return Err("an impulse-response sample is not finite".into());
    }
    if !ir.etc_db.iter().all(|v| etc_ok(*v)) {
        return Err("an impulse-response ETC value is NaN or +∞".into());
    }
    Ok(())
}

/// Checks a transfer trace's impulse response and its sample rate.
pub fn transfer_ir(t: &TransferIr) -> Result<(), String> {
    if !(t.sample_rate.0.is_finite() && t.sample_rate.0 > 0.0) {
        return Err("impulse response sample rate must be finite and above 0".into());
    }
    ir(&t.ir)
}

fn curve(c: &DistortionCurve, len: usize) -> Result<(), String> {
    if c.level_db.len() != len || c.floor_db.len() != len {
        return Err(format!(
            "distortion curve is not {len} long, as the grid is"
        ));
    }
    if !c.level_db.iter().chain(&c.floor_db).all(|v| db_ok(*v)) {
        return Err(format!(
            "a distortion level is infinite or beyond ±{MAX_ABS_DB} dB"
        ));
    }
    Ok(())
}

/// Checks a sweep's analysis facts: what the analysis divides by or times from is finite
/// and positive, the rest finite.
pub fn sweep_info(i: &SweepInfo) -> Result<(), String> {
    let positive = [i.sample_rate.0, i.rate.0, i.duration.0];
    let finite = [
        i.arrival.0,
        i.reference_level.0,
        i.window_pre.0,
        i.window_post.0,
        i.gate_pre.0,
        i.gate.0,
        i.floor_margin.0,
    ];
    if !positive.iter().all(|v| v.is_finite() && *v > 0.0) || i.repeats == 0 {
        return Err(
            "sweep sample rate, rate and duration must be finite and above 0, repeats at least 1"
                .into(),
        );
    }
    if !finite.iter().all(|v| v.is_finite()) {
        return Err("a sweep analysis fact is not finite".into());
    }
    Ok(())
}

/// Checks a sweep's data against `len` grid columns: distortion curves that long, orders
/// from 2 ascending, the impulse response, the analysis facts, and bounded room bands and
/// unresolved ranges.
pub fn sweep(s: &SweepData, len: usize) -> Result<(), String> {
    let mut prev = 1u8;
    for h in &s.harmonics {
        if h.order <= prev {
            return Err("harmonic orders must ascend from 2".into());
        }
        prev = h.order;
        curve(&h.curve, len)?;
    }
    curve(&s.thd, len)?;
    ir(&s.ir)?;
    sweep_info(&s.info)?;
    if let Some(r) = &s.room
        && (r.octave.len() > MAX_ROOM_BANDS || r.third.len() > MAX_ROOM_BANDS)
    {
        return Err("more room-parameter bands than a sweep has".into());
    }
    if let Some(u) = &s.unresolved {
        for x in [&u.response, &u.harmonics] {
            // Ranges are disjoint and each covers at least one column.
            if x.ranges.len() > len {
                return Err("more unresolved ranges than columns".into());
            }
            if !x
                .ranges
                .iter()
                .all(|r| r.lo.0.is_finite() && r.hi.0.is_finite() && r.lo.0 <= r.hi.0)
            {
                return Err("an unresolved range is not finite and ordered".into());
            }
        }
    }
    Ok(())
}

/// Checks a whole stored trace: its grid (and that it is the one its metadata names), its
/// columns, sweep and impulse response on it, a finite reference delay, a finite
/// calibration sensitivity and a mic curve's normalisation frequency.
pub fn trace(t: &StoredTrace) -> Result<(), String> {
    use ac2_proto::model::CalState;
    grid(&t.grid)?;
    if t.grid.id() != t.meta.grid_id {
        return Err("data is not on the trace's grid".into());
    }
    let len = t.grid.len();
    columns(&t.columns, len)?;
    if let Some(s) = &t.sweep {
        sweep(s, len)?;
    }
    if let Some(i) = &t.ir {
        transfer_ir(i)?;
    }
    if !t.meta.delay.0.is_finite() {
        return Err("delay is not finite".into());
    }
    if let CalState::Calibrated { sensitivity, .. } = &t.meta.cal
        && !sensitivity.0.is_finite()
    {
        return Err("calibration sensitivity is not finite".into());
    }
    if let Some(mc) = &t.meta.mic_curve {
        mic_f_norm(mc.f_norm.0)?;
    }
    Ok(())
}

/// Checks a mic curve's normalisation frequency: the curve is evaluated there, so it is a
/// finite frequency above 0 Hz.
pub fn mic_f_norm(f: f64) -> Result<(), String> {
    if f.is_finite() && f > 0.0 {
        Ok(())
    } else {
        Err("mic curve normalisation frequency must be finite and above 0 Hz".into())
    }
}

//! Impulse responses as WAV files: one channel of a WAV / RF64 file imported as a transfer
//! trace that carries the samples and their frequency response, and a trace's impulse
//! response exported as a mono float WAV, so IRs move to and from other analyzers.
//!
//! The response is [`ac2_core::ir_response`]'s, on the ten-octave log grid a transfer
//! measurement uses. The trace's `delay` is the time zero of its phase re the file's first
//! sample (the peak with [`IrAlign::Peak`]), and its IR's `t0` puts that time zero at 0 in
//! the IR views, as a captured transfer trace's inserted delay is.

use ac2_core::grid::LogGrid;
use ac2_core::ir_response::{self, Align, IrResponseError, IrSpec, IrWindow};
use ac2_proto::GridDef;
use ac2_proto::ctrl::ImportProblem;
use ac2_proto::model::{
    FreqRange, ImportFormat, ImportNote, IrAlign, IrFile, IrImport, IrImportWindow, LogGridSpec,
    TraceIr, TraceKind, TransferIr, Unresolved,
};
use ac2_proto::units::{Hz, Seconds};

use crate::columns::{Columns, StoredTrace};
use crate::raw::wav::{self, WavReader};
use crate::text::{ImportError, Imported, fail};

/// The file starts like a WAV or RF64 file.
pub fn is_wav(content: &[u8]) -> bool {
    content.len() >= 12 && matches!(&content[0..4], b"RIFF" | b"RF64") && &content[8..12] == b"WAVE"
}

/// Samples kept before the peak when a file longer than [`TraceIr::MAX_POINTS`] is cut:
/// room for a speaker's pre-ringing and a crossover's group delay.
const TRUNCATE_PRE: usize = (TraceIr::MAX_POINTS / 16) as usize;

fn bad_options(msg: impl Into<String>) -> ImportError {
    fail(None, ImportProblem::BadOptions, msg)
}

/// Reads one channel of the WAV / RF64 file `content` as an impulse response.
pub fn import(content: &[u8], opts: IrImport) -> Result<Imported, ImportError> {
    let mut r = WavReader::from_bytes(content)
        .map_err(|e| fail(None, ImportProblem::NotWav, e.to_string()))?;
    let info = r.info();
    let channel = match opts.channel {
        Some(c) if c < info.channels => c,
        Some(c) => {
            return Err(fail(
                None,
                ImportProblem::PickChannel,
                format!(
                    "the file has {} channel(s); there is no channel {}",
                    info.channels,
                    u32::from(c) + 1
                ),
            ));
        }
        None if info.channels == 1 => 0,
        None => {
            return Err(fail(
                None,
                ImportProblem::PickChannel,
                format!(
                    "the file has {} channels: choose the one to import",
                    info.channels
                ),
            ));
        }
    };
    let n = usize::from(info.channels);
    let mut all: Vec<f64> = Vec::with_capacity(usize::try_from(info.frames).unwrap_or(0));
    let mut buf = vec![0.0f32; 4096 * n];
    loop {
        let got = r.read(&mut buf).map_err(|e| {
            fail(
                None,
                ImportProblem::NotWav,
                format!("unreadable samples: {e}"),
            )
        })?;
        if got == 0 {
            break;
        }
        all.extend(
            buf[..got * n]
                .chunks_exact(n)
                .map(|f| f64::from(f[usize::from(channel)])),
        );
    }
    let fs = f64::from(info.sample_rate);
    let max = TraceIr::MAX_POINTS as usize;
    let mut notes = Vec::new();
    let first = if all.len() > max {
        notes.push(ImportNote::IrTruncated);
        let p = ir_response::peak_index(&all);
        p.saturating_sub(TRUNCATE_PRE).min(all.len() - max)
    } else {
        0
    };
    let kept: Vec<f64> = all[first..all.len().min(first + max)].to_vec();
    let grid_spec = LogGridSpec::ten_octaves(opts.resolution.ppo());
    let grid = LogGrid {
        ppo: grid_spec.ppo,
        k_min: grid_spec.k_min,
        k_max: grid_spec.k_max,
    };
    let window = match opts.window {
        IrImportWindow::Whole => IrWindow::Whole,
        IrImportWindow::Gate { pre, post } => IrWindow::Gate {
            pre_s: pre.0,
            post_s: post.0,
        },
    };
    let align = match opts.align {
        IrAlign::Peak => Align::Peak,
        IrAlign::Start => Align::Start,
    };
    let resp = ir_response::analyse(
        &kept,
        fs,
        &IrSpec {
            grid,
            window,
            align,
        },
    )
    .map_err(|e| match e {
        IrResponseError::BadGate => bad_options(e.to_string()),
        _ => fail(None, ImportProblem::NoImpulse, e.to_string()),
    })?;
    let start_s = first as f64 / fs;
    let f32s = |v: &[f64]| v.iter().map(|x| *x as f32).collect::<Vec<f32>>();
    let columns = Columns {
        mag_db: f32s(&resp.magnitude_db),
        phase_deg: Some(f32s(&resp.phase_deg)),
        coherence: None,
    };
    let ir = TransferIr {
        sample_rate: Hz(fs),
        ir: TraceIr {
            t0: Seconds(-resp.delay_s),
            dt: Seconds(1.0 / fs),
            linear: kept.iter().map(|v| *v as f32).collect(),
            etc_db: f32s(&resp.etc_db),
        },
    };
    let ir_file = IrFile {
        sample_rate: Hz(fs),
        channels: info.channels,
        channel,
        frames: info.frames,
        kept: u32::try_from(kept.len()).unwrap_or(u32::MAX),
        first: first as u64,
        peak: Seconds(start_s + resp.peak_s),
        import: IrImport {
            channel: Some(channel),
            ..opts
        },
        unresolved: Unresolved {
            resolution: opts.resolution,
            ranges: resp
                .unresolved
                .iter()
                .map(|&(lo, hi)| FreqRange {
                    lo: Hz(lo),
                    hi: Hz(hi),
                })
                .collect(),
        },
    };
    Ok(Imported {
        name: None,
        format: ImportFormat::IrWav,
        kind: TraceKind::Transfer,
        grid: GridDef::Log {
            ppo: grid_spec.ppo,
            k_min: grid_spec.k_min,
            k_max: grid_spec.k_max,
        },
        columns,
        rows: kept.len(),
        delay: Some(Seconds(start_s + resp.delay_s)),
        sweep: None,
        ir: Some(ir),
        notes,
        ir_file: Some(Box::new(ir_file)),
    })
}

/// Why a trace has no impulse response to export.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum IrExportError {
    /// Neither a sweep nor a transfer trace captured with an IR.
    #[error("the trace has no impulse response (sweeps, transfer captures and imported IRs do)")]
    NoIr,
    /// Stored one point per `every` samples: the points are bucket extremes for display, not
    /// a resampled response.
    #[error(
        "the impulse response is stored decimated (one point per {every:.1} samples, longer \
         than {max} points): it is not the response at any sample rate; a shorter gate keeps \
         it whole",
        max = TraceIr::MAX_POINTS
    )]
    Decimated {
        /// Samples per stored point.
        every: f64,
    },
}

/// The trace's impulse response as a mono 32-bit float WAV at its sample rate: every stored
/// sample, as stored (an imported file's samples come back bit-exact).
pub fn export(t: &StoredTrace) -> Result<Vec<u8>, IrExportError> {
    let (fs, ir) = match (&t.ir, &t.sweep) {
        (Some(x), _) => (x.sample_rate.0, &x.ir),
        (None, Some(s)) => (s.info.sample_rate.0, &s.ir),
        (None, None) => return Err(IrExportError::NoIr),
    };
    let every = ir.dt.0 * fs;
    if (every - 1.0).abs() > 1e-6 {
        return Err(IrExportError::Decimated { every });
    }
    Ok(wav::to_bytes(1, fs.round() as u32, &ir.linear))
}

#[cfg(test)]
mod tests;

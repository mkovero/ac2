use ac2_proto::model::{CalState, TraceMeta, TraceOwner, TraceSource};
use ac2_proto::units::{TraceId, WallNs};

use super::*;

fn meta(imp: &Imported) -> TraceMeta {
    TraceMeta {
        id: TraceId(1),
        edit: crate::meta::new_edit(TraceId(1), "ir".into(), None, TraceOwner::Imported),
        kind: imp.kind,
        source: TraceSource::Imported {
            file_name: "ir.wav".into(),
            format: imp.format,
            notes: imp.notes.clone(),
            ir_file: imp.ir_file.clone(),
        },
        grid_id: imp.grid.id(),
        delay: imp.delay.unwrap_or(Seconds(0.0)),
        depth: None,
        cal: CalState::Uncalibrated,
        mic: None,
        mic_curve: None,
        created_at: WallNs(0),
    }
}

const FS: u32 = 48_000;

/// A plain PCM (format tag 1) or float (tag 3) WAV file, as a recorder or another analyzer
/// writes it, with an extra chunk before `data` that a reader must skip.
fn plain_wav(tag: u16, bits: u16, channels: u16, rate: u32, data: &[u8]) -> Vec<u8> {
    let block = channels * bits / 8;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(4 + 24 + 14 + 8 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVE");
    b.extend_from_slice(b"fmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&tag.to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
    b.extend_from_slice(&block.to_le_bytes());
    b.extend_from_slice(&bits.to_le_bytes());
    b.extend_from_slice(b"LIST");
    b.extend_from_slice(&5u32.to_le_bytes());
    b.extend_from_slice(b"abcde\0");
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(data);
    b
}

fn impulse(len: usize, at: usize, v: f32) -> Vec<f32> {
    let mut h = vec![0.0; len];
    h[at] = v;
    h
}

fn opts() -> IrImport {
    IrImport::default()
}

#[test]
fn a_float_impulse_becomes_a_flat_response_with_its_delay() {
    let bytes = wav::to_bytes(1, FS, &impulse(4800, 300, 1.0));
    assert!(is_wav(&bytes));
    let imp = import(&bytes, opts()).expect("import");
    assert_eq!(imp.format, ImportFormat::IrWav);
    assert_eq!(imp.kind, TraceKind::Transfer);
    let d = imp.delay.expect("delay").0;
    assert!((d - 300.0 / 48_000.0).abs() < 1e-9, "{d}");
    let ir = imp.ir.as_ref().expect("ir");
    assert_eq!(ir.sample_rate, Hz(48_000.0));
    assert_eq!(ir.ir.linear.len(), 4800);
    assert!((ir.ir.t0.0 + d).abs() < 1e-12);
    assert!((ir.ir.dt.0 - 1.0 / 48_000.0).abs() < 1e-15);
    let f = imp.ir_file.as_ref().expect("facts");
    assert_eq!(
        (f.channels, f.channel, f.frames, f.kept, f.first),
        (1, 0, 4800, 4800, 0)
    );
    assert!((f.peak.0 - d).abs() < 1e-12);
    let freqs = crate::columns::frequencies(&imp.grid);
    for (i, fq) in freqs.iter().enumerate() {
        let (m, p) = (
            imp.columns.mag_db[i],
            imp.columns.phase_deg.as_ref().expect("phase")[i],
        );
        if *fq < 20_000.0 {
            assert!(m.abs() < 1e-4 && p.abs() < 1e-2, "{fq} Hz: {m} dB {p}°");
        } else if *fq > 24_000.0 {
            assert!(m.is_nan(), "{fq} Hz above Nyquist: {m}");
        }
    }
}

#[test]
fn integer_files_of_every_width_read_to_full_scale() {
    // −0.5 full scale at sample 10 in 16, 24 and 32 bits.
    let cases: [(u16, Vec<u8>); 3] = [
        (16, (-16_384i16).to_le_bytes().to_vec()),
        (24, (-4_194_304i32).to_le_bytes()[..3].to_vec()),
        (32, (-1_073_741_824i32).to_le_bytes().to_vec()),
    ];
    for (bits, peak) in cases {
        let w = usize::from(bits / 8);
        let mut data = vec![0u8; 64 * w];
        data[10 * w..11 * w].copy_from_slice(&peak);
        let imp = import(&plain_wav(1, bits, 1, 44_100, &data), opts()).expect("import");
        let ir = imp.ir.expect("ir");
        assert_eq!(ir.sample_rate, Hz(44_100.0));
        assert_eq!(ir.ir.linear[10], -0.5, "{bits} bits");
        // −0.5: −6.02 dB, and the inversion is the phase (180°).
        let k = crate::columns::frequencies(&imp.grid)
            .iter()
            .position(|f| (*f - 1000.0).abs() < 1e-6)
            .expect("1 kHz");
        assert!((imp.columns.mag_db[k] + 6.0206).abs() < 1e-3, "{bits}");
        assert!(
            (imp.columns.phase_deg.as_ref().expect("phase")[k].abs() - 180.0).abs() < 1e-2,
            "{bits}"
        );
    }
    // Plain float (tag 3) as well as the extensible header ac2 writes.
    let mut data = vec![0u8; 64 * 4];
    data[40..44].copy_from_slice(&0.25f32.to_le_bytes());
    let imp = import(&plain_wav(3, 32, 1, 96_000, &data), opts()).expect("float");
    assert_eq!(imp.ir.expect("ir").ir.linear[10], 0.25);
}

#[test]
fn a_multichannel_file_needs_a_channel_chosen() {
    // Two channels: the impulse at 5 on the left, at 20 on the right.
    let mut inter = vec![0.0f32; 2 * 256];
    inter[2 * 5] = 1.0;
    inter[2 * 20 + 1] = 0.5;
    let bytes = wav::to_bytes(2, FS, &inter);
    let e = import(&bytes, opts()).expect_err("ambiguous");
    assert_eq!(e.problem, ImportProblem::PickChannel);
    assert!(e.msg.contains("2 channels"), "{}", e.msg);
    let e = import(
        &bytes,
        IrImport {
            channel: Some(2),
            ..opts()
        },
    )
    .expect_err("no such channel");
    assert_eq!(e.problem, ImportProblem::PickChannel);
    let right = import(
        &bytes,
        IrImport {
            channel: Some(1),
            ..opts()
        },
    )
    .expect("right");
    assert!((right.delay.expect("delay").0 - 20.0 / 48_000.0).abs() < 1e-9);
    let f = right.ir_file.expect("facts");
    assert_eq!((f.channels, f.channel), (2, 1));
}

#[test]
fn export_then_import_is_bit_exact() {
    let h: Vec<f32> = (0..3000)
        .map(|i| {
            let t = i as f32;
            (-(t - 200.0).abs() / 150.0).exp() * (t * 0.37).sin() + if i == 200 { 1.5 } else { 0.0 }
        })
        .collect();
    let bytes = wav::to_bytes(1, FS, &h);
    let imp = import(&bytes, opts()).expect("import");
    let trace = StoredTrace {
        meta: meta(&imp),
        grid: imp.grid.clone(),
        columns: imp.columns.clone(),
        sweep: None,
        ir: imp.ir.clone(),
        mic_curve: None,
    };
    let out = export(&trace).expect("export");
    assert_eq!(out, bytes);
    let back = import(&out, opts()).expect("again");
    let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(
        bits(&back.ir.expect("ir").ir.linear),
        bits(&imp.ir.expect("ir").ir.linear)
    );
    assert_eq!(bits(&back.columns.mag_db), bits(&imp.columns.mag_db));
}

#[test]
fn a_long_file_is_cut_around_its_peak() {
    let len = TraceIr::MAX_POINTS as usize + 100_000;
    let peak = 60_000;
    let bytes = wav::to_bytes(1, FS, &impulse(len, peak, 1.0));
    let imp = import(&bytes, opts()).expect("import");
    assert_eq!(imp.notes, vec![ImportNote::IrTruncated]);
    let f = imp.ir_file.expect("facts");
    assert_eq!(f.kept, TraceIr::MAX_POINTS);
    assert_eq!(f.first, (peak - TRUNCATE_PRE) as u64);
    // The delay stays re the file's first sample.
    assert!((imp.delay.expect("delay").0 - peak as f64 / 48_000.0).abs() < 1e-9);
    assert!((f.peak.0 - peak as f64 / 48_000.0).abs() < 1e-9);
}

#[test]
fn refusals() {
    assert_eq!(
        import(b"RIFF....WAVEjunk", opts()).expect_err("x").problem,
        ImportProblem::NotWav
    );
    let silent = wav::to_bytes(1, FS, &[0.0; 64]);
    assert_eq!(
        import(&silent, opts()).expect_err("x").problem,
        ImportProblem::NoImpulse
    );
    let gate = IrImport {
        window: IrImportWindow::Gate {
            pre: Seconds(0.0),
            post: Seconds(0.0),
        },
        ..opts()
    };
    let one = wav::to_bytes(1, FS, &impulse(64, 3, 1.0));
    assert_eq!(
        import(&one, gate).expect_err("x").problem,
        ImportProblem::BadOptions
    );
}

#[test]
fn a_trace_without_an_ir_or_with_a_decimated_one_is_not_exported() {
    let bytes = wav::to_bytes(1, FS, &impulse(64, 3, 1.0));
    let imp = import(&bytes, opts()).expect("import");
    let mut trace = StoredTrace {
        meta: meta(&imp),
        grid: imp.grid.clone(),
        columns: imp.columns.clone(),
        sweep: None,
        ir: None,
        mic_curve: None,
    };
    assert_eq!(export(&trace), Err(IrExportError::NoIr));
    let mut ir = imp.ir.expect("ir");
    ir.ir.dt = Seconds(3.0 / 48_000.0);
    trace.ir = Some(ir);
    assert!(
        matches!(export(&trace), Err(IrExportError::Decimated { every }) if (every - 3.0).abs() < 1e-9)
    );
}

#[test]
fn a_sweep_keeps_as_many_ir_samples_as_a_trace_holds() {
    // A sweep IR within the limit is stored sample for sample, so it exports.
    assert_eq!(ac2_core::sweep::IR_POINTS, TraceIr::MAX_POINTS as usize);
}

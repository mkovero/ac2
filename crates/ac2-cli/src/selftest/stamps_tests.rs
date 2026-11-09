use std::sync::atomic::AtomicBool;

use ac2_audio::fake::{FakeDrive, Pace};
use ac2_audio::{BlockFlags, BlockHeader, DeviceSelector, FakeBackend, FakeConfig};
use ac2_traces::raw::WavReader;

use super::*;

const RATE: u32 = 48_000;
const BLOCK: u32 = 256;

fn backend(drive: FakeDrive, audio_clock_ppm: f64) -> FakeBackend {
    FakeBackend::new(FakeConfig {
        drive,
        sample_rate: RATE,
        block_frames: BLOCK,
        inputs: 2,
        outputs: 2,
        audio_clock_ppm,
        ..FakeConfig::default()
    })
    .expect("fake")
}

fn spec(keep: Option<u16>) -> StampsSpec {
    StampsSpec {
        input_device: DeviceSelector::Default,
        input_map: vec![0, 1],
        sample_rate: None,
        buffer_frames: None,
        keep,
    }
}

fn provenance() -> Provenance {
    Provenance {
        host: "test".into(),
        ac2_build: "test-build".into(),
    }
}

fn meta_json(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(dir.join("meta.json")).expect("meta")).expect("json")
}

/// Steps the fake device `blocks` blocks, draining and reading a pair per chunk as the
/// run loop would; returns the logger finished and the number of pair reads.
fn log_blocks(b: &FakeBackend, dir: &Path, keep: Option<u16>, blocks: u64) -> (StampsMeta, u64) {
    let mut log = Logger::open(b, &spec(keep), dir, provenance()).expect("open");
    let mut driver = b.take_driver().expect("manual driver");
    let (mut done, mut reads) = (0, 0);
    while done < blocks {
        let n = (blocks - done).min(64);
        driver.run_blocks(n);
        log.poll().expect("poll");
        log.pair().expect("pair");
        reads += 1;
        done += n;
    }
    (log.finish(false, 1.0).expect("finish"), reads)
}

/// Every block reaches the file in order, the fake's clock (50 ppm fast against the stamp
/// clock, stamps exact to the ns rounding) comes back from the fit, and meta.json says so.
#[test]
fn a_manual_run_logs_every_block_and_fits_the_fake_clock() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("run");
    let b = backend(FakeDrive::Manual, 50.0);
    let (meta, reads) = log_blocks(&b, &dir, None, 2000);

    let len = fs::metadata(dir.join("blocks.bin")).expect("blocks").len();
    assert_eq!(len, (HEADER_BYTES + 2000 * RECORD_BYTES) as u64);
    let mut recs = Vec::new();
    read_blocks(&dir.join("blocks.bin"), |r| recs.push(r)).expect("read");
    assert_eq!(recs.len(), 2000);
    for (i, r) in recs.iter().enumerate() {
        assert_eq!(r.start_sample, i as u64 * u64::from(BLOCK));
        assert_eq!(r.frames, BLOCK);
        assert!(r.capture_ns.is_some());
    }

    let end = meta.end.as_ref().expect("end");
    assert_eq!(end.capture.blocks, 2000);
    assert_eq!(end.capture.index_gaps, 0);
    assert_eq!(end.fit.points, 2000);
    assert_eq!(end.fit.segments, 1);
    let rate = end.fit.rate.expect("rate");
    assert!((rate.ppm - 50.0).abs() < 0.01, "{rate:?}");
    let sigma = end.fit.residual_sigma_ns.expect("sigma");
    assert!(sigma < 1.0, "residual {sigma} ns");

    let json = meta_json(&dir);
    assert_eq!(json["sample_rate"], RATE);
    assert_eq!(json["inputs"], serde_json::json!([1, 2]));
    assert_eq!(json["host"], "test");
    assert_eq!(json["end"]["capture"]["blocks"], 2000);
    assert!(json["stamp_clock"].as_str().is_some());

    let pairs = fs::read_to_string(dir.join("pairs.csv")).expect("pairs");
    let mut lines = pairs.lines();
    assert_eq!(lines.next(), Some(PAIRS_HEADER));
    let rows: Vec<&str> = lines.collect();
    assert_eq!(rows.len() as u64, end.pairs.kept);
    assert_eq!(
        end.pairs.kept + end.pairs.dropped + end.pairs.unavailable,
        reads
    );
    for row in rows {
        let v: Vec<u64> = row.split(',').map(|f| f.parse().expect("u64")).collect();
        assert_eq!(v.len(), 6, "{row}");
        assert!(v[2] <= MAX_BRACKET_NS, "{row}");
    }
    if cfg!(target_os = "linux") {
        // The fake's stamp clock is simulated, so most reads are quick and kept.
        assert!(end.pairs.kept > 0);
    }
}

/// The kept input is in the WAV, every frame, from the first block's index.
#[test]
fn keep_writes_the_channel_and_its_first_index() {
    let tmp = tempfile::tempdir().expect("tmp");
    let b = backend(FakeDrive::Manual, 0.0);
    let (meta, _) = log_blocks(&b, tmp.path(), Some(1), 100);
    let k = meta.keep.expect("kept");
    assert_eq!(k.channel, 2);
    assert_eq!(k.first_start_sample, Some(0));
    assert_eq!(k.frames, 100 * u64::from(BLOCK));
    let info = WavReader::open(&tmp.path().join("keep.wav"))
        .expect("wav")
        .info();
    assert_eq!(info.channels, 1);
    assert_eq!(info.frames, 100 * u64::from(BLOCK));
    assert_eq!(info.sample_rate, RATE);
    assert_eq!(meta_json(tmp.path())["keep"]["first_start_sample"], 0);
}

#[test]
fn a_keep_outside_the_inputs_and_a_used_directory_are_refused() {
    let tmp = tempfile::tempdir().expect("tmp");
    let b = backend(FakeDrive::Manual, 0.0);
    assert!(matches!(
        Logger::open(&b, &spec(Some(5)), tmp.path(), provenance()),
        Err(StampsError::KeepNotCaptured(5))
    ));
    log_blocks(&b, tmp.path(), None, 4);
    let b = backend(FakeDrive::Manual, 0.0);
    assert!(matches!(
        Logger::open(&b, &spec(None), tmp.path(), provenance()),
        Err(StampsError::Exists(_))
    ));
}

/// A stretch broken by an xrun is fitted about its own mean: the jump between stretches is
/// no scatter.
#[test]
fn the_fit_pools_stretches_across_a_break() {
    let tmp = tempfile::tempdir().expect("tmp");
    let path = tmp.path().join("blocks.bin");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(RECORD_BYTES as u32).to_le_bytes());
    let ns_per_frame = 1e9 / f64::from(RATE);
    let mut rec = [0u8; RECORD_BYTES];
    for i in 0..200u64 {
        // After block 100 the stamps jump by 3 ms and the index by 1000 frames.
        let (start, extra_ns, flags) = match i {
            0..100 => (i * 256, 0, BlockFlags::NONE),
            100 => (i * 256 + 1000, 3_000_000, BlockFlags::XRUN),
            _ => (i * 256 + 1000, 3_000_000, BlockFlags::NONE),
        };
        let h = BlockHeader {
            start_sample: start,
            frames: 256,
            channels: 1,
            flags,
            callback_ns: 0,
            capture_ns: Some(1_000_000_000 + (start as f64 * ns_per_frame) as u64 + extra_ns),
        };
        encode(&h, &mut rec);
        bytes.extend_from_slice(&rec);
    }
    fs::write(&path, bytes).expect("write");
    let fit = fit_file(&path, f64::from(RATE)).expect("fit");
    assert_eq!(fit.points, 200);
    assert_eq!(fit.segments, 2);
    assert!(fit.rate.expect("rate").ppm.abs() < 0.01, "{fit:?}");
    assert!(fit.residual_sigma_ns.expect("sigma") < 1.0, "{fit:?}");
}

/// The run loop on a real-time fake: it ends on time, and the file holds what meta counts.
#[test]
fn a_timed_run_closes_its_files() {
    let tmp = tempfile::tempdir().expect("tmp");
    let b = backend(FakeDrive::Thread(Pace::Realtime), 0.0);
    let stop = AtomicBool::new(false);
    let meta = run(
        &b,
        &spec(None),
        tmp.path(),
        provenance(),
        Duration::from_millis(1200),
        &stop,
    )
    .expect("run");
    let end = meta.end.expect("end");
    assert!(!end.interrupted);
    assert!(end.capture.blocks > 50, "{}", end.capture.blocks);
    let mut n = 0u64;
    read_blocks(&tmp.path().join("blocks.bin"), |_| n += 1).expect("read");
    assert_eq!(n, end.capture.blocks);
}

/// Ctrl-C (the stop flag) ends the run as interrupted, files still whole.
#[test]
fn a_stopped_run_is_interrupted_and_finished() {
    let tmp = tempfile::tempdir().expect("tmp");
    let b = backend(FakeDrive::Thread(Pace::Realtime), 0.0);
    let stop = AtomicBool::new(true);
    let meta = run(
        &b,
        &spec(None),
        tmp.path(),
        provenance(),
        Duration::from_secs(60),
        &stop,
    )
    .expect("run");
    assert!(meta.end.expect("end").interrupted);
    assert_eq!(meta_json(tmp.path())["end"]["interrupted"], true);
}

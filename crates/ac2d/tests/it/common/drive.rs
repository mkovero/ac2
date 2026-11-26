//! Driving the fake backend by hand and reading where its audio stands.

use super::*;

/// Takes the manual driver the daemon's last stream open parked.
pub fn driver(b: &FakeBackend) -> FakeDriver {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(d) = b.take_driver() {
            return d;
        }
        assert!(Instant::now() < deadline, "no driver parked");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Runs `seconds` of device time in 50 ms steps, each once the daemon has taken the last.
pub fn run(d: &mut FakeDriver, seconds: f64) {
    let blocks = (seconds * f64::from(FS) / f64::from(BLOCK)).ceil() as u64;
    let chunk = (0.05 * f64::from(FS) / f64::from(BLOCK)).ceil() as u64;
    let mut done = 0;
    while done < blocks {
        let n = chunk.min(blocks - done);
        d.run_blocks(n);
        done += n;
        taken(d);
    }
}

/// Waits until the daemon has read every captured block. Stepping on regardless would
/// overflow the capture ring whenever the machine is busy, and the tests would see lost
/// audio they never scripted.
pub fn taken(d: &FakeDriver) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while d.capture_queued() > 0 {
        assert!(
            Instant::now() < deadline,
            "the daemon stopped reading audio"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// One past the last sample the driver has produced.
pub fn end_sample(d: &FakeDriver) -> u64 {
    d.blocks() * u64::from(BLOCK)
}

/// The stamp (`audio_sample`) of the newest spectrum a fixed-length (`fft_len`) spectrum
/// job has formed from the audio so far, its frames carrying `applied_at` as the sample
/// their configuration applied from. The job starts its analysis at the first point of the
/// device's frame grid ([`SpectrumConfig::frame_hop`] samples apart) at or after that
/// sample and forms a spectrum every display hop from there once its window has filled,
/// stamped at the spectrum's last sample. Spectra a hop apart are of different audio, so a
/// live frame compared with a capture (which holds the newest) is selected by this exact
/// stamp: a frame merely within a hop of the newest audio may be the one before it.
pub fn newest_spectrum_stamp(d: &FakeDriver, applied_at: u64, fft_len: u32) -> u64 {
    let grid = u64::from(SpectrumConfig::frame_hop(FS, BLOCK));
    let hop = u64::from(SpectrumConfig::display_hop(fft_len, FS, BLOCK));
    let origin = applied_at.div_ceil(grid) * grid;
    let end = end_sample(d);
    let newest = origin + end.saturating_sub(origin) / hop * hop;
    assert!(
        newest - origin >= u64::from(fft_len),
        "no {fft_len}-point spectrum between samples {origin} and {end}"
    );
    newest - 1
}

/// Waits until the daemon's keepalive reports audio up to `end` taken in (any epoch).
pub fn handed_on(ka: &Sub, end: u64) {
    ka.frame(Duration::from_secs(30), |f| {
        matches!(f.data, ac2_proto::frame::FrameData::Ka(_)) && f.stamp.audio_sample.0 + 1 >= end
    })
    .unwrap_or_else(|| panic!("audio up to {end} never arrived"));
}

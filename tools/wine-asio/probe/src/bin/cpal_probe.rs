//! ASIO callback probe: opens an input and an output cpal stream on one ASIO driver,
//! writes silence for a few seconds, and records every callback (which stream, frames,
//! cpal timestamps, the driver's ASIO sample position, thread id, order). The record is
//! printed afterwards, so the callbacks themselves only write into a preallocated table.
//!
//! The question it answers: do input and output callbacks come from the same ASIO
//! bufferSwitch (alternating, equal frame counts, equal timestamps), and do the timestamps
//! carry enough to give both directions one exact shared sample index.

#[cfg(not(windows))]
fn main() {
    eprintln!("cpal_probe only does something on Windows (or under Wine)");
}

#[cfg(windows)]
fn main() {
    if let Err(e) = probe::run() {
        eprintln!("probe failed: {e}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod probe {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{BufferSize, SampleFormat, StreamConfig, StreamInstant};

    const RUN_FOR: Duration = Duration::from_secs(5);
    /// Room for 5 s of callbacks at 32-frame buffers, both directions, with margin.
    const MAX_RECORDS: usize = 32_768;
    const FIELDS: usize = 8;

    /// Flat lock-free record table: callbacks claim a row with one fetch_add and store
    /// plain integers, so the audio thread never allocates or locks.
    struct Table {
        next: AtomicUsize,
        cells: Vec<AtomicU64>,
    }

    impl Table {
        fn new() -> Self {
            Self {
                next: AtomicUsize::new(0),
                cells: (0..MAX_RECORDS * FIELDS)
                    .map(|_| AtomicU64::new(0))
                    .collect(),
            }
        }
        fn push(&self, row: [u64; FIELDS]) {
            let i = self.next.fetch_add(1, Ordering::Relaxed);
            if i >= MAX_RECORDS {
                return;
            }
            for (k, v) in row.into_iter().enumerate() {
                self.cells[i * FIELDS + k].store(v, Ordering::Relaxed);
            }
        }
        fn rows(&self) -> Vec<[u64; FIELDS]> {
            let n = self.next.load(Ordering::Acquire).min(MAX_RECORDS);
            (0..n)
                .map(|i| {
                    std::array::from_fn(|k| self.cells[i * FIELDS + k].load(Ordering::Relaxed))
                })
                .collect()
        }
    }

    // Row layout.
    const KIND: usize = 0; // 0 = input, 1 = output
    const FRAMES: usize = 1;
    const CB_NS: usize = 2; // cpal callback instant
    const HW_NS: usize = 3; // cpal capture (input) or playback (output) instant
    const SPOS: usize = 4; // ASIOGetSamplePosition sample position
    const STIME: usize = 5; // ASIOGetSamplePosition system time (ns)
    const WALL_NS: usize = 6; // Instant::now() since probe start
    const TID: usize = 7; // Win32 thread id

    #[repr(C)]
    #[derive(Default)]
    struct AsioU64 {
        hi: u32,
        lo: u32,
    }
    impl AsioU64 {
        fn get(&self) -> u64 {
            (u64::from(self.hi) << 32) | u64::from(self.lo)
        }
    }

    unsafe extern "C" {
        // From the ASIO SDK's common/asio.cpp, linked statically by asio-sys. cpal does not
        // surface the sample position, and it is the only clock that would give input and
        // output one exact shared sample index.
        #[link_name = "?ASIOGetSamplePosition@@YAJPEAUASIOSamples@@PEAUASIOTimeStamp@@@Z"]
        fn ASIOGetSamplePosition(s_pos: *mut AsioU64, t_stamp: *mut AsioU64) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
    }

    fn sample_position() -> (u64, u64) {
        let mut s = AsioU64::default();
        let mut t = AsioU64::default();
        // SAFETY: called from inside a buffer callback, while the driver is loaded and
        // running; both out-pointers are valid for the call.
        let r = unsafe { ASIOGetSamplePosition(&mut s, &mut t) };
        if r != 0 {
            (u64::MAX, u64::from(r as u32))
        } else {
            (s.get(), t.get())
        }
    }

    fn ns(t: StreamInstant) -> u64 {
        t.duration_since(StreamInstant::ZERO).as_nanos() as u64
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        println!("hosts available: {:?}", cpal::available_hosts());
        let host = cpal::host_from_id(cpal::HostId::Asio)?;
        let devices: Vec<_> = host.devices()?.collect();
        println!("ASIO devices: {}", devices.len());
        for d in &devices {
            let name = d
                .description()
                .map(|x| x.name().to_string())
                .unwrap_or_default();
            println!("  device {name:?} id={:?}", d.id().ok());
            match d.default_input_config() {
                Ok(c) => println!("    default input : {c:?}"),
                Err(e) => println!("    default input : error {e}"),
            }
            match d.default_output_config() {
                Ok(c) => println!("    default output: {c:?}"),
                Err(e) => println!("    default output: error {e}"),
            }
            if let Ok(cs) = d.supported_input_configs() {
                for c in cs {
                    println!("    supported input : {c:?}");
                }
            }
            if let Ok(cs) = d.supported_output_configs() {
                for c in cs {
                    println!("    supported output: {c:?}");
                }
            }
        }
        let device = devices
            .into_iter()
            .find(|d| {
                d.description()
                    .map(|x| x.name().to_ascii_lowercase().contains("wineasio"))
                    .unwrap_or(false)
            })
            .ok_or("no WineASIO device")?;

        let in_def = device.default_input_config()?;
        let out_def = device.default_output_config()?;
        let in_cfg = StreamConfig {
            channels: in_def.channels(),
            sample_rate: in_def.sample_rate(),
            buffer_size: BufferSize::Default,
        };
        let out_cfg = StreamConfig {
            channels: out_def.channels(),
            sample_rate: out_def.sample_rate(),
            buffer_size: BufferSize::Default,
        };
        let in_fmt: SampleFormat = in_def.sample_format();
        let out_fmt: SampleFormat = out_def.sample_format();
        println!("opening input {in_cfg:?} {in_fmt:?}, output {out_cfg:?} {out_fmt:?}");

        let table: &'static Table = Box::leak(Box::new(Table::new()));
        let errors: &'static Mutex<Vec<String>> = Box::leak(Box::new(Mutex::new(Vec::new())));
        let t0 = Instant::now();
        let in_ch = u64::from(in_cfg.channels);
        let out_ch = u64::from(out_cfg.channels);

        let input = device.build_input_stream_raw(
            in_cfg,
            in_fmt,
            move |data, info| {
                let (spos, stime) = sample_position();
                let ts = info.timestamp();
                table.push([
                    0,
                    data.len() as u64 / in_ch,
                    ns(ts.callback),
                    ns(ts.capture),
                    spos,
                    stime,
                    t0.elapsed().as_nanos() as u64,
                    // SAFETY: plain Win32 call without arguments.
                    u64::from(unsafe { GetCurrentThreadId() }),
                ]);
            },
            move |e| errors.lock().unwrap().push(format!("input: {e}")),
            None,
        )?;
        let output = device.build_output_stream_raw(
            out_cfg,
            out_fmt,
            move |data, info| {
                // Silence only: every sample format cpal uses for ASIO is zero at all-zero bytes.
                data.bytes_mut().fill(0);
                let (spos, stime) = sample_position();
                let ts = info.timestamp();
                table.push([
                    1,
                    data.len() as u64 / out_ch,
                    ns(ts.callback),
                    ns(ts.playback),
                    spos,
                    stime,
                    t0.elapsed().as_nanos() as u64,
                    // SAFETY: plain Win32 call without arguments.
                    u64::from(unsafe { GetCurrentThreadId() }),
                ]);
            },
            move |e| errors.lock().unwrap().push(format!("output: {e}")),
            None,
        )?;
        println!("main thread id {}", unsafe { GetCurrentThreadId() });
        input.play()?;
        output.play()?;
        std::thread::sleep(RUN_FOR);
        drop(input);
        drop(output);

        report(&table.rows(), in_def.sample_rate());
        for e in errors.lock().unwrap().iter() {
            println!("stream error: {e}");
        }
        Ok(())
    }

    fn report(rows: &[[u64; FIELDS]], rate: u32) {
        println!("# callbacks recorded: {}", rows.len());
        println!(
            "#  seq dir frames      cb_ns            hw_ns     asio_spos   asio_stime_ns      wall_ns   tid"
        );
        for (i, r) in rows.iter().enumerate() {
            let dir = if r[KIND] == 0 { "in " } else { "out" };
            println!(
                "{i:6} {dir} {:6} {:>14} {:>14} {:>12} {:>15} {:>12} {:>5}",
                r[FRAMES], r[CB_NS], r[HW_NS], r[SPOS] as i64, r[STIME], r[WALL_NS], r[TID]
            );
        }

        let ins: Vec<_> = rows.iter().filter(|r| r[KIND] == 0).collect();
        let outs: Vec<_> = rows.iter().filter(|r| r[KIND] == 1).collect();
        println!("# summary");
        println!("inputs {}, outputs {}", ins.len(), outs.len());
        let alternating = rows.windows(2).filter(|w| w[0][KIND] != w[1][KIND]).count();
        println!(
            "adjacent pairs with different direction: {alternating} of {}",
            rows.len().saturating_sub(1)
        );
        let mut tids: Vec<u64> = rows.iter().map(|r| r[TID]).collect();
        tids.sort_unstable();
        tids.dedup();
        println!("distinct callback thread ids: {tids:?}");
        let first = rows
            .first()
            .map(|r| if r[KIND] == 0 { "input" } else { "output" });
        println!("first callback: {first:?}");

        // Pair callbacks by cpal callback instant: one bufferSwitch hands both streams the
        // same ASIO system time.
        let (mut same_cb, mut same_spos, mut same_frames, mut paired) = (0, 0, 0, 0);
        for i in &ins {
            if let Some(o) = outs.iter().find(|o| o[CB_NS] == i[CB_NS]) {
                paired += 1;
                same_cb += 1;
                same_spos += usize::from(o[SPOS] == i[SPOS]);
                same_frames += usize::from(o[FRAMES] == i[FRAMES]);
            }
        }
        println!(
            "input callbacks with an output of identical callback instant: {same_cb} of {}; \
             of those same sample position {same_spos}, same frame count {same_frames} (paired {paired})",
            ins.len()
        );

        for (name, list) in [("input", &ins), ("output", &outs)] {
            let mut spos_step_ok = 0;
            let mut cb_dt_err_max_ns: i64 = 0;
            let mut gaps = 0;
            for w in list.windows(2) {
                let frames = w[0][FRAMES];
                let dspos = w[1][SPOS].wrapping_sub(w[0][SPOS]);
                if dspos == frames {
                    spos_step_ok += 1;
                } else {
                    gaps += 1;
                }
                let expect_ns = (frames as f64 * 1e9 / f64::from(rate)) as i64;
                let dt = w[1][CB_NS] as i64 - w[0][CB_NS] as i64;
                cb_dt_err_max_ns = cb_dt_err_max_ns.max((dt - expect_ns).abs());
            }
            println!(
                "{name}: sample position steps == frames: {spos_step_ok} of {} (other {gaps}); \
                 max |callback-instant step - frames/rate| = {:.3} ms",
                list.len().saturating_sub(1),
                cb_dt_err_max_ns as f64 / 1e6
            );
        }
    }
}

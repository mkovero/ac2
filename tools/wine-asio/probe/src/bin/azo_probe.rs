//! ASIO probe on `azo` (direct COM, no Steinberg SDK). Loads the WineASIO driver, prints
//! its channels, creates buffers for 2 in + 2 out and runs for a few seconds, recording per
//! bufferSwitch: buffer index, the ASIOTime sample position / system time handed to
//! bufferSwitchTimeInfo, the driver's getSamplePosition, thread id, and wall time.
//!
//! Output is silence unless `PROBE_IMPULSE=1`: then output 1 carries a single -20 dBFS
//! sample every `PULSE_EVERY` buffers and input 1 is scanned for it, so a JACK connection
//! out_1 -> in_1 (made by run.sh, inside the JACK dummy graph) measures the loopback delay
//! in samples on the driver's own sample-position clock.
//!
//! Env: PROBE_SECONDS (5), PROBE_CYCLES (1: stop/start cycles on the same buffers),
//! PROBE_PLAIN=1 (refuse kAsioSupportsTimeInfo so the driver calls plain bufferSwitch).

#[cfg(not(windows))]
fn main() {
    eprintln!("azo_probe only does something on Windows (or under Wine)");
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
    use std::ffi::{c_long, c_void};
    use std::ptr::null_mut;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use azo::driver::{self, Driver, UnsafeHandle};
    use azo::dto::ChannelId;
    use azo::sys::{Bool, Callbacks, MessageSelector, SampleRate, SampleType, Time};

    const MAX_RECORDS: usize = 65_536;
    const FIELDS: usize = 12;
    const IN_CH: usize = 2;
    const OUT_CH: usize = 2;
    /// Impulse every this many buffers; long enough that the echo of one pulse arrives
    /// before the next is sent, for any loopback shorter than this many buffers.
    const PULSE_EVERY: u64 = 32;
    /// Offset of the impulse inside its buffer: nonzero, so an off-by-one-buffer error
    /// and an off-by-one-sample error look different.
    const PULSE_FRAME: usize = 17;
    const PULSE_LEVEL: f32 = 0.1;

    // Row layout.
    const MODE: usize = 0; // 1 = bufferSwitchTimeInfo, 0 = bufferSwitch
    const BUF_IDX: usize = 1;
    const TI_SPOS: usize = 2; // ASIOTime.timeInfo.samplePosition
    const TI_STIME: usize = 3; // ASIOTime.timeInfo.systemTime (ns)
    const TI_FLAGS: usize = 4;
    const GSP_POS: usize = 5; // getSamplePosition, called inside the callback
    const GSP_TIME: usize = 6;
    const WALL_NS: usize = 7;
    const TID: usize = 8;
    const PULSE_OUT: usize = 9; // frame of the impulse written this buffer, or -1
    const PULSE_IN: usize = 10; // first frame of input 1 above threshold, or -1
    const CYCLE: usize = 11;

    /// Flat lock-free record table: the callback claims a row with one fetch_add and stores
    /// plain integers, so it never allocates or locks.
    struct Table {
        next: AtomicUsize,
        cells: Vec<AtomicI64>,
    }
    impl Table {
        fn push(&self, row: [i64; FIELDS]) {
            let i = self.next.fetch_add(1, Ordering::Relaxed);
            if i < MAX_RECORDS {
                for (k, v) in row.into_iter().enumerate() {
                    self.cells[i * FIELDS + k].store(v, Ordering::Relaxed);
                }
            }
        }
        fn rows(&self) -> Vec<[i64; FIELDS]> {
            let n = self.next.load(Ordering::Acquire).min(MAX_RECORDS);
            (0..n)
                .map(|i| {
                    std::array::from_fn(|k| self.cells[i * FIELDS + k].load(Ordering::Relaxed))
                })
                .collect()
        }
    }

    /// The driver's COM pointer, for calls from the driver's own callback thread. ASIO
    /// drivers are in-process and are called from their callback thread by every host
    /// (the SDK's own `ASIOGetSamplePosition` included), so no apartment marshalling.
    struct Shared(UnsafeHandle);
    // SAFETY: see above; the handle is only used while the driver is loaded and running.
    unsafe impl Send for Shared {}
    unsafe impl Sync for Shared {}

    static DRIVER: OnceLock<Shared> = OnceLock::new();
    static TABLE: OnceLock<Table> = OnceLock::new();
    static IN_BUFS: [[AtomicPtr<c_void>; 2]; IN_CH] =
        [const { [const { AtomicPtr::new(null_mut()) }; 2] }; IN_CH];
    static OUT_BUFS: [[AtomicPtr<c_void>; 2]; OUT_CH] =
        [const { [const { AtomicPtr::new(null_mut()) }; 2] }; OUT_CH];
    static FRAMES: AtomicUsize = AtomicUsize::new(0);
    static IS_F32: AtomicBool = AtomicBool::new(true);
    static IMPULSE: AtomicBool = AtomicBool::new(false);
    static TIME_INFO: AtomicBool = AtomicBool::new(true);
    static SWITCHES: AtomicU64 = AtomicU64::new(0);
    static CYCLE_NO: AtomicI64 = AtomicI64::new(0);
    static MESSAGES: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];
    static T0: OnceLock<Instant> = OnceLock::new();

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
    }

    static CALLBACKS: Callbacks = Callbacks {
        buffer_switch,
        sample_rate_did_change,
        asio_message,
        buffer_switch_time_info,
    };

    unsafe extern "system" fn buffer_switch(index: c_long, _direct: Bool) {
        on_switch(index, None);
    }

    unsafe extern "system" fn buffer_switch_time_info(
        params: *mut Time,
        index: c_long,
        _direct: Bool,
    ) -> *mut Time {
        // SAFETY: the driver passes a valid ASIOTime for the duration of the call.
        let t = unsafe { params.as_ref() };
        on_switch(
            index,
            t.map(|t| {
                (
                    i64::from(t.time_info.sample_position),
                    i64::from(t.time_info.system_time),
                    t.time_info.flags.bits() as i64,
                )
            }),
        );
        params
    }

    unsafe extern "system" fn sample_rate_did_change(_rate: SampleRate) {
        MESSAGES[31].fetch_add(1, Ordering::Relaxed);
    }

    unsafe extern "system" fn asio_message(
        selector: MessageSelector,
        value: c_long,
        _message: *const c_void,
        _opt: *const f64,
    ) -> c_long {
        let s = c_long::from(selector);
        if (0..31).contains(&s) {
            MESSAGES[s as usize].fetch_add(1, Ordering::Relaxed);
        }
        let time_info = TIME_INFO.load(Ordering::Relaxed);
        match selector {
            MessageSelector::SELECTOR_SUPPORTED => {
                let v = MessageSelector::from(value);
                let yes = v == MessageSelector::ENGINE_VERSION
                    || v == MessageSelector::RESET_REQUEST
                    || v == MessageSelector::RESYNC_REQUEST
                    || v == MessageSelector::LATENCIES_CHANGED
                    || v == MessageSelector::BUFFER_SIZE_CHANGE
                    || (v == MessageSelector::SUPPORTS_TIME_INFO && time_info);
                Bool::from(yes).0
            }
            MessageSelector::ENGINE_VERSION => 2,
            MessageSelector::SUPPORTS_TIME_INFO => Bool::from(time_info).0,
            MessageSelector::RESET_REQUEST
            | MessageSelector::RESYNC_REQUEST
            | MessageSelector::LATENCIES_CHANGED => Bool::TRUE.0,
            _ => Bool::FALSE.0,
        }
    }

    fn on_switch(index: c_long, time_info: Option<(i64, i64, i64)>) {
        let wall = T0.get().map_or(0, |t| t.elapsed().as_nanos() as i64);
        let n = SWITCHES.fetch_add(1, Ordering::Relaxed);
        let frames = FRAMES.load(Ordering::Relaxed);
        let idx = (index & 1) as usize;
        let (gsp_pos, gsp_time) = match DRIVER.get().map(|d| d.0.sample_position()) {
            Some(Ok(p)) => (p.position, p.time_stamp),
            Some(Err(e)) => (-1, i64::from(c_long::from(e.code()))),
            None => (-1, -1),
        };
        let is_f32 = IS_F32.load(Ordering::Relaxed);

        // Input 1: first sample above half the pulse level.
        let mut pulse_in = -1;
        if IMPULSE.load(Ordering::Relaxed) {
            let p = IN_BUFS[0][idx].load(Ordering::Relaxed);
            if !p.is_null() {
                for f in 0..frames {
                    // SAFETY: the driver's half-buffer `idx` holds `frames` 4-byte samples.
                    let v = unsafe {
                        if is_f32 {
                            *(p as *const f32).add(f)
                        } else {
                            *(p as *const i32).add(f) as f32 / i32::MAX as f32
                        }
                    };
                    if v.abs() > PULSE_LEVEL / 2.0 {
                        pulse_in = f as i64;
                        break;
                    }
                }
            }
        }

        // Outputs: silence, plus the impulse on output 1 when enabled.
        let mut pulse_out = -1;
        for (ch, bufs) in OUT_BUFS.iter().enumerate() {
            let p = bufs[idx].load(Ordering::Relaxed);
            if p.is_null() {
                continue;
            }
            // SAFETY: as above; both supported sample types are 4 bytes and zero at zero bits.
            unsafe { std::ptr::write_bytes(p as *mut u8, 0, frames * 4) };
            if ch == 0 && IMPULSE.load(Ordering::Relaxed) && n % PULSE_EVERY == PULSE_EVERY / 2 {
                // SAFETY: PULSE_FRAME < frames is checked before start.
                unsafe {
                    if is_f32 {
                        *(p as *mut f32).add(PULSE_FRAME) = PULSE_LEVEL;
                    } else {
                        *(p as *mut i32).add(PULSE_FRAME) = (PULSE_LEVEL * i32::MAX as f32) as i32;
                    }
                }
                pulse_out = PULSE_FRAME as i64;
            }
        }
        if let Some(d) = DRIVER.get() {
            // Not every driver implements it; the result is only informative here.
            let _ = d.0.output_ready();
        }

        let (ti_spos, ti_stime, ti_flags, mode) = match time_info {
            Some((s, t, f)) => (s, t, f, 1),
            None => (-1, -1, -1, 0),
        };
        if let Some(t) = TABLE.get() {
            t.push([
                mode,
                index as i64,
                ti_spos,
                ti_stime,
                ti_flags,
                gsp_pos,
                gsp_time,
                wall,
                // SAFETY: plain Win32 call without arguments.
                i64::from(unsafe { GetCurrentThreadId() }),
                pulse_out,
                pulse_in,
                CYCLE_NO.load(Ordering::Relaxed),
            ]);
        }
    }

    fn env_u64(name: &str, default: u64) -> u64 {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let seconds = env_u64("PROBE_SECONDS", 5);
        let cycles = env_u64("PROBE_CYCLES", 1).max(1);
        IMPULSE.store(env_u64("PROBE_IMPULSE", 0) == 1, Ordering::Relaxed);
        TIME_INFO.store(env_u64("PROBE_PLAIN", 0) != 1, Ordering::Relaxed);
        TABLE.get_or_init(|| Table {
            next: AtomicUsize::new(0),
            cells: (0..MAX_RECORDS * FIELDS)
                .map(|_| AtomicI64::new(0))
                .collect(),
        });

        let metas = driver::Metadata::enumerate()?;
        println!("ASIO drivers in registry: {}", metas.len());
        for m in &metas {
            println!("  {:?} clsid {:?}", m.description, m.clsid);
        }
        let meta = metas
            .iter()
            .find(|m| {
                m.description
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("wineasio")
            })
            .ok_or("no WineASIO driver registered")?;
        let drv = driver::SafeHandle::new(&meta.clsid)?;
        if !drv.init(None) {
            return Err(format!("init failed: {:?}", drv.last_error()).into());
        }
        println!("name {:?} version {:?}", drv.name(), drv.version());
        let counts = drv.channel_counts()?;
        let bs = drv.buffer_size()?;
        let rate = drv.get_sample_rate()?;
        println!(
            "channels in {} out {}; buffer size {bs:?}; rate {rate}",
            counts.in_, counts.out
        );
        println!("latencies (before create_buffers) {:?}", drv.latencies()?);
        match drv.clock_sources() {
            Ok(c) => println!("clock sources {c:?}"),
            Err(e) => println!("clock sources: error {e}"),
        }
        for input in [true, false] {
            let n = if input { counts.in_ } else { counts.out };
            for index in 0..n {
                let info = drv.channel_info(ChannelId { input, index })?;
                println!(
                    "  {} {index}: {:?} active={} group={} type={:?}",
                    if input { "in " } else { "out" },
                    info.name,
                    info.is_active,
                    info.group,
                    info.sample_type
                );
            }
        }

        let frames = bs.preferred;
        let sample_type = drv
            .channel_info(ChannelId {
                input: true,
                index: 0,
            })?
            .sample_type;
        let is_f32 = if sample_type == SampleType::PCM_F32_LSB {
            true
        } else if sample_type == SampleType::PCM_I32_LSB {
            false
        } else {
            return Err(
                format!("probe handles F32/I32 LSB only, driver uses {sample_type:?}").into(),
            );
        };
        if (frames as usize) <= PULSE_FRAME {
            return Err("buffer too short for the impulse offset".into());
        }
        IS_F32.store(is_f32, Ordering::Relaxed);
        FRAMES.store(frames as usize, Ordering::Relaxed);

        let channels = (0..IN_CH as c_long)
            .map(|index| ChannelId { input: true, index })
            .chain((0..OUT_CH as c_long).map(|index| ChannelId {
                input: false,
                index,
            }));
        // SAFETY: CALLBACKS is static; buffers are disposed before the driver is dropped.
        let bufs: Vec<[*mut c_void; 2]> =
            unsafe { drv.create_buffers(channels, frames, &raw const CALLBACKS) }?.collect();
        for (i, b) in bufs.iter().enumerate() {
            let slot = if i < IN_CH {
                &IN_BUFS[i]
            } else {
                &OUT_BUFS[i - IN_CH]
            };
            slot[0].store(b[0], Ordering::Relaxed);
            slot[1].store(b[1], Ordering::Relaxed);
        }
        println!(
            "latencies (after create_buffers, {frames} frames) {:?}",
            drv.latencies()?
        );
        // SAFETY: the clone (AddRef) is dropped never; the process exits after dispose.
        let _ = DRIVER.set(Shared(unsafe { drv.as_unsafe() }.clone()));
        println!(
            "main thread id {}; impulse {}; time info offered {}",
            unsafe { GetCurrentThreadId() },
            IMPULSE.load(Ordering::Relaxed),
            TIME_INFO.load(Ordering::Relaxed)
        );

        T0.get_or_init(Instant::now);
        for c in 0..cycles {
            CYCLE_NO.store(c as i64, Ordering::Relaxed);
            let before = SWITCHES.load(Ordering::Relaxed);
            drv.start()?;
            std::thread::sleep(Duration::from_secs(seconds));
            drv.stop()?;
            println!(
                "cycle {c}: {} buffer switches",
                SWITCHES.load(Ordering::Relaxed) - before
            );
        }
        drv.dispose_buffers()?;

        let rows = TABLE.get().map(Table::rows).unwrap_or_default();
        report(&rows, frames as i64, rate);
        let names = [
            (1, "selectorSupported"),
            (2, "engineVersion"),
            (3, "resetRequest"),
            (4, "bufferSizeChange"),
            (5, "resyncRequest"),
            (6, "latenciesChanged"),
            (7, "supportsTimeInfo"),
            (8, "supportsTimeCode"),
            (15, "overload"),
            (31, "sampleRateDidChange"),
        ];
        for (i, name) in names {
            let n = MESSAGES[i].load(Ordering::Relaxed);
            if n > 0 {
                println!("asioMessage {name}: {n}");
            }
        }
        Ok(())
    }

    fn report(rows: &[[i64; FIELDS]], frames: i64, rate: f64) {
        println!("# buffer switches recorded: {}", rows.len());
        println!(
            "#    n cyc mode idx   ti_spos     ti_stime_ns  flg   gsp_pos     gsp_time_ns     wall_ns   tid pout pin"
        );
        for (i, r) in rows.iter().enumerate() {
            println!(
                "{i:6} {:3} {:4} {:3} {:9} {:15} {:4} {:9} {:15} {:11} {:5} {:4} {:3}",
                r[CYCLE],
                r[MODE],
                r[BUF_IDX],
                r[TI_SPOS],
                r[TI_STIME],
                r[TI_FLAGS],
                r[GSP_POS],
                r[GSP_TIME],
                r[WALL_NS],
                r[TID],
                r[PULSE_OUT],
                r[PULSE_IN]
            );
        }

        println!("# summary (buffer {frames} frames, rate {rate})");
        let mut tids: Vec<i64> = rows.iter().map(|r| r[TID]).collect();
        tids.sort_unstable();
        tids.dedup();
        println!("callback thread ids: {tids:?}");
        let cycles = rows.iter().map(|r| r[CYCLE]).max().map_or(0, |c| c + 1);
        for c in 0..cycles {
            let rs: Vec<_> = rows.iter().filter(|r| r[CYCLE] == c).collect();
            let pos = |r: &[i64; FIELDS]| if r[MODE] == 1 { r[TI_SPOS] } else { r[GSP_POS] };
            let (mut step_ok, mut step_bad, mut idx_alt, mut ti_eq_gsp) = (0, 0, 0, 0);
            let mut bad_examples = Vec::new();
            for w in rs.windows(2) {
                let d = pos(w[1]) - pos(w[0]);
                if d == frames {
                    step_ok += 1;
                } else {
                    step_bad += 1;
                    if bad_examples.len() < 5 {
                        bad_examples.push(d);
                    }
                }
                idx_alt += usize::from(w[0][BUF_IDX] != w[1][BUF_IDX]);
            }
            for r in &rs {
                ti_eq_gsp += usize::from(r[MODE] == 1 && r[TI_SPOS] == r[GSP_POS]);
            }
            let first = rs.first().map(|r| (pos(r), r[TID]));
            println!(
                "cycle {c}: {} switches; first (position, tid) {first:?}; position step == {frames}: \
                 {step_ok} of {} (other steps {bad_examples:?}); buffer index alternates {idx_alt}; \
                 ASIOTime position == getSamplePosition in {ti_eq_gsp}",
                rs.len(),
                step_ok + step_bad
            );
            // System time: resolution and drift against the sample clock.
            let st: Vec<(i64, i64)> = rs
                .iter()
                .map(|r| {
                    if r[MODE] == 1 {
                        (pos(r), r[TI_STIME])
                    } else {
                        (pos(r), r[GSP_TIME])
                    }
                })
                .collect();
            let mut min_nonzero_step = i64::MAX;
            for w in st.windows(2) {
                let d = w[1].1 - w[0].1;
                if d > 0 {
                    min_nonzero_step = min_nonzero_step.min(d);
                }
            }
            if let (Some(a), Some(b)) = (st.get(1), st.last()) {
                let ns_per_sample = (b.1 - a.1) as f64 / (b.0 - a.0).max(1) as f64;
                println!(
                    "cycle {c}: system time smallest nonzero step {} ns; over the run {:.1} ns/sample \
                     (nominal {:.1})",
                    min_nonzero_step,
                    ns_per_sample,
                    1e9 / rate
                );
            }
            // Loopback: input pulse position minus the latest output pulse position.
            let mut last_out: Option<i64> = None;
            let mut delays = Vec::new();
            for r in &rs {
                if r[PULSE_IN] >= 0
                    && let Some(o) = last_out.take()
                {
                    delays.push(pos(r) + r[PULSE_IN] - o);
                }
                if r[PULSE_OUT] >= 0 {
                    last_out = Some(pos(r) + r[PULSE_OUT]);
                }
            }
            if !delays.is_empty() {
                let mut d = delays.clone();
                d.sort_unstable();
                d.dedup();
                println!(
                    "cycle {c}: loopback delay samples: {} pulses, distinct values {d:?}",
                    delays.len()
                );
            }
        }
    }
}

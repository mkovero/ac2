//! `--latency-log FILE`: one CSV row per pane drawn per presented frame, stamping each stage
//! the newest data frame on that pane went through on its way to the screen
//! (`docs/design/latency-study.md` §2, sites A0–A4).
//!
//! Off, every site is one `Option` check. On, rows are fixed-size records gathered in
//! preallocated buffers and handed about once a second to a writer thread, which formats
//! them: the render path never formats or writes. A pass's rows wait for the next pass,
//! which knows when the GPU probe of the pass before ran.
//!
//! Present timing: eframe and wgpu report nothing after `queue.present()`. A probe paint
//! callback on the topmost layer stamps `prepare_ns` (egui-wgpu preparing the callbacks,
//! before it acquires the surface texture) and `submit_ns` (painting, after the acquire, as
//! the last paint of the frame: `queue.submit` and `present` follow with no app code
//! between). A present mode that blocks (FIFO) blocks in the acquire, which is between
//! the two.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ac2_proto::topic::Topic;
use eframe::egui;

use crate::conn::DataSnapshot;
use crate::state::{AppState, PaneId, PaneKind};

/// The CSV's column names, in order.
pub const COLUMNS: &[&str] = &[
    "pass",
    "pass_start_ns",
    "ui_end_ns",
    "frame_cpu_ms",
    "panes_drawn",
    "pane",
    "view",
    "scene",
    "build_ms",
    "scene_ns",
    "topic",
    "incarnation",
    "seq",
    "audio_sample",
    "capture_wall_ns",
    "read_ns",
    "read_wall_ns",
    "snapshot_ns",
    "apply_ns",
    "prepare_ns",
    "submit_ns",
    "submit_wall_ns",
    "glide_k",
    "drawn_capture_wall_ns",
    "glide_lag_ms",
    "row_lag_slots",
    "newest_slot",
    "slot_advance",
];

/// Rows gathered before they go to the writer; also the buffers' preallocated size.
const BATCH: usize = 512;
/// The most rows held while the writer is behind; past it rows are counted, not kept.
const HELD_MAX: usize = 8 * BATCH;
/// How often a batch goes to the writer, at the most.
const SEND_EVERY: Duration = Duration::from_secs(1);

/// What a pane showed: its kind, and for a spectrum pane which picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Transfer,
    Spectrum,
    Spectrograph,
    Spectrum3d,
    Spl,
    Distortion,
    Programme,
    Walk,
}

impl View {
    const ALL: [View; 8] = [
        View::Transfer,
        View::Spectrum,
        View::Spectrograph,
        View::Spectrum3d,
        View::Spl,
        View::Distortion,
        View::Programme,
        View::Walk,
    ];

    pub fn name(self) -> &'static str {
        match self {
            View::Transfer => "transfer",
            View::Spectrum => "spectrum",
            View::Spectrograph => "spectrograph",
            View::Spectrum3d => "spectrum_3d",
            View::Spl => "spl",
            View::Distortion => "distortion",
            View::Programme => "programme",
            View::Walk => "walk",
        }
    }

    fn bit(self) -> u8 {
        1 << (self as u8)
    }

    /// What pane `id` draws now, by the same tests the pane's scene is chosen by.
    pub fn of(st: &AppState, id: PaneId) -> Option<View> {
        let view = st.layout.view(id)?;
        Some(match view.kind {
            PaneKind::Transfer => View::Transfer,
            PaneKind::Spectrum
                if view.shows_3d(st.three_d())
                    && st.prefs.quality_3d.tier(st.auto_tier).is_some() =>
            {
                View::Spectrum3d
            }
            PaneKind::Spectrum if view.modes.spectrum.spectrograph() => View::Spectrograph,
            PaneKind::Spectrum => View::Spectrum,
            PaneKind::Spl => View::Spl,
            PaneKind::Distortion => View::Distortion,
            PaneKind::Programme => View::Programme,
            PaneKind::Walk => View::Walk,
        })
    }
}

/// The newest data frame a spectrum pane showed and how far its picture had got to it.
#[derive(Clone, Copy, Debug)]
struct Data {
    topic: Topic,
    incarnation: u64,
    seq: u64,
    audio_sample: u64,
    capture_wall_ns: u64,
    read_ns: i64,
    snapshot_ns: Option<i64>,
    apply_ns: Option<i64>,
    glide_k: Option<f64>,
    /// How far the capture time the picture stands for trails `capture_wall_ns`, ns.
    glide_lag_ns: f64,
    row_lag_slots: Option<f64>,
    newest_slot: Option<u64>,
    slot_advance: Option<i64>,
}

/// One pane drawn in one pass (or a pass that drew no pane: `pane` `None`). Times are ns
/// on the monotonic clock since the log started; `pass_wall_ns` is the wall clock at
/// `pass_start_ns`, which maps the others to wall time.
#[derive(Clone, Copy, Debug, Default)]
struct Row {
    pass: u64,
    pass_start_ns: i64,
    pass_wall_ns: i64,
    ui_end_ns: Option<i64>,
    frame_cpu_ms: Option<f32>,
    panes_drawn: u8,
    pane: Option<u32>,
    view: Option<View>,
    built: bool,
    build_ms: f64,
    scene_ns: i64,
    data: Option<Data>,
    prepare_ns: Option<i64>,
    submit_ns: Option<i64>,
}

#[derive(Clone, Copy, Debug)]
struct Applied {
    seq: u64,
    drained: Instant,
    at: Instant,
}

/// What the probe paint callback saw of the GPU side of the last pass it ran in.
#[derive(Debug, Default)]
struct Probe {
    pass: AtomicU64,
    prepare_ns: AtomicI64,
    submit_ns: AtomicI64,
}

/// The `--latency-log` switch: off ([`Latency::off`]) or writing to a file.
#[derive(Debug, Default)]
pub struct Latency(Option<Box<Log>>);

#[derive(Debug)]
struct Log {
    epoch: Instant,
    probe: Arc<Probe>,
    pass: Option<u64>,
    pass_start_ns: i64,
    pass_wall_ns: i64,
    ui_end_ns: Option<i64>,
    views: u8,
    /// This pass's rows, waiting for its probe stamps.
    pending: Vec<Row>,
    batch: Vec<Row>,
    sent: Instant,
    /// Each topic's newest frame by rig slot and topic: its seq, when the link's snapshot
    /// carrying it first was drained and when the reducer applied that snapshot.
    applied: HashMap<(u32, Topic), Applied>,
    /// Each pane's newest spectrograph slot as last drawn.
    slots: HashMap<u32, u64>,
    dropped: u64,
    tx: Option<SyncSender<Vec<Row>>>,
    spare: Receiver<Vec<Row>>,
    writer: Option<JoinHandle<()>>,
}

fn wall_now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

impl Latency {
    /// Logging off: every site returns at once.
    pub fn off() -> Self {
        Self(None)
    }

    /// Starts a log at `path` (replaced if it exists), its first lines `notes` as `#`
    /// comments (what the run was on: the adapter, the present mode).
    pub fn create(path: &Path, notes: &[String]) -> io::Result<Self> {
        let mut out = BufWriter::new(File::create(path)?);
        let epoch = Instant::now();
        let wall = wall_now_ns();
        writeln!(out, "# ac2-ui latency log, format 1")?;
        writeln!(
            out,
            "# *_ns columns: monotonic ns since the log started at wall {wall} ns; \
             *_wall_ns: ns since the Unix epoch (capture_wall_ns on the daemon's clock)"
        )?;
        for n in notes {
            writeln!(out, "# {n}")?;
        }
        writeln!(out, "{}", COLUMNS.join(","))?;
        out.flush()?;
        let (tx, rx) = sync_channel::<Vec<Row>>(4);
        let (back, spare) = sync_channel::<Vec<Row>>(4);
        let writer = std::thread::Builder::new()
            .name("latency-log".into())
            .spawn(move || write_rows(out, &rx, &back))?;
        Ok(Self(Some(Box::new(Log {
            epoch,
            probe: Arc::default(),
            pass: None,
            pass_start_ns: 0,
            pass_wall_ns: wall,
            ui_end_ns: None,
            views: 0,
            pending: Vec::with_capacity(32),
            batch: Vec::with_capacity(BATCH),
            sent: epoch,
            applied: HashMap::with_capacity(64),
            slots: HashMap::with_capacity(16),
            dropped: 0,
            tx: Some(tx),
            spare,
            writer: Some(writer),
        }))))
    }

    pub fn is_on(&self) -> bool {
        self.0.is_some()
    }

    /// A pass starts: the pass before's rows take its probe stamps and eframe's CPU time
    /// for it (`frame_cpu_s`, without the time spent waiting on the surface).
    pub(crate) fn pass_start(&mut self, pass: u64, frame_cpu_s: Option<f32>) {
        let Some(l) = self.0.as_deref_mut() else {
            return;
        };
        l.finish_pass(frame_cpu_s);
        let now = Instant::now();
        l.pass = Some(pass);
        l.pass_start_ns = l.ns(now);
        l.pass_wall_ns = wall_now_ns();
        l.ui_end_ns = None;
        l.views = 0;
        if now.duration_since(l.sent) >= SEND_EVERY {
            l.send();
        }
    }

    /// The reducer is about to apply rig `slot`'s snapshot `d`: each topic whose newest
    /// frame is new is stamped applied now.
    pub(crate) fn applied(&mut self, slot: u32, d: &DataSnapshot) {
        let Some(l) = self.0.as_deref_mut() else {
            return;
        };
        let now = Instant::now();
        for tf in d.latest.frames.values() {
            let a = Applied {
                seq: tf.frame.stamp.seq,
                drained: d.drained,
                at: now,
            };
            let e = l.applied.entry((slot, tf.topic)).or_insert(a);
            if e.seq != a.seq {
                *e = a;
            }
        }
    }

    /// Pane `pane` drew `view`, its scene built (or taken from the cache: `built` false)
    /// between `t0` and `t1`.
    pub(crate) fn pane(
        &mut self,
        st: &AppState,
        pane: PaneId,
        view: Option<View>,
        built: bool,
        t0: Instant,
        t1: Instant,
    ) {
        let Some(l) = self.0.as_deref_mut() else {
            return;
        };
        if let Some(v) = view {
            l.views |= v.bit();
        }
        // A scene is taken from the cache only while it is the picture the newest frames
        // make (a frame with the same levels keeps it), so the newest frame is the one shown.
        let data = match view {
            Some(v @ (View::Spectrum | View::Spectrograph | View::Spectrum3d)) => {
                l.data(st, pane, v, t1)
            }
            _ => None,
        };
        let scene_ns = l.ns(t1);
        let row = Row {
            pass: l.pass.unwrap_or(0),
            pass_start_ns: l.pass_start_ns,
            pass_wall_ns: l.pass_wall_ns,
            pane: Some(pane.0),
            view,
            built,
            build_ms: t1.saturating_duration_since(t0).as_secs_f64() * 1e3,
            scene_ns,
            data,
            ..Row::default()
        };
        if l.pending.len() < HELD_MAX {
            l.pending.push(row);
        }
    }

    /// The pass's widgets are laid out: stamps its end and puts the GPU probe on top of
    /// everything drawn (only with a wgpu renderer: `gpu`).
    pub(crate) fn pass_end(&mut self, ctx: &egui::Context, gpu: bool) {
        let Some(l) = self.0.as_deref_mut() else {
            return;
        };
        l.ui_end_ns = Some(l.ns(Instant::now()));
        if gpu {
            let at = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1.0, 1.0));
            let layer = egui::LayerId::new(egui::Order::Debug, egui::Id::new("latency-probe"));
            ctx.layer_painter(layer)
                .add(egui_wgpu::Callback::new_paint_callback(
                    at,
                    ProbeCallback {
                        pass: l.pass.unwrap_or(0),
                        epoch: l.epoch,
                        probe: Arc::clone(&l.probe),
                    },
                ));
        }
    }

    /// Writes what is held and waits for the writer to finish the file.
    pub fn close(&mut self) {
        if let Some(mut l) = self.0.take() {
            l.finish_pass(None);
            l.send();
            l.tx = None;
            if let Some(w) = l.writer.take() {
                let _ = w.join();
            }
        }
    }
}

impl Drop for Latency {
    fn drop(&mut self) {
        self.close();
    }
}

impl Log {
    fn ns(&self, t: Instant) -> i64 {
        match t.checked_duration_since(self.epoch) {
            Some(d) => d.as_nanos() as i64,
            None => -(self.epoch.duration_since(t).as_nanos() as i64),
        }
    }

    /// The pass that ran last: its rows (or one row saying it drew no pane) to the batch.
    fn finish_pass(&mut self, frame_cpu_s: Option<f32>) {
        let Some(pass) = self.pass else {
            return;
        };
        if self.pending.is_empty() {
            self.pending.push(Row {
                pass,
                pass_start_ns: self.pass_start_ns,
                pass_wall_ns: self.pass_wall_ns,
                ..Row::default()
            });
        }
        let probed = self.probe.pass.load(Ordering::Acquire) == pass;
        let (prepare, submit) = if probed {
            (
                Some(self.probe.prepare_ns.load(Ordering::Relaxed)),
                Some(self.probe.submit_ns.load(Ordering::Relaxed)),
            )
        } else {
            (None, None)
        };
        for mut r in self.pending.drain(..) {
            r.ui_end_ns = self.ui_end_ns;
            r.frame_cpu_ms = frame_cpu_s.map(|s| s * 1e3);
            r.panes_drawn = self.views;
            r.prepare_ns = prepare;
            r.submit_ns = submit;
            if self.batch.len() < HELD_MAX {
                self.batch.push(r);
            } else {
                self.dropped += 1;
            }
        }
        if self.batch.len() >= BATCH {
            self.send();
        }
    }

    /// Hands the batch to the writer; one comes back to fill next.
    fn send(&mut self) {
        self.sent = Instant::now();
        let Some(tx) = &self.tx else {
            return;
        };
        if self.batch.is_empty() {
            return;
        }
        let next = self
            .spare
            .try_recv()
            .unwrap_or_else(|_| Vec::with_capacity(BATCH));
        let full = std::mem::replace(&mut self.batch, next);
        match tx.try_send(full) {
            Ok(()) => {}
            // The writer is behind: keep the rows and try again later.
            Err(TrySendError::Full(back)) => {
                let fresh = std::mem::replace(&mut self.batch, back);
                drop(fresh);
            }
            Err(TrySendError::Disconnected(_)) => {
                tracing::warn!("latency log: the writer stopped; logging off");
                self.tx = None;
            }
        }
        if self.dropped > 0 {
            tracing::warn!("latency log: {} rows dropped (writer behind)", self.dropped);
            self.dropped = 0;
        }
    }

    /// The newest frame spectrum pane `pane` shows in `view` at `now`, and how far behind
    /// it the picture is.
    fn data(&mut self, st: &AppState, pane: PaneId, view: View, now: Instant) -> Option<Data> {
        let m = st.pane_meas(pane)?;
        let stream = crate::state::spectrum_stream(m)?;
        let tf = crate::scenes::frame(st, m.id, stream)?;
        let stamp = &tf.frame.stamp;
        let capture = stamp.capture_wall_ns.0;
        let applied = self
            .applied
            .get(&(st.rig.slot, tf.topic))
            .filter(|a| a.seq == stamp.seq)
            .copied();
        let mut d = Data {
            topic: tf.topic,
            incarnation: stamp.daemon_incarnation.0,
            seq: stamp.seq,
            audio_sample: stamp.audio_sample.0,
            capture_wall_ns: capture,
            read_ns: self.ns(tf.received),
            // The snapshot held now is aged every pass (its `drained` moves on): the link's
            // own drain is the one noted when it was applied.
            snapshot_ns: applied.map(|a| self.ns(a.drained)),
            apply_ns: applied.map(|a| self.ns(a.at)),
            glide_k: None,
            glide_lag_ns: 0.0,
            row_lag_slots: None,
            newest_slot: None,
            slot_advance: None,
        };
        match view {
            View::Spectrum if st.prefs.spec_ease => {
                if let Some(g) = st.rig.folded.spec_glide.get(&tf.topic) {
                    d.glide_k = g.progress(stamp, now);
                    d.glide_lag_ns = capture as f64 - g.drawn_capture_ns(stamp, now);
                }
            }
            View::Spectrograph | View::Spectrum3d => {
                let h = st.rig.folded.spectrographs.get(&m.id)?;
                let newest = h.newest_slot();
                d.newest_slot = newest;
                if let Some(n) = newest {
                    let last = self.slots.insert(pane.0, n);
                    d.slot_advance = last.map(|l| n as i64 - l as i64);
                }
                // Where the time axis stands against the newest slot's end: short of it
                // (positive) or past it while the next frame is due (negative). The glide
                // lag is how far the axis trails the newest frame's audio, 0 once past it
                // (the newest content is drawn either way).
                let lead = st.scroll_lead(pane, now);
                d.row_lag_slots = Some(-lead);
                if st.prefs.spec_ease
                    && let (Some(c), Some(n)) = (st.rig.folded.scroll.get(&m.id), newest)
                    && let Some(newest_s) = c.newest_s()
                {
                    let at_s = ((n + 1) as f64 + lead) * h.slot_s();
                    d.glide_lag_ns = ((newest_s - at_s) * 1e9).max(0.0);
                }
            }
            _ => {}
        }
        Some(d)
    }
}

/// Stamps egui-wgpu's prepare and paint of the pass it was added in.
struct ProbeCallback {
    pass: u64,
    epoch: Instant,
    probe: Arc<Probe>,
}

impl ProbeCallback {
    fn now_ns(&self) -> i64 {
        Instant::now()
            .saturating_duration_since(self.epoch)
            .as_nanos() as i64
    }
}

impl egui_wgpu::CallbackTrait for ProbeCallback {
    fn prepare(
        &self,
        _device: &ac2_plot::wgpu::Device,
        _queue: &ac2_plot::wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut ac2_plot::wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<ac2_plot::wgpu::CommandBuffer> {
        self.probe
            .prepare_ns
            .store(self.now_ns(), Ordering::Relaxed);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        _pass: &mut ac2_plot::wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        self.probe.submit_ns.store(self.now_ns(), Ordering::Relaxed);
        self.probe.pass.store(self.pass, Ordering::Release);
    }
}

/// The writer thread: formats each batch, writes it and sends the emptied buffer back.
fn write_rows(mut out: BufWriter<File>, rx: &Receiver<Vec<Row>>, back: &SyncSender<Vec<Row>>) {
    let mut failed = false;
    let mut line = String::with_capacity(512);
    while let Ok(mut rows) = rx.recv() {
        for r in &rows {
            line.clear();
            format_row(&mut line, r);
            if !failed && let Err(e) = out.write_all(line.as_bytes()) {
                tracing::warn!("latency log: {e}");
                failed = true;
            }
        }
        if !failed && let Err(e) = out.flush() {
            tracing::warn!("latency log: {e}");
            failed = true;
        }
        rows.clear();
        let _ = back.try_send(rows);
    }
    let _ = out.flush();
}

fn opt<T: std::fmt::Display>(s: &mut String, v: Option<T>) {
    use std::fmt::Write as _;
    if let Some(v) = v {
        let _ = write!(s, "{v}");
    }
    s.push(',');
}

/// One CSV line, columns as [`COLUMNS`].
fn format_row(s: &mut String, r: &Row) {
    use std::fmt::Write as _;
    let wall = |ns: i64| r.pass_wall_ns + (ns - r.pass_start_ns);
    let _ = write!(s, "{},{},", r.pass, r.pass_start_ns);
    opt(s, r.ui_end_ns);
    opt(s, r.frame_cpu_ms.map(|v| format!("{v:.3}")));
    let mut first = true;
    for v in View::ALL {
        if r.panes_drawn & v.bit() != 0 {
            if !first {
                s.push('+');
            }
            s.push_str(v.name());
            first = false;
        }
    }
    s.push(',');
    opt(s, r.pane);
    opt(s, r.view.map(View::name));
    if r.pane.is_some() {
        let _ = write!(
            s,
            "{},{:.3},{},",
            if r.built { "built" } else { "cached" },
            r.build_ms,
            r.scene_ns
        );
    } else {
        s.push_str(",,,");
    }
    match &r.data {
        Some(d) => {
            let _ = write!(
                s,
                "{},{},{},{},{},{},{},",
                d.topic,
                d.incarnation,
                d.seq,
                d.audio_sample,
                d.capture_wall_ns,
                d.read_ns,
                wall(d.read_ns),
            );
            opt(s, d.snapshot_ns);
            opt(s, d.apply_ns);
        }
        None => s.push_str(",,,,,,,,,"),
    }
    opt(s, r.prepare_ns);
    opt(s, r.submit_ns);
    opt(s, r.submit_ns.map(wall));
    match &r.data {
        Some(d) => {
            opt(s, d.glide_k.map(|k| format!("{k:.4}")));
            let lag = d.glide_lag_ns.round() as i64;
            let _ = write!(
                s,
                "{},{:.3},",
                d.capture_wall_ns as i64 - lag,
                d.glide_lag_ns / 1e6
            );
            opt(s, d.row_lag_slots.map(|v| format!("{v:.4}")));
            opt(s, d.newest_slot);
            if let Some(a) = d.slot_advance {
                let _ = write!(s, "{a}");
            }
        }
        None => s.push_str(",,,,,"),
    }
    s.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Off, no site keeps anything, starts a thread or touches a file.
    #[test]
    fn off_does_nothing() {
        let mut l = Latency::off();
        let st = AppState::new(ac2_scene::theme::ThemeName::Dark, "test".into());
        let ctx = egui::Context::default();
        let t = Instant::now();
        for pass in 0..100 {
            l.pass_start(pass, Some(0.001));
            l.pane(&st, PaneId(0), Some(View::Spectrum), true, t, t);
            l.pass_end(&ctx, true);
        }
        assert!(!l.is_on());
        assert!(l.0.is_none());
        l.close();
        assert!(l.0.is_none());
    }

    /// Every line has a field per column, with or without data, probe or pane.
    #[test]
    fn rows_have_every_column() {
        let mut s = String::new();
        format_row(&mut s, &Row::default());
        assert_eq!(s.trim_end().split(',').count(), COLUMNS.len(), "{s}");
        s.clear();
        let r = Row {
            pane: Some(1),
            view: Some(View::Spectrum3d),
            panes_drawn: View::Spectrum3d.bit() | View::Spl.bit(),
            prepare_ns: Some(5),
            submit_ns: Some(6),
            data: Some(Data {
                topic: Topic::Data {
                    meas: ac2_proto::units::MeasId(1),
                    stream: ac2_proto::topic::Stream::Spec,
                },
                incarnation: 1,
                seq: 2,
                audio_sample: 3,
                capture_wall_ns: 4,
                read_ns: 1,
                snapshot_ns: Some(2),
                apply_ns: Some(3),
                glide_k: Some(0.5),
                glide_lag_ns: 1.0,
                row_lag_slots: Some(0.25),
                newest_slot: Some(9),
                slot_advance: Some(1),
            }),
            ..Row::default()
        };
        format_row(&mut s, &r);
        let f: Vec<&str> = s.trim_end().split(',').collect();
        assert_eq!(f.len(), COLUMNS.len(), "{s}");
        let col = |n: &str| f[COLUMNS.iter().position(|c| *c == n).unwrap_or(0)];
        assert_eq!(col("panes_drawn"), "spectrum_3d+spl");
        assert_eq!(col("view"), "spectrum_3d");
        assert_eq!(col("slot_advance"), "1");
        assert_eq!(col("glide_lag_ms"), "0.000");
    }
}

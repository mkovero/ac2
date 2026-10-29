//! Every ctrl command and its wire name, mutation flag and lease token.

mod meta;

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::grid::GridId;
use crate::model::{
    ArtnetTarget, AverageMethod, BackendKind, Ballistic, CalRef, ColourFamily, Compared,
    CorrelationWindow, DelayPick, DelayReference, DeviceId, DmxSet, ElectricalConnection,
    ExportFormat, FinderBand, GeneratorDesired, ImportFormat, ImportRole, InputSetup, IrImport,
    MeasConfig, MicCurveId, MicLoading, OscMessage, OutputSetup, OwnedTraces, Pair, PaneLayout,
    PlanSpec, PreviewInput, RecordRequest, RecordingRef, ReplayPace, SessionConfig, SessionRef,
    SplLogWhich, TargetCurve, TraceEdit, TraceOwner, WalkKind, WalkPointSpec,
};
use crate::units::{
    Blob, Db, DbSpl, Dbfs, Dbtp, Hz, LeaseToken, Lufs, MeasId, MvPerPa, Rev, Seconds, TraceId,
    TransferId, Volts, WalkId,
};

/// Every command. Wire: `{"op": "<group>.<name>", "args": {…}}`; `args` is absent for
/// commands without arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", content = "args", deny_unknown_fields)]
pub enum Command {
    /// Version handshake; the first request of every connection.
    #[serde(rename = "hello")]
    Hello {
        /// Client software name and version, for logs.
        client: String,
    },

    // -- session ------------------------------------------------------------------------
    /// List the daemon's backends and their devices.
    #[serde(rename = "session.devices")]
    SessionDevices,
    /// Meter every input of a device without opening a session on it: capture only, the
    /// outputs are never opened. Naming the same device again keeps the preview open;
    /// another device replaces it. Closed by `session.preview_stop`, `session.open`,
    /// `session.detect_loopback` or when not renewed within `expires_in_ms`. The declared
    /// kinds and pairs of the device's inputs travel back in the preview frames' meta so the
    /// rows can be labelled; the meters themselves read dBFS.
    #[serde(rename = "session.preview")]
    SessionPreview {
        /// Backend.
        backend: BackendKind,
        /// Device.
        device: DeviceId,
        /// What is on the inputs (undeclared inputs are microphones).
        inputs: Vec<PreviewInput>,
        /// Pairs among them; the rules of `session.inputs` apply.
        pairs: Vec<Pair>,
    },
    /// Close the preview.
    #[serde(rename = "session.preview_stop")]
    SessionPreviewStop,
    /// Open a device's driver control panel on the daemon's machine (ASIO: where pro
    /// interfaces set their rate, buffer size, clock and routing). Answered once the driver
    /// has shown it, or after a short wait while a modal panel stays open. A change there
    /// may reset the open session's stream: it reopens at the driver's new settings
    /// (`StopCause::DriverReset`, `OpenSession::driver_change`). Refused (`unsupported`)
    /// by backends without one.
    #[serde(rename = "session.control_panel")]
    SessionControlPanel {
        /// Backend.
        backend: BackendKind,
        /// Device.
        device: DeviceId,
    },
    /// Play a short band-limited noise burst at `level` on `output` of `output_device` and
    /// find the input of `input_device` it returns on (the same device, or two of one
    /// backend). Needs the stimulus lease and an explicit level; refused while the stimulus
    /// is firing.
    #[serde(rename = "session.detect_loopback")]
    SessionDetectLoopback {
        /// Lease.
        lease_token: LeaseToken,
        /// Backend.
        backend: BackendKind,
        /// Device listened on.
        input_device: DeviceId,
        /// Device the burst plays on.
        output_device: DeviceId,
        /// Output to play the burst on (zero-based).
        output: u16,
        /// RMS level of the burst; refused when absent (there is no default level).
        level: Option<Dbfs>,
    },
    /// Open the audio I/O (new epoch).
    #[serde(rename = "session.open")]
    SessionOpen {
        /// Configuration.
        config: SessionConfig,
    },
    /// Close the audio I/O (new epoch).
    #[serde(rename = "session.close")]
    SessionClose,
    /// Current session.
    #[serde(rename = "session.status")]
    SessionStatus,
    /// Set the kind (and a mic's name and active curve) of the listed inputs (others
    /// unchanged) and the pairs among all inputs (the whole list). A chosen curve must be
    /// stored for the row's mic (unless the row is unchanged). Refused by name: a pair of
    /// two kinds, of an input twice, of an input with itself, or of an input the
    /// open session does not capture; a row's `pair` side is set from the pair list.
    #[serde(rename = "session.inputs")]
    SessionInputs {
        /// Rows to upsert, one per channel.
        inputs: Vec<InputSetup>,
        /// Every pair.
        pairs: Vec<Pair>,
    },
    /// Set or clear the labels of the listed outputs (others unchanged). Kept by the daemon
    /// with the rig's settings.
    #[serde(rename = "session.outputs")]
    SessionOutputs {
        /// Rows to upsert, one per channel; a `None` label clears it.
        outputs: Vec<OutputSetup>,
    },

    // -- gen (Q6 lease) -----------------------------------------------------------------
    /// Acquire the stimulus lease.
    #[serde(rename = "gen.acquire")]
    GenAcquire {
        /// Take over from another holder (stops output and disarms first).
        force: bool,
    },
    /// Set the full desired generator state; refreshes the lease.
    #[serde(rename = "gen.set")]
    GenSet {
        /// Lease.
        lease_token: LeaseToken,
        /// Desired state.
        desired: GeneratorDesired,
    },
    /// Refresh the lease.
    #[serde(rename = "gen.refresh")]
    GenRefresh {
        /// Lease.
        lease_token: LeaseToken,
    },
    /// Stop, disarm and release.
    #[serde(rename = "gen.release")]
    GenRelease {
        /// Lease.
        lease_token: LeaseToken,
    },
    /// Universal stop: fade out and disarm; no lease needed.
    #[serde(rename = "gen.stop")]
    GenStop,
    /// Set the system maximum level (`generator.ceiling`). Any client may lower it: a
    /// stimulus armed or playing above the new maximum is stopped and disarmed. Raising it
    /// needs `confirm_raise` and is refused while anything is armed or playing. Never above
    /// `generator.ceiling_bound`. Audited, and kept by the daemon across restarts.
    #[serde(rename = "gen.ceiling")]
    GenCeiling {
        /// New maximum, dBFS RMS.
        ceiling: Dbfs,
        /// The operator confirmed a raise (ignored for a lowering).
        confirm_raise: bool,
    },

    // -- rig / osc / artnet -------------------------------------------------------------
    /// Set where OSC cues go (`rig.osc_target`), or stop sending (`None`). Kept by the
    /// daemon across restarts.
    #[serde(rename = "osc.target")]
    OscTarget {
        /// The `rig.rev` the edit was made on; another is refused with `conflict`.
        base: Rev,
        /// An IP literal and port (no name lookup); `None`: send nowhere.
        #[serde(with = "crate::model::opt_addr")]
        addr: Option<SocketAddr>,
    },
    /// Send one message to the OSC target now, to check the receiver (setup). Refused
    /// (`invalid`) without a target; `append_level` appends 0 dB SPL.
    #[serde(rename = "osc.test")]
    OscTest {
        /// Message.
        msg: OscMessage,
    },
    /// Set where the rig's Art-Net universe goes (`rig.artnet_target`), or stop sending
    /// (`None`). Kept by the daemon across restarts.
    #[serde(rename = "artnet.target")]
    ArtnetTarget {
        /// The `rig.rev` the edit was made on; another is refused with `conflict`.
        base: Rev,
        /// Address and universe; `None`: send nowhere.
        target: Option<ArtnetTarget>,
    },
    /// Set how long the DMX channels take to move to a new cue frame (`rig.artnet_fade`),
    /// 0–60 s, 0: they jump; out of range or not finite is refused (`invalid`). Kept by the
    /// daemon across restarts.
    #[serde(rename = "artnet.fade")]
    ArtnetFade {
        /// The `rig.rev` the edit was made on; another is refused with `conflict`.
        base: Rev,
        /// Fade time.
        seconds: Seconds,
    },
    /// Send one frame with these levels (every other channel 0) to the Art-Net target now,
    /// to find a fixture's channels (setup). The cue frame comes back once the test has been
    /// held a few seconds, or at once when a cue changes state. Refused (`invalid`) without
    /// a target.
    #[serde(rename = "artnet.test")]
    ArtnetTest {
        /// Levels.
        channels: Vec<DmxSet>,
    },
    /// Whether the daemon reopens the last session's device capture-only when it starts,
    /// so the rig measurements meter with no client connected (`rig.monitor_at_start`).
    #[serde(rename = "rig.monitor_start")]
    RigMonitorStart {
        /// The `rig.rev` the edit was made on; another is refused with `conflict`.
        base: Rev,
        /// On or off.
        on: bool,
    },
    /// Set the rig's programme-meter settings (`rig.levels`): the ballistic the bars read,
    /// the correlator window, the over threshold, the correlation gate and the loudness
    /// target; a field `None` keeps its value. The daemon runs every detector and every
    /// source's loudness all the time, so a change reads a settled value at once. A threshold
    /// outside −20 … +6 dBTP, a gate outside −120 … 0 dBFS or a target outside −36 … −5 LUFS,
    /// or any not finite, is refused (`invalid`). Kept by the daemon across restarts.
    #[serde(rename = "levels.set")]
    LevelsSet {
        /// The `rig.rev` the edit was made on; another is refused with `conflict`.
        base: Rev,
        /// What the bars read. Under `momentary` / `short_term` each input's `ppm` / `max`
        /// are its true peak in dBTP and the bars read the frame's `loudness`.
        ballistic: Option<Ballistic>,
        /// The correlator's window.
        correlation_window: Option<CorrelationWindow>,
        /// The over threshold.
        over_threshold: Option<Dbtp>,
        /// The correlation gate.
        correlation_gate: Option<Dbfs>,
        /// The loudness target.
        loudness_target: Option<Lufs>,
    },
    /// Clear the max hold of every ballistic and the over count of `channels` (device
    /// inputs; empty: every captured input), on the session meters and the preview alike.
    /// No state changes: the next meter frame carries the cleared values.
    #[serde(rename = "levels.reset")]
    LevelsReset {
        /// Device input channels; empty = all.
        channels: Vec<u16>,
    },

    // -- meas ---------------------------------------------------------------------------
    /// Create a measurement.
    #[serde(rename = "meas.create")]
    MeasCreate {
        /// Configuration.
        config: MeasConfig,
    },
    /// Replace a measurement's configuration.
    #[serde(rename = "meas.update")]
    MeasUpdate {
        /// Measurement.
        meas: MeasId,
        /// The `config_rev` the edit was made on; another (someone changed it since) is
        /// refused with `conflict`, and the client re-reads it.
        base: Rev,
        /// New configuration.
        config: MeasConfig,
    },
    /// Choose the colour family a measurement's curves are drawn in (`None`: the one its
    /// id gives). Its job and config rev go on. Refused for a math channel, which takes its
    /// owner's.
    #[serde(rename = "meas.colour")]
    MeasColour {
        /// Measurement.
        meas: MeasId,
        /// Family, or `None` for the automatic one.
        colour: Option<ColourFamily>,
    },
    /// Set the panes a client shows while the measurement is selected, or clear them
    /// (`None`). Its job and config rev go on.
    #[serde(rename = "meas.layout")]
    MeasLayout {
        /// Measurement.
        meas: MeasId,
        /// Layout, or `None` for none of its own.
        layout: Option<PaneLayout>,
    },
    /// Set the curves drawn compared into the measurement's panes, first compared first
    /// (drawn above the later ones). Its job and config rev go on.
    #[serde(rename = "meas.compare")]
    MeasCompare {
        /// Measurement whose panes draw them.
        meas: MeasId,
        /// Live curves, math channels and stored traces of other measurements, at most
        /// [`Compared::MAX`].
        compared: Vec<Compared>,
    },
    /// Set the display offset of the measurement's live curves, wherever they are drawn.
    /// Its job and config rev go on.
    #[serde(rename = "meas.offset")]
    MeasOffset {
        /// Measurement.
        meas: MeasId,
        /// Offset, finite and at most [`crate::model::Measurement::OFFSET_MAX_DB`] either way.
        offset: Db,
    },
    /// Delete a measurement, and say what becomes of the stored traces and math channels
    /// it owns. Refused while a math channel that stays computes from it or from a trace
    /// deleted with it, and while a run of it (a sweep measurement) plays.
    #[serde(rename = "meas.delete")]
    MeasDelete {
        /// Measurement.
        meas: MeasId,
        /// Its traces and math channels: kept (moved to the imported group) or deleted.
        traces: OwnedTraces,
    },
    /// Edit a measurement's EQ what-if sets (transfer and sweep measurements). Its job and
    /// config rev go on; replies with the measurement.
    #[serde(rename = "meas.eq")]
    MeasEq {
        /// Measurement.
        meas: MeasId,
        /// The edit.
        edit: crate::model::EqEdit,
    },
    /// Start the job (refused for a sweep measurement: `sweep.run` plays it).
    #[serde(rename = "meas.start")]
    MeasStart {
        /// Measurement.
        meas: MeasId,
    },
    /// Stop the job.
    #[serde(rename = "meas.stop")]
    MeasStop {
        /// Measurement.
        meas: MeasId,
    },
    /// Reset averages.
    #[serde(rename = "meas.reset")]
    MeasReset {
        /// Measurement.
        meas: MeasId,
    },

    // -- delay --------------------------------------------------------------------------
    /// Run the delay finder.
    #[serde(rename = "delay.find")]
    DelayFind {
        /// Measurement.
        meas: MeasId,
        /// Analysis band.
        band: FinderBand,
        /// Measurement block length; nil = as much audio as there is, up to the band's
        /// default (decision D2: sub 2, 4 or 8 s, default 4 s).
        observation: Option<Seconds>,
    },
    /// Apply a finder result.
    #[serde(rename = "delay.insert")]
    DelayInsert {
        /// Measurement.
        meas: MeasId,
        /// Which result.
        pick: DelayPick,
    },
    /// Set the delay explicitly.
    #[serde(rename = "delay.set")]
    DelaySet {
        /// Measurement.
        meas: MeasId,
        /// Delay.
        delay: Seconds,
    },
    /// Move the applied delay by `by` (either sign, fractions of a sample allowed). The
    /// transfer function keeps its averages where it can, so the curve moves at once.
    #[serde(rename = "delay.nudge")]
    DelayNudge {
        /// Measurement.
        meas: MeasId,
        /// Step.
        by: Seconds,
    },
    /// Enable or disable tracking.
    #[serde(rename = "delay.track")]
    DelayTrack {
        /// Measurement.
        meas: MeasId,
        /// Enabled.
        enabled: bool,
    },

    // -- trace --------------------------------------------------------------------------
    /// Capture the live result into a trace.
    #[serde(rename = "trace.capture")]
    TraceCapture {
        /// Source measurement.
        meas: MeasId,
        /// Name.
        name: String,
        /// Slot 1…9 to put the trace in (taken from any trace holding it).
        slot: Option<u8>,
    },
    /// List trace metadata.
    #[serde(rename = "trace.list")]
    TraceList,
    /// Get one trace's data.
    #[serde(rename = "trace.get")]
    TraceGet {
        /// Trace.
        trace: TraceId,
    },
    /// Replace a trace's editable properties.
    #[serde(rename = "trace.update")]
    TraceUpdate {
        /// Trace.
        trace: TraceId,
        /// New properties.
        edit: TraceEdit,
    },
    /// Delete a trace.
    #[serde(rename = "trace.delete")]
    TraceDelete {
        /// Trace.
        trace: TraceId,
    },
    /// Average traces into a new trace.
    #[serde(rename = "trace.average")]
    TraceAverage {
        /// Inputs.
        traces: Vec<TraceId>,
        /// Method.
        method: AverageMethod,
        /// Phase reference.
        reference: DelayReference,
        /// Name of the result.
        name: String,
    },
    /// Import a file sent by the client.
    #[serde(rename = "trace.import")]
    TraceImport {
        /// Original file name.
        file_name: String,
        /// Format.
        format: ImportFormat,
        /// Measured trace or target curve.
        role: ImportRole,
        /// How to read an impulse-response WAV file; `None` takes [`IrImport::default`].
        /// Refused (`invalid`) with a text file.
        ir: Option<IrImport>,
        /// File content.
        content: Blob,
        /// The measurement it is listed under, or the imported group: a target loaded for
        /// the system being tuned belongs with that system's measurement. Refused for a
        /// math channel or a measurement that does not exist.
        owner: TraceOwner,
    },
    /// Generate a target curve as a new target trace (`trace.import` of a target, without a
    /// file), listed under `owner`. Refused (`invalid`) for parameters
    /// [`TargetCurve::check`] refuses, and for an owner `trace.import` refuses.
    #[serde(rename = "trace.target")]
    TraceTarget {
        /// The curve.
        curve: TargetCurve,
        /// The trace's name.
        name: String,
        /// The measurement it is listed under, or the imported group.
        owner: TraceOwner,
    },
    /// Apply a curve of the mic library to a stored trace, or remove the one applied
    /// (`curve: None`). A display edit: the stored columns stay as measured
    /// ([`TraceMeta::mic_curve`](crate::model::TraceMeta::mic_curve)). Refused for a trace
    /// whose columns already carry a curve (captured with one) and for targets.
    #[serde(rename = "trace.mic_curve")]
    TraceMicCurve {
        /// Trace.
        trace: TraceId,
        /// The curve to apply; `None` removes.
        curve: Option<MicCurveId>,
    },
    /// Export a trace.
    #[serde(rename = "trace.export")]
    TraceExport {
        /// Trace.
        trace: TraceId,
        /// Format.
        format: ExportFormat,
    },

    // -- cal ----------------------------------------------------------------------------
    /// Calibrate an input against an acoustic calibrator.
    #[serde(rename = "cal.spl")]
    CalSpl {
        /// Input channel.
        input: u16,
        /// Mic name.
        mic: String,
        /// Calibrator level.
        calibrator_level: DbSpl,
        /// Calibrator frequency.
        calibrator_freq: Hz,
    },
    /// Calibrate an input electrically: the operator measured `volts` (RMS) at the input
    /// while a steady tone of `freq` is on it; the daemon reads the input's level and
    /// stores `20·lg(V_FS / (S · 20 µPa))` with `V_FS = volts / 10^(level/20)` and the mic
    /// sensitivity `S` (`None`: the data-sheet value of `mic`'s curve files, when they
    /// state exactly one), loaded by `loading` when given: `S · Z_in / (Z_in + Z_out)`.
    #[serde(rename = "cal.spl_electrical")]
    CalSplElectrical {
        /// Input channel.
        input: u16,
        /// Mic name.
        mic: String,
        /// Where the voltage was measured.
        connection: ElectricalConnection,
        /// Voltage measured, RMS.
        volts: Volts,
        /// Frequency of the tone.
        freq: Hz,
        /// Mic sensitivity; `None` = the data sheet's.
        mic_sensitivity: Option<MvPerPa>,
        /// Mic output and preamp input impedance: the sensitivity is divided down by them
        /// (`cal::loaded_sensitivity`). `None` = unloaded.
        loading: Option<MicLoading>,
        /// Stated uncertainty, ± dB; `None` = 1 dB.
        uncertainty: Option<Db>,
        /// Replace an acoustic calibration of this input and mic (refused otherwise: a
        /// calibrator reading is the better one).
        replace_acoustic: bool,
    },
    /// Import a mic curve file into the mic library as `mic`'s curve `label` (default:
    /// the angle the file names, `90°`, else its file stem); a curve with that label is
    /// replaced. With `input`, that input's mic name is set to `mic`, and the curve becomes
    /// its active one when it is the mic's only curve.
    #[serde(rename = "cal.curve_import")]
    CalCurveImport {
        /// Mic name.
        mic: String,
        /// Label; `None` = from the file.
        label: Option<String>,
        /// Original file name.
        file_name: String,
        /// File content (`.frd`, `.txt`, CSV); the daemon parses and validates it.
        content: Blob,
        /// Input whose mic name is set.
        input: Option<u16>,
    },
    /// Rename a curve; inputs that chose it follow.
    #[serde(rename = "cal.curve_rename")]
    CalCurveRename {
        /// The curve.
        curve: MicCurveId,
        /// New label.
        label: String,
    },
    /// Delete a curve. Inputs that chose it keep the label and say it is not stored.
    #[serde(rename = "cal.curve_delete")]
    CalCurveDelete {
        /// The curve.
        curve: MicCurveId,
    },
    /// Calibrate a line input: the operator feeds the input a steady tone and measures
    /// `volts` (RMS) at the input; the daemon measures the tone's level `L` the way
    /// `cal.spl_electrical` does (refused without signal, clipped or not steady yet) and
    /// stores `V_FS = volts / 10^(L/20)`, the volts at 0 dBFS, under the open session's
    /// input device. On a `line` input only: a mic reads dB SPL and a digital feed has no
    /// volt.
    #[serde(rename = "cal.line")]
    CalLine {
        /// Input channel.
        channel: u16,
        /// Voltage measured, RMS.
        volts: Volts,
    },
    /// List sensitivity and line calibrations and the mic library.
    #[serde(rename = "cal.list")]
    CalList,
    /// Delete a sensitivity or line calibration, on any device (no open session needed).
    #[serde(rename = "cal.delete")]
    CalDelete {
        /// Entry.
        key: CalRef,
    },

    // -- spl ----------------------------------------------------------------------------
    /// Rows of an SPL meter's per-second log, from row number `from` (rows already dropped
    /// are skipped: the reply says where it starts), at most `max` (capped at
    /// [`crate::model::SplLogPage::MAX_ROWS`]).
    #[serde(rename = "spl.log_get")]
    SplLogGet {
        /// SPL measurement.
        meas: MeasId,
        /// Which log: the current one, or the one `spl.log_new` ended last.
        log: SplLogWhich,
        /// First row wanted.
        from: u64,
        /// Most rows wanted.
        max: u32,
    },
    /// Each Leq window of an SPL meter over a span of its current log, as the meter's job
    /// computed them and its `leq` frames carried them: the newest seconds one by one, or
    /// any span of the whole log at a resolution ([`crate::model::HistorySpan`]).
    #[serde(rename = "spl.history_get")]
    SplHistoryGet {
        /// SPL measurement.
        meas: MeasId,
        /// Which seconds, at which resolution.
        span: crate::model::HistorySpan,
    },
    /// Ends an SPL meter's log and starts a new one: the windows, their states, the alarms,
    /// the run clock and the total start over; the windows and limits are kept. The ended
    /// log stays readable (`spl.log_get` with `log: previous`) until the next `spl.log_new`
    /// of the meter, the meter's deletion or a daemon restart.
    #[serde(rename = "spl.log_new")]
    SplLogNew {
        /// SPL measurement.
        meas: MeasId,
    },
    /// Computes a transfer per band from the meter's mic to the place the band limits are
    /// for (`docs/design/band-leq.md`, *The transfer*) from the band levels of a steady test
    /// signal at FOH and at the place (`at_place`), and the place's background with the
    /// system silent, and stores it in the band
    /// meter of `meas` (which must have one); the reply is the updated measurement. The
    /// levels come from band logs over a span or as typed values.
    #[serde(rename = "spl.band_transfer")]
    SplBandTransfer {
        /// SPL meter whose band meter gets the transfer.
        meas: MeasId,
        /// Band levels at FOH (the meter's own mic position).
        foh: crate::model::BandLevelSource,
        /// Band levels at the place the limits are for, the same signal.
        at_place: crate::model::BandLevelSource,
        /// The place's background, the system silent; `None`: every band unchecked.
        background: Option<crate::model::BandLevelSource>,
        /// The operator's name of the place ([`crate::model::BandTransferSet::place`]).
        place: String,
    },

    /// A span of an SPL meter's band log (its current log): the energy average over
    /// `[from, until)` as `spl.band_transfer` takes it from a log source, and with `step`
    /// every `step`-th logged second (at most [`crate::model::SplBandLog::MAX_ROWS`] rows,
    /// else refused).
    #[serde(rename = "spl.band_log_get")]
    SplBandLogGet {
        /// SPL meter.
        meas: MeasId,
        /// Start (inclusive).
        from: crate::units::WallNs,
        /// End (exclusive).
        until: crate::units::WallNs,
        /// Rows wanted: every `step`-th second (1: each); `None`: the average alone.
        step: Option<u32>,
    },

    // -- sweep --------------------------------------------------------------------------
    /// Run a sweep measurement with its settings: plays its `repeats` synchronised sweeps,
    /// records the reference and measurement inputs and stores a `sweep` trace it owns
    /// (response, harmonic distortion, IR, room parameters). Like firing, it needs the
    /// stimulus lease and the generator armed; the reply is the started run, whose progress
    /// and outcome follow as `sweep` events.
    #[serde(rename = "sweep.run")]
    SweepRun {
        /// Lease.
        lease_token: LeaseToken,
        /// The sweep measurement.
        meas: MeasId,
        /// Name of the resulting trace; `None` = `Run <number>`.
        name: Option<String>,
    },

    // -- state, grid, file --------------------------------------------------------------
    /// Full state snapshot.
    #[serde(rename = "state.snapshot")]
    StateSnapshot,
    /// Events after `rev`, or `resync_required`.
    #[serde(rename = "state.since")]
    StateSince {
        /// Last applied rev.
        rev: Rev,
    },
    /// Grid definition.
    #[serde(rename = "grid.get")]
    GridGet {
        /// Grid.
        grid_id: GridId,
    },
    /// Save measurements and traces to a session on the daemon host; the working state then
    /// belongs to it, with no unsaved changes. An existing session other than the working
    /// state's own is refused (`conflict`, `session_exists`) unless `overwrite`.
    #[serde(rename = "file.save")]
    FileSave {
        /// Where (`name` or `path`).
        session: SessionRef,
        /// Replace an existing session the working state does not belong to.
        overwrite: bool,
    },
    /// Replace measurements and traces with a saved session; always comes up disarmed with
    /// no generator owner, in a new session epoch. Refused (`conflict`, `unsaved_changes`)
    /// while the working state has unsaved changes, unless `discard`; what it replaces is
    /// then kept for `file.recover`.
    #[serde(rename = "file.load")]
    FileLoad {
        /// Which.
        session: SessionRef,
        /// Replace unsaved changes.
        discard: bool,
    },
    /// Start an untitled session: every measurement and trace deleted (the audio I/O
    /// stays open). Guarded as `file.load`.
    #[serde(rename = "file.new")]
    FileNew {
        /// Replace unsaved changes.
        discard: bool,
    },
    /// Swap the working state with the one the last `file.new` / `file.load` replaced
    /// (`working.recover`), disarmed as `file.load`.
    #[serde(rename = "file.recover")]
    FileRecover,
    /// Import the trace files of a session — one of another format version too — into the
    /// working state through `trace.import`'s reader, file by file.
    #[serde(rename = "file.salvage")]
    FileSalvage {
        /// Which.
        session: SessionRef,
    },
    /// Sessions in the daemon's session directory.
    #[serde(rename = "file.list")]
    FileList,
    /// Start uploading a session archive (`.ac2session`) of at most 64 MiB.
    #[serde(rename = "file.upload_begin")]
    FileUploadBegin {
        /// The archive (or session folder) as the client names it: its path on the
        /// client's machine. A loaded upload's `working.origin` is `client` {this}.
        file: String,
        /// Size, bytes.
        bytes: u64,
        /// SHA-256 of the whole archive, hex; checked when the last byte arrives.
        sha256: String,
    },
    /// The next chunk of an upload, in order.
    #[serde(rename = "file.upload_chunk")]
    FileUploadChunk {
        /// Upload.
        transfer: TransferId,
        /// Where it starts: the bytes received so far.
        offset: u64,
        /// At most `chunk_bytes`.
        data: Blob,
    },
    /// Store a complete upload as a session in the daemon's session directory, without
    /// opening it. An existing one is refused (`session_exists`) unless `overwrite`.
    #[serde(rename = "file.import")]
    FileImport {
        /// Upload.
        transfer: TransferId,
        /// Session name.
        name: String,
        /// Replace an existing session.
        overwrite: bool,
    },
    /// Pack a session archive to download: the working state (`None`) or a saved session.
    #[serde(rename = "file.download_begin")]
    FileDownloadBegin {
        /// Which (`name` or `path`); `None`: the working state.
        session: Option<SessionRef>,
    },
    /// A chunk of a download: `chunk_bytes` from `offset` (less at the end).
    #[serde(rename = "file.download_chunk")]
    FileDownloadChunk {
        /// Download.
        transfer: TransferId,
        /// Where it starts.
        offset: u64,
    },
    /// Done with a download. `saved_as`: the client wrote the working state's archive to
    /// that file, which the working state then belongs to (clean, unless it changed since
    /// `file.download_begin`).
    #[serde(rename = "file.download_end")]
    FileDownloadEnd {
        /// Download.
        transfer: TransferId,
        /// The file written, as the client names it.
        saved_as: Option<String>,
    },

    // -- plan ---------------------------------------------------------------------------
    /// Save a room plan to the plan library (`plans/<name>.json` in the daemon's data
    /// directory), replacing the one of that name. Refused with the reason when the plan
    /// does not validate (*edge 3 crosses edge 6*); the name is a file name (letters,
    /// digits, ` `, `-`, `_`, `.`, not starting with `.`). Walks keep the session's copies.
    #[serde(rename = "plan.save")]
    PlanSave {
        /// The plan.
        plan: PlanSpec,
    },
    /// The plan library.
    #[serde(rename = "plan.list")]
    PlanList,
    /// Delete a plan from the library; the session's copies (its walks' plans) stay.
    #[serde(rename = "plan.delete")]
    PlanDelete {
        /// Plan name.
        name: String,
    },

    // -- walk ---------------------------------------------------------------------------
    /// Start a spatial walk through `points` of plan `plan` (the session's copy when it holds
    /// one, else the library's, copied into the session). A live walk's `meas` are the
    /// transfer measurements that capture at the points, one per mic, each on its own
    /// measurement input; a sweep walk's `meas` is its one sweep measurement, whose
    /// measurement inputs are the mics (at most [`crate::model::WalkState::MAX_MICS`] either
    /// way). The points are grouped into steps that place every mic once, each step's mics
    /// carried least from the previous one (`ac2_room::steps`). The walk goes to its first
    /// step: a live walk's measurements' averages are reset and their delay tracking turned
    /// on (a sweep has neither: its run at a step is the capture).
    #[serde(rename = "walk.new")]
    WalkNew {
        /// Walk name.
        name: String,
        /// Plan name.
        plan: String,
        /// What a capture is.
        kind: WalkKind,
        /// The measurements capturing at the points: a live walk's in mic order, a sweep
        /// walk's one.
        meas: Vec<MeasId>,
        /// The points, in walking order (at most [`crate::model::WalkState::MAX_POINTS`]).
        points: Vec<WalkPointSpec>,
    },
    /// Rename a walk and edit its points, matched by label: a point kept keeps its state,
    /// a new one is pending, and the points are grouped into steps again. A captured point
    /// keeps its place and its mic and cannot be removed (skip or redo it first).
    #[serde(rename = "walk.update")]
    WalkUpdate {
        /// The walk.
        walk: WalkId,
        /// New name.
        name: String,
        /// The points, in walking order.
        points: Vec<WalkPointSpec>,
    },
    /// Go to step `step` (moving the mics restarts the walk's measurements).
    #[serde(rename = "walk.goto")]
    WalkGoto {
        /// The walk.
        walk: WalkId,
        /// Step index.
        step: u16,
    },
    /// Capture at every pending point of the current step in one step, each by its mic's
    /// measurement, each trace named *<point> · <mic>* and carrying its position, and go to
    /// the next step with a pending point. Before a measurement is ready (averages full, no
    /// delay move since they restarted, no arrival to pick, no fault) its trace records
    /// `captured_unsettled`. The walk's average is made at the second capture and follows
    /// every capture, redo and skip. A sweep walk refuses it: its capture is a run of its
    /// sweep measurement (`sweep.run`, under the stimulus lease), which records every
    /// standing mic at its point.
    #[serde(rename = "walk.capture")]
    WalkCapture {
        /// The walk.
        walk: WalkId,
    },
    /// Skip point `point` (a captured one's trace becomes superseded, out of the average);
    /// when that leaves the current step nothing pending, go to the next step with a
    /// pending point.
    #[serde(rename = "walk.skip")]
    WalkSkip {
        /// The walk.
        walk: WalkId,
        /// Point index.
        point: u16,
    },
    /// Redo point `point`: its capture becomes superseded (kept, out of the average), it is
    /// pending again for the mic that captured it, and the walk goes to its step.
    #[serde(rename = "walk.redo")]
    WalkRedo {
        /// The walk.
        walk: WalkId,
        /// Point index.
        point: u16,
    },
    /// The mics standing at `points` (pending points of the current step, at least two) move
    /// round by one: the mic at each goes to the next listed point, the last one's to the
    /// first, so two points trade mics. Later steps follow the mics: the same exchange
    /// applies to their pending points, so each mic goes on from where it now stands.
    #[serde(rename = "walk.swap")]
    WalkSwap {
        /// The walk.
        walk: WalkId,
        /// Point indices.
        points: Vec<u16>,
    },
    /// Delete a walk. Its traces and its average stay, as ordinary traces and an ordinary
    /// average of them.
    #[serde(rename = "walk.delete")]
    WalkDelete {
        /// The walk.
        walk: WalkId,
    },

    // -- rec ----------------------------------------------------------------------------
    /// Record inputs of the open session to a raw capture file (f32 WAV / RF64 and a JSON
    /// sidecar) in the daemon's recording directory. One recording at a time; the reply is
    /// the started run, whose progress and end follow as `recording` events.
    #[serde(rename = "rec.start")]
    RecStart {
        /// What to record and its bounds.
        request: RecordRequest,
    },
    /// End the recording and finalise its file.
    #[serde(rename = "rec.stop")]
    RecStop,
    /// Recordings in the daemon's recording directory.
    #[serde(rename = "rec.list")]
    RecList,
    /// How the daemon serves clients: transport, server key, mDNS name, the authorized
    /// client keys and the keys refused lately.
    #[serde(rename = "server.info")]
    ServerInfo,
    /// Authorize a client key under `name` (network mode). Applies to connections from now
    /// on; written to the authorized-clients file.
    #[serde(rename = "server.authorize")]
    ServerAuthorize {
        /// Name the client's requests will carry (1 … 64 characters, no spaces).
        name: String,
        /// The client's public key (Z85, 40 characters).
        key: String,
    },
    /// Revoke the client key named `name` (network mode): its requests are refused at once,
    /// and it cannot connect again. A client cannot revoke its own key.
    #[serde(rename = "server.revoke")]
    ServerRevoke {
        /// The authorized name.
        name: String,
    },
    /// Open a session that plays a recording instead of a device (new epoch): its inputs
    /// are the recorded ones under their device numbers, it has no outputs, and the running
    /// measurements analyse it as they would the device. A replay never reopens itself: it
    /// stops after the last frame.
    #[serde(rename = "session.replay")]
    SessionReplay {
        /// Which recording.
        recording: RecordingRef,
        /// How fast.
        pace: ReplayPace,
    },
}

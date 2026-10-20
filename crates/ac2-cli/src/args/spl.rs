//! `ac2 spl …`: SPL meter, band Leq and Leq meters.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use super::{CalSpl, MeasRef, TimeWeightArg, WeightArg};
use ac2_scene::unit_text::{
    BandLimitArg, BandSourceArg, BandWindowArg, Channel, DayOffsetArg, Gain, LeqLimitArg,
    LeqWindowArg, PeakLimitArg, PositionArg, Time, TimeRef,
};

/// `spl …`.
#[derive(Debug, Subcommand)]
pub enum SplCmd {
    /// Live SPL readout (q/Esc/Ctrl-C quits). Uses `--meas`, or its own SPL meter on
    /// `--input`, created for the duration of the command (Leq, Lmax and Lmin integrate
    /// from the command's start).
    Watch(SplWatch),
    /// Calibrate an input against an acoustic calibrator (same as `cal spl`).
    Cal(CalSpl),
    /// Rolling Leq windows of an SPL meter: watch them, set windows and limits, export the
    /// per-second log.
    Leq {
        #[command(subcommand)]
        cmd: LeqCmd,
    },
    /// Set a running meter's frequency and time weighting (`--weight c --time slow`): it
    /// carries on, its Leq windows and log untouched.
    Set(SplSet),
    /// The band meter of an SPL meter: 1/3-octave band Leq in windows of one band each,
    /// their own length and weighting, each against its own limit, judged at the mic as typed or moved there from
    /// a named place by a measured band transfer, and the predicted LAeq at that place.
    /// Informational, not legal advice.
    Bands {
        #[command(subcommand)]
        cmd: BandsCmd,
    },
}

/// `spl bands …`.
#[derive(Debug, Subcommand)]
pub enum BandsCmd {
    /// The band windows, those of one length and weighting on one row, the band nearest
    /// its limit and what to do about it, the predicted LAeq (q/Esc/Ctrl-C quits). With
    /// `--json` one line per second, the windows in an array (each its `band_hz`).
    Watch(LeqWatch),
    /// Turn the band meter on, set its windows, limits and §13 corrections (a preset sets
    /// windows and limits at once), or turn it off; the meter, its log
    /// and a measured transfer carry on.
    Set(BandsSet),
    /// Measure the band transfer FOH → a place (`--place`, `receiving room` unless named)
    /// from band levels at FOH and at the place of the same steady test signal, and the
    /// place's background with the system silent; stored in the meter's band meter.
    Transfer(BandsTransfer),
    /// A span of the meter's band log: the energy average per band (as `transfer` takes
    /// it from `METER@FROM..UNTIL`) and how much of the span was logged; `--step` adds the
    /// logged seconds. `--levels-out` writes the averages as a `<Hz> <dB>` file that
    /// `transfer --place-levels FILE` (or `--foh`, `--background`) reads on another rig.
    Log(BandsLog),
    /// Store an estimated transfer where the place cannot be reached: per-band
    /// attenuation typed from a `<Hz> <dB>` file, marked estimated everywhere it shows.
    Estimate(BandsEstimate),
}

/// `spl bands estimate`.
#[derive(Debug, Args)]
pub struct BandsEstimate {
    #[command(flatten)]
    pub meter: MeterRef,
    /// `<Hz> <dB>` lines: each band's estimated attenuation FOH → the place (dB, ≥ 0); a
    /// band not in the file has no limit at FOH.
    #[arg(long, value_name = "FILE")]
    pub attenuation: std::path::PathBuf,
    /// The place the limits are for, as every line naming it says: `flat 4 bedroom`.
    #[arg(long, value_name = "NAME", default_value = ac2_proto::model::BandTransferSet::DEFAULT_PLACE)]
    pub place: String,
}

/// Band meter presets (not legal advice; `docs/design/band-leq.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BandPresetArg {
    /// Finland, STM 545/2015 Liite 2 Taulukko 2, rooms meant for sleeping: LZeq 60 min,
    /// 20 … 200 Hz, night 74 … 32 dB, day 5 dB higher; predicted LAeq 60 min at night
    /// ≤ 25 dB.
    #[value(name = "finland-545-lf")]
    Finland545Lf,
    /// Finland, STM 545/2015 Liite 2 Taulukko 1, living rooms: LZeq 60 min, 20 … 200 Hz
    /// without band limits; predicted LAeq 60 min day ≤ 35 dB, night ≤ 30 dB.
    #[value(name = "finland-545-living-room")]
    Finland545LivingRoom,
}

/// §13 impulse correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ImpulseArg {
    None,
    /// +5 dB.
    #[value(name = "5")]
    Plus5,
    /// +10 dB.
    #[value(name = "10")]
    Plus10,
}

/// §13 narrowband (tonal) correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TonalArg {
    None,
    /// +3 dB.
    #[value(name = "3")]
    Plus3,
    /// +6 dB.
    #[value(name = "6")]
    Plus6,
}

/// `spl bands set`.
#[derive(Debug, Args)]
#[command(group(clap::ArgGroup::new("what").required(true).multiple(true)
    .args(["preset", "windows", "limits", "day_offsets", "impulse", "tonal", "warn",
        "off"])))]
pub struct BandsSet {
    #[command(flatten)]
    pub meter: MeterRef,
    /// A rule's windows, limits and predicted window (the corrections and a
    /// transfer are kept); everything stays settable after. Informational, not legal
    /// advice.
    #[arg(long, value_enum)]
    pub preset: Option<BandPresetArg>,
    /// The band windows, replacing the meter's, each one band: its length and weighting,
    /// then its band after `@`: `z:60min@63hz,z:1min@20hz` (Z unless `a:` or `c:`; any Leq
    /// length up to 24 h). `z:60min@20hz..200hz` is one window per band 20 … 200 Hz. A
    /// window kept (same band, length and weighting) keeps its limit; the same one twice is
    /// refused; at most 64.
    #[arg(long, value_delimiter = ',', value_name = "WINDOWS")]
    pub windows: Option<Vec<BandWindowArg>>,
    /// A band window's limit, dB SPL where the limits apply: `z:60min@63hz=42db`
    /// (`z:60min@20hz..200hz=…` sets every band's window of the range); `…=none` removes it;
    /// repeatable.
    #[arg(long = "limit", value_name = "WINDOW=LIMIT")]
    pub limits: Vec<BandLimitArg>,
    /// `z:60min@63hz=5db`: the window's limit holds at night (22–07), the day's this much
    /// higher (a range sets each band's window); `…=none`: one limit day and night;
    /// repeatable.
    #[arg(long = "day-offset", value_name = "WINDOW=DB")]
    pub day_offsets: Vec<DayOffsetArg>,
    /// §13 impulse correction in force from now: `none`, `5` or `10` dB.
    #[arg(long, value_enum)]
    pub impulse: Option<ImpulseArg>,
    /// §13 narrowband correction in force from now: `none`, `3` or `6` dB.
    #[arg(long, value_enum)]
    pub tonal: Option<TonalArg>,
    /// Warn margin of every band window: near when this close below a limit, e.g. `3db`.
    #[arg(long, value_name = "DB")]
    pub warn: Option<Gain>,
    /// Turn the band meter off (its configuration and transfer go).
    #[arg(long, conflicts_with_all = ["preset", "windows", "limits", "day_offsets",
        "impulse", "tonal", "warn"])]
    pub off: bool,
}

/// `spl bands transfer`. Each source is a file of `<Hz> <dB>` lines (dB SPL per 1/3-octave
/// band, from another instrument), or a span of an SPL meter's band log:
/// `METER@FROM..UNTIL`, the meter by name or id, the times `21:00:30` (local, today),
/// `2026-10-08T21:00:30` (local), `…Z` (UTC), `-30s` (before now) or `now`.
#[derive(Debug, Args)]
pub struct BandsTransfer {
    #[command(flatten)]
    pub meter: MeterRef,
    /// Band levels at FOH (the meter's mic) with the test signal playing.
    #[arg(long, value_name = "SOURCE")]
    pub foh: BandSourceArg,
    /// Band levels at the place, the same signal at the same level.
    #[arg(long = "place-levels", value_name = "SOURCE")]
    pub at_place: BandSourceArg,
    /// The place's background with the system silent; without it every band is
    /// unchecked.
    #[arg(long, value_name = "SOURCE")]
    pub background: Option<BandSourceArg>,
    /// The place the limits are for, as every line naming it says: `flat 4 bedroom`.
    #[arg(long, value_name = "NAME", default_value = ac2_proto::model::BandTransferSet::DEFAULT_PLACE)]
    pub place: String,
}

/// `spl bands log`. Times: `21:00:30` (local, today), `2026-10-08T21:00:30` (local),
/// `…Z` (UTC), `-30s` (before now) or `now`.
#[derive(Debug, Args)]
pub struct BandsLog {
    #[command(flatten)]
    pub meter: MeterRef,
    /// Start of the span.
    #[arg(long, value_name = "TIME", allow_hyphen_values = true)]
    pub from: TimeRef,
    /// End of the span.
    #[arg(
        long,
        value_name = "TIME",
        default_value = "now",
        allow_hyphen_values = true
    )]
    pub until: TimeRef,
    /// Also every STEP-th logged second (1: each; at most 3600 rows).
    #[arg(long, value_name = "STEP")]
    pub step: Option<u32>,
    /// Write the span's per-band averages (dB SPL) to FILE as `<Hz> <dB>` lines.
    #[arg(long, value_name = "FILE")]
    pub levels_out: Option<std::path::PathBuf>,
}

/// `spl set`.
#[derive(Debug, Args)]
#[command(group(clap::ArgGroup::new("what").required(true).multiple(true).args(["weight", "time"])))]
pub struct SplSet {
    #[command(flatten)]
    pub meter: MeterRef,
    /// Frequency weighting.
    #[arg(long, value_enum)]
    pub weight: Option<WeightArg>,
    /// Time weighting.
    #[arg(long, value_enum)]
    pub time: Option<TimeWeightArg>,
}

/// `spl leq …`.
#[derive(Debug, Subcommand)]
pub enum LeqCmd {
    /// Big-number view of the meter's windows: value, limit, state, headroom (q/Esc/Ctrl-C
    /// quits). With `--json` one line per second.
    Watch(LeqWatch),
    /// Set the meter's windows, limits, warn margin, a preset or the headroom horizon; the
    /// meter, its log and its windows carry on.
    Set(LeqSet),
    /// Write the meter's per-second log (LAeq, LCeq, LZeq per second) as CSV.
    Export(LeqExport),
    /// End the meter's log and start a new one (show start after a loud soundcheck): the
    /// windows, their states, the alarms, the run clock and the total start over; windows
    /// and limits are kept. Asks for `--yes`; `--export FILE` writes the ended log first.
    New(LeqNew),
}

/// Which SPL meter: `--meas`, the one on `--input`, or the only one.
#[derive(Debug, Args)]
pub struct MeterRef {
    /// SPL measurement (id or name).
    #[arg(long, conflicts_with = "input")]
    pub meas: Option<MeasRef>,
    /// The SPL meter on this input.
    #[arg(long)]
    pub input: Option<Channel>,
}

/// `spl leq watch`.
#[derive(Debug, Args)]
pub struct LeqWatch {
    #[command(flatten)]
    pub meter: MeterRef,
    /// Quit after this long, e.g. `10s` (scripts).
    #[arg(long = "for", value_name = "TIME")]
    pub duration: Option<Time>,
}

/// Informational limit presets (not legal advice; `docs/design/leq.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PresetArg {
    /// DIN 15905-5: LAeq 30 min ≤ 99 dB.
    #[value(name = "din15905")]
    Din15905,
    /// Swiss V-NISSG: LAeq 60 min ≤ 93 dB.
    #[value(name = "swiss93")]
    Swiss93,
    /// Swiss V-NISSG: LAeq 60 min ≤ 96 dB.
    #[value(name = "swiss96")]
    Swiss96,
    /// Swiss V-NISSG: LAeq 60 min ≤ 100 dB.
    #[value(name = "swiss100")]
    Swiss100,
    /// WHO safe listening (2022): LAeq 15 min ≤ 100 dB.
    Who,
    /// France, Code de la santé publique art. R1336-1: LAeq 15 min ≤ 102 dB, LCeq 15 min
    /// ≤ 118 dB.
    #[value(name = "france")]
    France,
    /// France, art. R1336-1, events for children up to six: LAeq 15 min ≤ 94 dB, LCeq
    /// 15 min ≤ 104 dB.
    #[value(name = "france-children")]
    FranceChildren,
    /// Flanders, VLAREM II art. 6.7.3: LAeq 15 min ≤ 85 dB.
    #[value(name = "flanders-85")]
    Flanders85,
    /// Flanders, VLAREM II art. 5.32.2.2bis § 1: LAeq 15 min ≤ 95 dB.
    #[value(name = "flanders-95")]
    Flanders95,
    /// Flanders, VLAREM II art. 5.32.2.2bis § 2: LAeq 60 min ≤ 100 dB, LAeq 15 min shown.
    #[value(name = "flanders-100")]
    Flanders100,
    /// Brussels-Capital, arrêté of 26 January 2017 art. 3: LAeq 15 min ≤ 85 dB.
    #[value(name = "brussels-85")]
    Brussels85,
    /// Brussels-Capital, art. 4: LAeq 15 min ≤ 95 dB, LCeq 15 min ≤ 110 dB.
    #[value(name = "brussels-95")]
    Brussels95,
    /// Brussels-Capital, art. 5: LAeq 60 min ≤ 100 dB, LCeq 60 min ≤ 115 dB.
    #[value(name = "brussels-100")]
    Brussels100,
    /// Netherlands, fourth covenant (voluntary) art. 3.1.2: LAeq 15 min ≤ 103 dB.
    #[value(name = "nl-covenant")]
    NlCovenant,
    /// Netherlands covenant art. 3.1.3, audiences of 16–17: LAeq 15 min ≤ 100 dB.
    #[value(name = "nl-covenant-16-17")]
    NlCovenant16To17,
    /// Netherlands covenant art. 3.1.3, audiences of 14–15: LAeq 15 min ≤ 96 dB.
    #[value(name = "nl-covenant-14-15")]
    NlCovenant14To15,
    /// Netherlands covenant art. 3.1.3, audiences up to 13: LAeq 15 min ≤ 91 dB.
    #[value(name = "nl-covenant-13")]
    NlCovenantTo13,
    /// Finland, STM 545/2015 §12: LAeq 4 h ≤ 100 dB, LAFmax ≤ 115 dB, LCpeak ≤ 140 dB.
    #[value(name = "finland-545")]
    Finland545,
}

/// `spl leq set`.
#[derive(Debug, Args)]
pub struct LeqSet {
    #[command(flatten)]
    pub meter: MeterRef,
    /// The windows, replacing the meter's: `1min,5min,10min,30min,60min` (A-weighted;
    /// `c:30s` for C). Windows kept keep their limits. With `--preset`: windows added to
    /// the preset's, without limits.
    #[arg(long, value_delimiter = ',', value_name = "WINDOWS")]
    pub windows: Option<Vec<LeqWindowArg>>,
    /// Replace the windows with exactly a preset's, with its limits (the meter's other
    /// windows and limits go); repeatable: the windows of all, a window two share at the
    /// lower limit. Informational, not legal advice.
    #[arg(long, value_enum)]
    pub preset: Vec<PresetArg>,
    /// A window's limit: `30min=99db`, `30min=none` removes it; repeatable.
    #[arg(long = "limit", value_name = "WINDOW=LIMIT")]
    pub limits: Vec<LeqLimitArg>,
    /// Warn margin of every window: near when this close below the limit, e.g. `3db`.
    #[arg(long, value_name = "DB")]
    pub warn: Option<Gain>,
    /// Headroom horizon: the steady level allowed over this much of the future, e.g. `1min`.
    #[arg(long, value_name = "WINDOW")]
    pub horizon: Option<LeqWindowArg>,
    /// A peak limit, judged on the highest second of the last 10 s: `lcpeak=135db`,
    /// `lafmax=125db`, `lcpeak=none` removes it; repeatable. A preset sets its rule's.
    #[arg(long = "peak-limit", value_name = "QUANTITY=LIMIT")]
    pub peak_limits: Vec<PeakLimitArg>,
    /// Measuring-position correction added to every level the meter reports (and judges)
    /// once calibrated: the difference from the mic to where the limit applies, e.g. `4db`
    /// (FOH → loudest audience spot); `none` removes it. The log keeps what was measured.
    #[arg(long, value_name = "DB")]
    pub position: Option<PositionArg>,
    /// The correction of the peak levels (LCpeak), when it differs from `--position`'s.
    #[arg(long, value_name = "DB")]
    pub position_peak: Option<Gain>,
}

/// `spl leq export`.
#[derive(Debug, Args)]
pub struct LeqExport {
    #[command(flatten)]
    pub meter: MeterRef,
    /// Write to this file (default: standard output).
    #[arg(long, short = 'o', value_name = "FILE")]
    pub out: Option<PathBuf>,
    /// The log `spl leq new` ended last (kept until the next new log or a daemon restart).
    #[arg(long)]
    pub previous: bool,
}

/// `spl leq new`.
#[derive(Debug, Args)]
pub struct LeqNew {
    #[command(flatten)]
    pub meter: MeterRef,
    /// Write the ended log to this file as CSV (as `spl leq export`), all of it.
    #[arg(long, value_name = "FILE")]
    pub export: Option<PathBuf>,
    /// Go ahead: the current log's windows, alarms, run clock and total are discarded.
    #[arg(long)]
    pub yes: bool,
}

/// `spl watch`.
#[derive(Debug, Args)]
pub struct SplWatch {
    /// Existing SPL measurement.
    #[arg(long, conflicts_with = "input")]
    pub meas: Option<MeasRef>,
    /// Input channel.
    #[arg(long, required_unless_present = "meas")]
    pub input: Option<Channel>,
    /// Frequency weighting.
    #[arg(long, value_enum, default_value = "a")]
    pub weight: WeightArg,
    /// Time weighting.
    #[arg(long, value_enum, default_value = "fast")]
    pub time: TimeWeightArg,
    /// Quit after this long, e.g. `10s` (scripts).
    #[arg(long = "for", value_name = "TIME")]
    pub duration: Option<Time>,
}

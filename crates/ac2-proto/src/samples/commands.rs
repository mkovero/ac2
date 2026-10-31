//! Every command.

use super::builders::*;
use super::state::*;
use crate::ctrl::Command;
use crate::model::*;
use crate::units::*;

/// One example of every [`Command`] variant.
pub fn commands() -> Vec<Command> {
    vec![
        Command::Hello {
            client: "ac2-cli 0.0.0".into(),
        },
        Command::SessionDevices,
        Command::SessionOpen {
            config: session_config(),
        },
        Command::SessionClose,
        Command::SessionStatus,
        Command::GenAcquire { force: false },
        Command::GenSet {
            lease_token: token(),
            desired: GeneratorDesired {
                settings: settings(),
                armed: true,
                firing: false,
            },
        },
        Command::GenRefresh {
            lease_token: token(),
        },
        Command::GenRelease {
            lease_token: token(),
        },
        Command::GenStop,
        Command::MeasCreate {
            config: meas_config(),
        },
        Command::MeasUpdate {
            meas: MeasId(3),
            base: Rev(40),
            config: MeasConfig {
                scope: crate::model::MeasScope::Session,
                name: "RTA".into(),
                kind: MeasKind::Rta {
                    config: RtaConfig {
                        input: 2,
                        fraction: BandFraction::Third,
                        f_lo: Hz(20.0),
                        f_hi: Hz(20_000.0),
                        weighting: Weighting::Z,
                        averaging: SpecAveraging::Exponential {
                            time_constant: Seconds(1.0),
                        },
                        display: crate::model::SpectrumDisplay {
                            style: crate::model::SpectrumStyle::Fill,
                            peak: crate::model::PeakMode::Track,
                            spectrograph_span_s: 60,
                            spectrograph_colours: crate::model::Colormap::Magma,
                        },
                    },
                },
            },
        },
        Command::MeasDelete {
            meas: MeasId(3),
            traces: OwnedTraces::Keep,
        },
        Command::MeasStart { meas: MeasId(1) },
        Command::MeasStop { meas: MeasId(1) },
        Command::MeasReset { meas: MeasId(1) },
        Command::DelayFind {
            meas: MeasId(1),
            band: FinderBand::Sub,
            observation: Some(Seconds(8.0)),
        },
        Command::DelayInsert {
            meas: MeasId(1),
            pick: DelayPick::Ranked { index: 1 },
        },
        Command::DelaySet {
            meas: MeasId(1),
            delay: Seconds(0.012_5),
        },
        Command::DelayTrack {
            meas: MeasId(1),
            enabled: true,
        },
        Command::TraceCapture {
            meas: MeasId(1),
            name: "Main L".into(),
            slot: Some(3),
        },
        Command::TraceList,
        Command::TraceGet { trace: TraceId(7) },
        Command::TraceUpdate {
            trace: TraceId(7),
            edit: edit(),
        },
        Command::TraceDelete { trace: TraceId(7) },
        Command::TraceAverage {
            traces: vec![TraceId(7), TraceId(8)],
            method: AverageMethod::CoherenceWeighted,
            reference: DelayReference::Trace { trace: TraceId(7) },
            name: "avg".into(),
        },
        Command::TraceImport {
            file_name: "genelec.wav".into(),
            format: ImportFormat::IrWav,
            role: ImportRole::Trace,
            ir: Some(IrImport {
                channel: Some(1),
                window: IrImportWindow::Gate {
                    pre: Seconds(0.001),
                    post: Seconds(0.01),
                },
                align: IrAlign::Peak,
                resolution: Resolution::FortyEighth,
            }),
            content: Blob(b"RIFF".to_vec()),
            owner: crate::model::TraceOwner::Imported,
        },
        Command::TraceExport {
            trace: TraceId(8),
            format: ExportFormat::IrWav,
        },
        Command::CalSpl {
            input: 1,
            mic: "M30 #1234".into(),
            calibrator_level: DbSpl(94.0),
            calibrator_freq: Hz(1000.0),
        },
        Command::CalCurveImport {
            mic: "M30 #1234".into(),
            label: Some("0°".into()),
            file_name: "M30-1234.frd".into(),
            content: Blob(b"20 -0.5\n20000 1.5\n".to_vec()),
            input: Some(1),
        },
        Command::CalList,
        Command::SplLogGet {
            meas: MeasId(4),
            log: SplLogWhich::Previous,
            from: 120,
            max: 3600,
        },
        Command::SweepRun {
            lease_token: token(),
            meas: MeasId(7),
            name: Some("1083 sweep".into()),
        },
        Command::StateSnapshot,
        Command::StateSince { rev: Rev(41) },
        Command::GridGet {
            grid_id: log_grid().id(),
        },
        Command::FileSave {
            session: SessionRef::Name {
                name: "friday show".into(),
            },
            overwrite: false,
        },
        Command::FileLoad {
            session: SessionRef::Path {
                path: "/tmp/show".into(),
            },
            discard: true,
        },
        Command::FileList,
        Command::SessionInputs {
            inputs: inputs().inputs,
            pairs: inputs().pairs,
        },
        Command::CalDelete {
            key: CalRef::Line(line_cal_key()),
        },
        Command::SessionPreview {
            backend: BackendKind::Jack,
            device: DeviceId("jack".into()),
            inputs: vec![
                PreviewInput {
                    channel: 0,
                    kind: InputKind::Line,
                },
                PreviewInput {
                    channel: 1,
                    kind: InputKind::Line,
                },
            ],
            pairs: vec![Pair {
                left: 0,
                right: 1,
                name: None,
            }],
        },
        Command::SessionPreviewStop,
        Command::SessionDetectLoopback {
            lease_token: token(),
            backend: BackendKind::Jack,
            input_device: DeviceId("jack".into()),
            output_device: DeviceId("jack".into()),
            output: 0,
            level: Some(Dbfs(-30.0)),
        },
        Command::TraceMicCurve {
            trace: TraceId(8),
            curve: Some(curve_id()),
        },
        Command::CalCurveRename {
            curve: curve_id(),
            label: "on axis".into(),
        },
        Command::CalCurveDelete { curve: curve_id() },
        Command::SplLogNew { meas: MeasId(4) },
        Command::CalSplElectrical {
            input: 1,
            mic: "M30 #1234".into(),
            connection: ElectricalConnection::InLine,
            volts: Volts(0.015),
            freq: Hz(1000.0),
            mic_sensitivity: Some(MvPerPa(15.0)),
            loading: Some(MicLoading {
                mic_impedance: Ohms(330.0),
                input_impedance: Ohms(2000.0),
            }),
            uncertainty: None,
            replace_acoustic: false,
        },
        Command::SplHistoryGet {
            meas: MeasId(4),
            span: HistorySpan::Range {
                from: WallNs(1_789_500_000_000_000_000),
                to: WallNs(1_789_510_800_000_000_000),
                step_s: 30,
            },
        },
        Command::DelayNudge {
            meas: MeasId(1),
            by: Seconds(-0.25 / 48_000.0),
        },
        Command::RecStart {
            request: RecordRequest {
                inputs: vec![0, 1],
                name: Some("soundcheck".into()),
                max_duration: Seconds(600.0),
                max_bytes: None,
            },
        },
        Command::RecStop,
        Command::RecList,
        Command::SessionReplay {
            recording: RecordingRef::Name {
                name: "soundcheck".into(),
            },
            pace: ReplayPace::Fast,
        },
        Command::GenCeiling {
            ceiling: Dbfs(-40.0),
            confirm_raise: true,
        },
        Command::SessionOutputs {
            outputs: vec![
                OutputSetup {
                    channel: 0,
                    label: Some("Main L".into()),
                },
                OutputSetup {
                    channel: 1,
                    label: None,
                },
            ],
        },
        Command::ServerInfo,
        Command::ServerAuthorize {
            name: "laptop".into(),
            key: "Yne@$w-vo<fVvi]a<NY6T1ed:M$fCG*[IaLV{hID".into(),
        },
        Command::ServerRevoke {
            name: "laptop".into(),
        },
        Command::SplBandTransfer {
            meas: MeasId(4),
            foh: BandLevelSource::Log {
                meas: MeasId(4),
                from: WallNs(1_789_500_000_000_000_000),
                until: WallNs(1_789_500_030_000_000_000),
            },
            at_place: BandLevelSource::Levels {
                levels: (0..BAND_COUNT)
                    .map(|i| (i < 3).then_some(DbSpl(50.0 - i as f64)))
                    .collect(),
            },
            background: None,
            place: "flat 4 bedroom".into(),
        },
        Command::SplBandLogGet {
            meas: MeasId(4),
            from: WallNs(1_789_500_000_000_000_000),
            until: WallNs(1_789_500_030_000_000_000),
            step: Some(10),
        },
        Command::MeasColour {
            meas: MeasId(3),
            colour: Some(ColourFamily::Hue { index: 2 }),
        },
        Command::FileNew { discard: false },
        Command::FileRecover,
        Command::FileSalvage {
            session: SessionRef::Upload {
                transfer: TransferId(3),
            },
        },
        Command::FileUploadBegin {
            file: "/run/media/foh/STICK/friday show.ac2session".into(),
            bytes: 1_234_567,
            sha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into(),
        },
        Command::FileUploadChunk {
            transfer: TransferId(3),
            offset: 1_048_576,
            data: Blob(vec![0x1f, 0x8b, 0x08, 0x00]),
        },
        Command::FileImport {
            transfer: TransferId(3),
            name: "friday show".into(),
            overwrite: true,
        },
        Command::FileDownloadBegin { session: None },
        Command::FileDownloadChunk {
            transfer: TransferId(4),
            offset: 0,
        },
        Command::FileDownloadEnd {
            transfer: TransferId(4),
            saved_as: Some("/run/media/foh/STICK/friday show.ac2session".into()),
        },
        Command::MeasLayout {
            meas: MeasId(1),
            layout: Some(pane_layout()),
        },
        Command::SessionControlPanel {
            backend: BackendKind::Asio,
            device: DeviceId("Fireface USB".into()),
        },
        Command::OscTarget {
            base: Rev(38),
            addr: Some("[fe80::1]:9000".parse().expect("valid sample address")),
        },
        Command::OscTest {
            msg: osc_message("/light/red", vec![OscArg::Int(1)], true),
        },
        Command::RigMonitorStart {
            base: Rev(38),
            on: true,
        },
        Command::ArtnetTarget {
            base: Rev(38),
            target: Some(ArtnetTarget {
                addr: "2.255.255.255:6454".parse().expect("valid sample address"),
                universe: PortAddress::from_parts(0, 1, 2).expect("valid sample universe"),
            }),
        },
        Command::ArtnetTest {
            channels: dmx("12=255 13=40"),
        },
        Command::ArtnetFade {
            base: Rev(38),
            seconds: Seconds(2.5),
        },
        Command::MeasCompare {
            meas: MeasId(4),
            compared: vec![
                crate::model::Compared::Meas { meas: MeasId(5) },
                crate::model::Compared::Trace {
                    trace: crate::units::TraceId(7),
                },
            ],
        },
        Command::MeasOffset {
            meas: MeasId(4),
            offset: crate::units::Db(6.0),
        },
        Command::CalLine {
            channel: 4,
            volts: Volts(1.2277),
        },
    ]
    .into_iter()
    .chain(super::eq::eq_commands())
    .chain([Command::TraceTarget {
        curve: TargetCurve::Tilt {
            slope_db_per_oct: -1.0,
            pivot: Hz(1000.0),
            lf_shelf: Some(LfShelf {
                gain: Db(6.0),
                corner: Hz(100.0),
            }),
        },
        name: "tilt −1.0 dB/oct @ 1 kHz, LF +6.0 dB @ 100 Hz".into(),
        owner: crate::model::TraceOwner::Meas { meas: MeasId(4) },
    }])
    .chain([
        Command::LevelsSet {
            base: Rev(38),
            ballistic: Some(crate::model::Ballistic::ShortTerm),
            correlation_window: None,
            over_threshold: Some(crate::units::Dbtp(-1.0)),
            correlation_gate: None,
            loudness_target: Some(crate::units::Lufs(-16.0)),
        },
        Command::LevelsReset {
            channels: vec![0, 1],
        },
        Command::PlanSave { plan: plan() },
        Command::PlanList,
        Command::PlanDelete {
            name: "Club".into(),
        },
        Command::WalkNew {
            name: "Mains".into(),
            plan: "Hall".into(),
            kind: WalkKind::LiveTransfer,
            meas: vec![MeasId(1), MeasId(2)],
            points: vec![
                WalkPointSpec {
                    label: "P1".into(),
                    at: [5.0, 12.0, 1.2],
                },
                WalkPointSpec {
                    label: "P2".into(),
                    at: [12.5, 15.0, 1.2],
                },
            ],
        },
        Command::WalkUpdate {
            walk: WalkId(1),
            name: "Mains L".into(),
            points: vec![WalkPointSpec {
                label: "P1".into(),
                at: [5.0, 12.0, 1.2],
            }],
        },
        Command::WalkGoto {
            walk: WalkId(1),
            step: 1,
        },
        Command::WalkCapture { walk: WalkId(1) },
        Command::WalkSkip {
            walk: WalkId(1),
            point: 1,
        },
        Command::WalkSwap {
            walk: WalkId(1),
            points: vec![2, 3],
        },
        Command::WalkRedo {
            walk: WalkId(1),
            point: 0,
        },
        Command::WalkDelete { walk: WalkId(1) },
        Command::SimRun {
            request: super::sim::sim_request(),
        },
        Command::SimFit {
            request: super::sim::sim_fit_request(),
        },
        Command::SimMap {
            request: super::sim::sim_map_request(),
        },
        Command::SimCancel,
    ])
    .collect()
}

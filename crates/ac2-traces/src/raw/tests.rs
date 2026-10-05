use super::*;
use ac2_proto::model::{BackendKind, ClockRelation};

fn sidecar(name: &str) -> Sidecar {
    let st = ac2_proto::samples::state();
    Sidecar {
        format: FORMAT.into(),
        version: VERSION,
        software: Software {
            ac2: "ac2d 0.0.0".into(),
            build: "test".into(),
            protocol: ac2_proto::PROTO_VERSION,
        },
        audio: AudioFile {
            file: format!("{name}{AUDIO_SUFFIX}"),
            sample_rate: 48_000,
            channels: vec![
                RecordedChannel {
                    input: 0,
                    name: Some("Loop return".into()),
                    mic: None,
                    roles: vec![
                        ChannelRole::Loopback,
                        ChannelRole::Reference {
                            measurement: "main".into(),
                        },
                    ],
                },
                RecordedChannel {
                    input: 1,
                    name: Some("Room mic".into()),
                    mic: Some("M30".into()),
                    roles: vec![ChannelRole::Measured {
                        measurement: "main".into(),
                    }],
                },
            ],
        },
        device: RecordedDevice {
            backend: BackendKind::Fake,
            input_device: DeviceId("fake".into()),
            output_device: DeviceId("fake".into()),
            buffer_frames: 256,
            clock: ClockRelation::SingleCallback,
            session_epoch: SessionEpoch(3),
            loopback: Some(LoopbackRoute {
                output: 0,
                input: 0,
            }),
        },
        start: Mark::new(1000, 1_790_000_000_000_000_000),
        end: None,
        limits: Limits {
            max_duration: Seconds(60.0),
            max_bytes: None,
        },
        started_by: ClientId("alice".into()),
        initial: Initial {
            measurements: st.measurements.clone(),
            generator: st.generator.clone(),
            inputs: st.inputs.clone(),
            calibrations: st.calibrations.clone(),
        },
        timeline: vec![TimelineEntry {
            at_sample: SampleIndex(1500),
            frame: 500,
            wall_ns: WallNs(1_790_000_000_010_000_000),
            change: TimelineChange::Measurement(Box::new(Patch::Set(st.measurements[0].clone()))),
        }],
        discontinuities: vec![
            Discontinuity {
                frame: 256,
                session_sample: SampleIndex(1256),
                lost_frames: 0,
                estimated: false,
                causes: vec![DiscontinuityCause::Xrun],
            },
            Discontinuity {
                frame: 512,
                session_sample: SampleIndex(2512),
                lost_frames: 1000,
                estimated: false,
                causes: vec![DiscontinuityCause::RecorderBehind],
            },
        ],
    }
}

#[test]
fn sidecar_round_trips_and_a_foreign_version_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let s = sidecar("a");
    write_sidecar(dir.path(), "a", &s).expect("write");
    assert_eq!(read_sidecar(dir.path(), "a").expect("read"), s);
    let mut v: serde_json::Value =
        serde_json::from_slice(&fs::read(sidecar_path(dir.path(), "a")).expect("read"))
            .expect("json");
    v["version"] = serde_json::json!(VERSION + 1);
    v["field_from_the_future"] = serde_json::json!(1);
    fs::write(sidecar_path(dir.path(), "a"), v.to_string()).expect("write");
    let e = read_sidecar(dir.path(), "a").expect_err("newer version");
    assert!(e.to_string().contains("version"), "{e}");
    assert!(matches!(
        read_sidecar(dir.path(), "missing"),
        Err(RawError::NotFound(_))
    ));
}

#[test]
fn file_frames_and_session_samples_map_across_gaps() {
    let s = sidecar("a");
    // Frames 0..512 are samples 1000..1512; the recorder lost 1000 samples before
    // frame 512, which is sample 2512.
    assert_eq!(session_sample_of(&s, 0), 1000);
    assert_eq!(session_sample_of(&s, 511), 1511);
    assert_eq!(session_sample_of(&s, 512), 2512);
    assert_eq!(session_sample_of(&s, 600), 2600);
    let f = |x| frame_of(1000, &s.discontinuities, x);
    assert_eq!(f(900), 0);
    assert_eq!(f(1000), 0);
    assert_eq!(f(1300), 300);
    assert_eq!(f(1511), 511);
    assert_eq!(
        f(2000),
        512,
        "a lost sample maps to the next recorded frame"
    );
    assert_eq!(f(2512), 512);
    assert_eq!(f(2600), 600);
}

#[test]
fn an_unfinished_recording_is_finished_as_interrupted() {
    let dir = tempfile::tempdir().expect("dir");
    let mut s = sidecar("cut");
    s.discontinuities.truncate(1);
    write_sidecar(dir.path(), "cut", &s).expect("write");
    let mut w = WavWriter::create(&audio_path(dir.path(), "cut"), 2, 48_000).expect("create");
    w.write(&vec![0.25; 2 * 300]).expect("write");
    // Dropped without finish: the header still says nothing was recorded.
    drop(w);
    let mut done = sidecar("done");
    done.end = Some(End {
        at: Mark::new(1010, 1),
        frames: 10,
        reason: RecordingEnd::Stopped,
    });
    write_sidecar(dir.path(), "done", &done).expect("write");

    assert!(
        recover(dir.path(), 0, std::time::Duration::from_secs(3600)).is_empty(),
        "a file written just now may still be recording"
    );
    let r = recover(
        dir.path(),
        1_790_000_000_500_000_000,
        std::time::Duration::ZERO,
    );
    assert_eq!(r.len(), 1, "only the unfinished one");
    assert_eq!(r[0].0, "cut");
    assert_eq!(r[0].1.as_ref().ok(), Some(&300));
    let back = read_sidecar(dir.path(), "cut").expect("read");
    let end = back.end.expect("ended");
    assert_eq!(end.reason, RecordingEnd::Interrupted);
    assert_eq!(end.frames, 300);
    assert_eq!(end.at.session_sample, SampleIndex(1300));
    let reader = WavReader::open(&audio_path(dir.path(), "cut")).expect("open");
    assert_eq!(reader.info().frames, 300);
    assert!(
        recover(dir.path(), 0, std::time::Duration::ZERO).is_empty(),
        "nothing left to finish"
    );

    let names: Vec<String> = list(dir.path())
        .expect("list")
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(names.len(), 2);
}

#[test]
fn paths_name_their_recording() {
    let p = Path::new("/r/show 1.ac2rec.json");
    assert_eq!(
        split_path(p),
        Some((PathBuf::from("/r"), "show 1".to_owned()))
    );
    assert_eq!(
        split_path(Path::new("/r/show.wav")),
        Some((PathBuf::from("/r"), "show".to_owned()))
    );
    assert_eq!(split_path(Path::new("/r/show.flac")), None);
    assert!(validate_name("show 1").is_ok());
    assert!(validate_name("../x").is_err());
}

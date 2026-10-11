use clap::Parser;

use super::*;
use crate::Cli;
use crate::args::{Cmd, MeasCmd};

fn config(args: &[&str]) -> Result<MeasConfig, CliError> {
    let cli = Cli::try_parse_from(["ac2", "meas", "new"].iter().chain(args))
        .map_err(|e| CliError::Usage(e.to_string()))?;
    let Cmd::Meas {
        cmd: MeasCmd::New(n),
    } = cli.cmd
    else {
        unreachable!()
    };
    meas_config(&n)
}

#[test]
fn a_sweep_takes_lf_harmonics_by_name() {
    use ac2_proto::model::LfHarmonics;
    let base = [
        "sweep", "--name", "s", "--ref", "2", "--meas", "1", "--out", "1,2", "--level", "-50dbfs",
    ];
    let lf = |extra: &[&str]| {
        let args: Vec<&str> = base.iter().chain(extra).copied().collect();
        match config(&args).map(|c| c.kind) {
            Ok(MeasKind::Sweep { config }) => Ok(config.lf_harmonics),
            Ok(other) => panic!("{other:?}"),
            Err(e) => Err(e),
        }
    };
    assert_eq!(lf(&[]).expect("parsed"), LfHarmonics::Standard);
    assert_eq!(
        lf(&["--lf-harmonics", "fine"]).expect("parsed"),
        LfHarmonics::Fine
    );
    assert_eq!(
        lf(&["--lf-harmonics", "standard"]).expect("parsed"),
        LfHarmonics::Standard
    );
    assert!(lf(&["--lf-harmonics", "on"]).is_err());
    let e = config(&[
        "tf",
        "--name",
        "t",
        "--ref",
        "2",
        "--meas",
        "1",
        "--lf-harmonics",
        "fine",
    ])
    .expect_err("refused");
    assert!(
        e.to_string().contains("--lf-harmonics applies to a sweep"),
        "{e}"
    );
}

#[test]
fn spectrum_and_rta_take_an_average_and_meas_set_changes_it() {
    use ac2_proto::units::Seconds;
    let rta = config(&[
        "rta",
        "--name",
        "r",
        "--input",
        "1",
        "--average",
        "fifo:2400",
    ])
    .expect("rta");
    let MeasKind::Rta { config: c } = &rta.kind else {
        panic!("{rta:?}");
    };
    assert_eq!(c.averaging, SpecAveraging::Fifo { frames: 2400 });
    let spec = config(&[
        "spectrum",
        "--name",
        "s",
        "--input",
        "1",
        "--average",
        "exp:2s",
    ])
    .expect("spectrum");
    let MeasKind::Spectrum { config: c } = &spec.kind else {
        panic!("{spec:?}");
    };
    assert_eq!(
        c.averaging,
        SpecAveraging::Exponential {
            time_constant: Seconds(2.0)
        }
    );
    // Off unless asked, as the UI's dialogs.
    let plain = config(&["rta", "--name", "r", "--input", "1"]).expect("rta");
    assert!(matches!(
        plain.kind,
        MeasKind::Rta { config } if config.averaging == SpecAveraging::Off
    ));
    assert!(config(&["rta", "--name", "r", "--input", "1", "--average", "fifo:0"]).is_err());
    let e = config(&[
        "tf",
        "--name",
        "t",
        "--ref",
        "2",
        "--meas",
        "1",
        "--average",
        "exp:1s",
    ])
    .expect_err("refused");
    assert!(e.to_string().contains("tf averages over --blocks"), "{e}");

    let avg = |a| MeasSet {
        average: Some(a),
        ..MeasSet::default()
    };
    let blocks = |b| MeasSet {
        blocks: Some(b),
        ..MeasSet::default()
    };
    let set = meas_set(&rta, avg(SpecAveraging::Off)).expect("set");
    assert!(matches!(
        set.kind,
        MeasKind::Rta { config } if config.averaging == SpecAveraging::Off
    ));
    assert!(meas_set(&rta, MeasSet::default()).is_err());
    let e = meas_set(&rta, blocks(4)).expect_err("refused");
    assert!(e.to_string().contains("--blocks applies to tf"), "{e}");
    let tf = config(&["tf", "--name", "t", "--ref", "2", "--meas", "1"]).expect("tf");
    let set = meas_set(&tf, blocks(32)).expect("set");
    assert!(matches!(
        set.kind,
        MeasKind::Transfer { config } if config.averaging == TfAveraging::Fifo { blocks: 32 }
    ));
    let e = meas_set(&tf, avg(SpecAveraging::Off)).expect_err("refused");
    assert!(e.to_string().contains("t is a tf measurement"), "{e}");
}

#[test]
fn meas_set_takes_a_colour_alone() {
    use ac2_proto::model::ColourFamily;
    let colour = |v: &str| -> Result<Option<ColourFamily>, String> {
        let cli = Cli::try_parse_from(["ac2", "meas", "set", "Main", "--colour", v])
            .map_err(|e| e.to_string())?;
        match cli.cmd {
            Cmd::Meas {
                cmd: MeasCmd::Set { colour, .. },
            } => Ok(colour.expect("given").0),
            _ => unreachable!(),
        }
    };
    assert_eq!(colour("1"), Ok(Some(ColourFamily::Hue { index: 0 })));
    assert_eq!(colour("8"), Ok(Some(ColourFamily::Hue { index: 7 })));
    assert_eq!(colour("grey"), Ok(Some(ColourFamily::Neutral)));
    assert_eq!(colour("auto"), Ok(None));
    for bad in ["0", "9", "red"] {
        let e = colour(bad).expect_err(bad);
        assert!(e.contains("a colour is 1 … 8, grey or auto"), "{e}");
    }
    assert!(MeasSet::default().is_empty());
}

#[test]
fn meas_set_changes_a_spectrums_fft_and_window() {
    let set_of = |args: &[&str]| -> Result<MeasSet, String> {
        let argv = [&["ac2", "meas", "set", "S"][..], args].concat();
        let cli = Cli::try_parse_from(argv).map_err(|e| e.to_string())?;
        match cli.cmd {
            Cmd::Meas {
                cmd: MeasCmd::Set { fft, window: w, .. },
            } => Ok(MeasSet {
                fft,
                window: w.map(window),
                ..MeasSet::default()
            }),
            _ => unreachable!(),
        }
    };
    let spectrum = config(&["spectrum", "--name", "S", "--input", "1"]).expect("spectrum");
    let set = set_of(&["--fft", "4096samples", "--window", "flattop"]).expect("parsed");
    assert!(!set.is_empty());
    let MeasKind::Spectrum { config: c } = meas_set(&spectrum, set).expect("set").kind else {
        panic!("a spectrum");
    };
    assert_eq!(
        (c.resolution, c.window),
        (SpecResolution::Fixed { fft_len: 4096 }, Window::FlatTop)
    );
    // The window alone keeps the length.
    let set = set_of(&["--window", "rect"]).expect("parsed");
    let MeasKind::Spectrum { config: c } = meas_set(&spectrum, set).expect("set").kind else {
        panic!("a spectrum");
    };
    assert_eq!(
        (c.resolution, c.window),
        (
            SpecResolution::Fixed { fft_len: 65_536 },
            Window::Rectangular
        )
    );
    for bad in ["1000samples", "32samples", "131072samples"] {
        let set = set_of(&["--fft", bad]).expect("parsed");
        let e = meas_set(&spectrum, set).expect_err(bad);
        assert!(e.to_string().contains("power of two, 64 … 65536"), "{e}");
    }
    let rta = config(&["rta", "--name", "R", "--input", "1"]).expect("rta");
    let e =
        meas_set(&rta, set_of(&["--fft", "1024samples"]).expect("parsed")).expect_err("refused");
    assert!(e.to_string().contains("--fft applies to spectrum"), "{e}");
    let e = meas_set(&rta, set_of(&["--window", "hann"]).expect("parsed")).expect_err("refused");
    assert!(
        e.to_string().contains("--window applies to spectrum"),
        "{e}"
    );
}

#[test]
fn tf_and_sweep_take_a_resolution_and_meas_set_changes_it() {
    let tf = |extra: &[&str]| {
        let base = ["tf", "--name", "t", "--ref", "2", "--meas", "1"];
        let args: Vec<&str> = base.iter().chain(extra).copied().collect();
        config(&args).map(|c| match c.kind {
            MeasKind::Transfer { config } => config.resolution,
            other => panic!("{other:?}"),
        })
    };
    assert_eq!(tf(&[]).expect("default"), Resolution::FortyEighth);
    assert_eq!(
        tf(&["--resolution", "1/96"]).expect("1/96"),
        Resolution::NinetySixth
    );
    assert_eq!(
        tf(&["--resolution", "12"]).expect("12"),
        Resolution::Twelfth
    );
    let e = tf(&["--resolution", "1/6"]).expect_err("refused");
    assert!(e.to_string().contains("1/12, 1/24, 1/48 or 1/96"), "{e}");
    assert!(tf(&["--ppo", "48"]).is_err());
    let sweep = config(&[
        "sweep",
        "--name",
        "s",
        "--ref",
        "2",
        "--meas",
        "1",
        "--out",
        "1,2",
        "--level",
        "-50dbfs",
        "--resolution",
        "1/24",
    ])
    .expect("sweep");
    assert!(matches!(
        &sweep.kind,
        MeasKind::Sweep { config } if config.resolution == Resolution::TwentyFourth
    ));
    let e = config(&["rta", "--name", "r", "--input", "1", "--resolution", "1/48"])
        .expect_err("refused");
    assert!(
        e.to_string()
            .contains("--resolution applies to tf, sweep and spectrum"),
        "{e}"
    );
    let e = tf(&["--resolution", "multi"]).expect_err("refused");
    assert!(e.to_string().contains("a spectrum's"), "{e}");

    let fine = MeasSet {
        resolution: Some(crate::args::ResolutionArg::grid(Resolution::NinetySixth)),
        ..MeasSet::default()
    };
    let set = meas_set(&sweep, fine).expect("set");
    assert!(matches!(
        set.kind,
        MeasKind::Sweep { config } if config.resolution == Resolution::NinetySixth
    ));
    let rta = config(&["rta", "--name", "r", "--input", "1"]).expect("rta");
    let e = meas_set(&rta, fine).expect_err("refused");
    assert!(
        e.to_string()
            .contains("--resolution applies to tf, sweep and spectrum"),
        "{e}"
    );
}

#[test]
fn spectrum_takes_multi_resolution_and_meas_set_switches_it() {
    let spec = |extra: &[&str]| {
        let base = ["spectrum", "--name", "s", "--input", "1"];
        let args: Vec<&str> = base.iter().chain(extra).copied().collect();
        config(&args).map(|c| match c.kind {
            MeasKind::Spectrum { config } => config.resolution,
            other => panic!("{other:?}"),
        })
    };
    assert_eq!(
        spec(&[]).expect("default"),
        SpecResolution::Fixed { fft_len: 65_536 }
    );
    assert_eq!(
        spec(&["--resolution", "multi"]).expect("multi"),
        SpecResolution::MULTI
    );
    assert_eq!(
        spec(&["--resolution", "multi:1/96"]).expect("multi:1/96"),
        SpecResolution::Multi {
            grid: Resolution::NinetySixth
        }
    );
    let e = spec(&["--resolution", "1/48"]).expect_err("refused");
    assert!(e.to_string().contains("--resolution multi"), "{e}");
    assert!(spec(&["--resolution", "multi:1/7"]).is_err());

    let multi = config(&["spectrum", "--name", "s", "--input", "1"]).expect("spectrum");
    let to = |r: &str| MeasSet {
        resolution: Some(r.parse().expect("arg")),
        ..MeasSet::default()
    };
    let m = meas_set(&multi, to("multi:1/24")).expect("set");
    let MeasKind::Spectrum { config: c } = &m.kind else {
        panic!("a spectrum")
    };
    assert_eq!(
        c.resolution,
        SpecResolution::Multi {
            grid: Resolution::TwentyFourth
        }
    );
    assert_eq!(
        crate::output::meas_kind(&m.kind),
        "spectrum in 1 (multi-res 1/24 oct · density)"
    );
    let back = MeasSet {
        fft: Some(SampleCount(8192)),
        ..MeasSet::default()
    };
    let MeasKind::Spectrum { config: c } = meas_set(&m, back).expect("set").kind else {
        panic!("a spectrum")
    };
    assert_eq!(c.resolution, SpecResolution::Fixed { fft_len: 8192 });
    let both = MeasSet {
        fft: Some(SampleCount(8192)),
        ..to("multi")
    };
    let e = meas_set(&m, both).expect_err("refused");
    assert!(e.to_string().contains("give one"), "{e}");
}

#[test]
fn spectrum_level_defaults_by_resolution_and_meas_set_changes_it() {
    use ac2_proto::model::SpecLevel;
    let level = |extra: &[&str]| {
        let base = ["spectrum", "--name", "s", "--input", "1"];
        let args: Vec<&str> = base.iter().chain(extra).copied().collect();
        config(&args).map(|c| match c.kind {
            MeasKind::Spectrum { config } => config.level,
            other => panic!("{other:?}"),
        })
    };
    assert_eq!(level(&[]).expect("fixed"), SpecLevel::Tone);
    assert_eq!(
        level(&["--resolution", "multi"]).expect("multi"),
        SpecLevel::Density
    );
    assert_eq!(
        level(&["--level", "density"]).expect("fixed density"),
        SpecLevel::Density
    );
    assert_eq!(
        level(&["--resolution", "multi", "--level", "tone"]).expect("multi tone"),
        SpecLevel::Tone
    );
    let e = level(&["--level", "-20dbfs"]).expect_err("refused");
    assert!(e.to_string().contains("tone|density"), "{e}");
    let e =
        config(&["rta", "--name", "r", "--input", "1", "--level", "density"]).expect_err("refused");
    assert!(e.to_string().contains("applies to a spectrum"), "{e}");

    let fixed = config(&["spectrum", "--name", "s", "--input", "1"]).expect("spectrum");
    let level_of = |m: &MeasConfig| match &m.kind {
        MeasKind::Spectrum { config } => config.level,
        other => panic!("{other:?}"),
    };
    let set = |m: &MeasConfig, s: MeasSet| meas_set(m, s).expect("set");
    let d = set(
        &fixed,
        MeasSet {
            level: Some(SpecLevel::Density),
            ..MeasSet::default()
        },
    );
    assert_eq!(level_of(&d), SpecLevel::Density);
    assert_eq!(
        crate::output::meas_kind(&d.kind),
        "spectrum in 1 (65536 pt · density)"
    );
    // Crossing fixed↔multi without a level takes the new resolution's default.
    let multi = MeasSet {
        resolution: Some("multi".parse().expect("arg")),
        ..MeasSet::default()
    };
    let m = set(&fixed, multi);
    assert_eq!(level_of(&m), SpecLevel::Density);
    let back = set(
        &m,
        MeasSet {
            fft: Some(SampleCount(8192)),
            ..MeasSet::default()
        },
    );
    assert_eq!(level_of(&back), SpecLevel::Tone);
    // An explicit level wins over the default.
    let m = set(
        &fixed,
        MeasSet {
            level: Some(SpecLevel::Tone),
            ..multi
        },
    );
    assert_eq!(level_of(&m), SpecLevel::Tone);
    assert_eq!(
        crate::output::meas_kind(&m.kind),
        "spectrum in 1 (multi-res 1/48 oct · tone)"
    );
    // A grid change within multi-resolution keeps the level chosen.
    let m = set(
        &m,
        MeasSet {
            resolution: Some("multi:1/24".parse().expect("arg")),
            ..MeasSet::default()
        },
    );
    assert_eq!(level_of(&m), SpecLevel::Tone);
}

#[test]
fn smoothing_flags() {
    let tf = |extra: &[&str]| {
        let mut a = vec!["tf", "--ref", "1", "--meas", "2", "--name", "x"];
        a.extend_from_slice(extra);
        match config(&a).map(|c| c.kind) {
            Ok(MeasKind::Transfer { config }) => Ok(config.smoothing),
            Ok(other) => panic!("{other:?}"),
            Err(e) => Err(e),
        }
    };
    // Phase is smoothed with the magnitude unless asked not to.
    assert_eq!(
        tf(&["--smooth", "6"]).ok(),
        Some(Some(Smoothing {
            fraction: SmoothingFraction::Sixth,
            mode: SmoothingMode::MagnitudePhase
        }))
    );
    assert_eq!(
        tf(&["--smooth", "12", "--smooth-magnitude-only"]).ok(),
        Some(Some(Smoothing {
            fraction: SmoothingFraction::Twelfth,
            mode: SmoothingMode::Magnitude
        }))
    );
    assert!(tf(&["--smooth-magnitude-only"]).is_err(), "needs --smooth");
    assert_eq!(tf(&[]).ok(), Some(None));

    let spec = config(&["spectrum", "--input", "2", "--name", "s", "--smooth", "3"]);
    match spec.map(|c| c.kind) {
        Ok(MeasKind::Spectrum { config }) => {
            assert_eq!(config.smoothing, Some(SmoothingFraction::Third));
        }
        other => panic!("{other:?}"),
    }
    let e = config(&["rta", "--input", "2", "--name", "r", "--smooth", "3"]);
    assert!(
        matches!(&e, Err(CliError::Usage(m)) if m.contains("already are fractional-octave")),
        "{e:?}"
    );
    assert!(config(&["spl", "--input", "2", "--name", "s", "--smooth", "3"]).is_err());
    assert!(
        config(&[
            "spectrum",
            "--input",
            "2",
            "--name",
            "s",
            "--smooth",
            "3",
            "--smooth-magnitude-only"
        ])
        .is_err()
    );
}

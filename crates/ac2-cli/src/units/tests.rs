use super::*;

#[test]
fn spectrum_averaging() {
    use ac2_proto::model::SpecAveraging;
    assert_eq!(ok::<SpecAveragingArg>("off").0, SpecAveraging::Off);
    assert_eq!(
        ok::<SpecAveragingArg>("FIFO:16").0,
        SpecAveraging::Fifo { frames: 16 }
    );
    assert_eq!(
        ok::<SpecAveragingArg>("exp:500ms").0,
        SpecAveraging::Exponential {
            time_constant: Seconds(0.5)
        }
    );
    assert!(bad::<SpecAveragingArg>("fifo:0").contains("≥ 1"));
    assert!(bad::<SpecAveragingArg>("fifo:1.5").contains("whole number"));
    assert!(bad::<SpecAveragingArg>("exp:0s").contains("longer than 0"));
    assert!(bad::<SpecAveragingArg>("exp:2").contains("missing unit"));
    assert!(bad::<SpecAveragingArg>("2s").contains("expected off"));
}

#[test]
fn bands_and_band_limits() {
    assert_eq!(ok::<BandNominalArg>("63hz"), BandNominalArg(5));
    assert_eq!(ok::<BandNominalArg>("1khz"), BandNominalArg(17));
    assert_eq!(ok::<BandNominalArg>("31.5hz"), BandNominalArg(2));
    assert!(bad::<BandNominalArg>("64hz").contains("not a 1/3-octave band"));
    assert_eq!(
        ok::<BandRangeArg>("20hz..200hz"),
        BandRangeArg { from: 0, to: 10 }
    );
    assert_eq!(
        ok::<BandRangeArg>("1khz"),
        BandRangeArg { from: 17, to: 17 }
    );
    assert!(bad::<BandRangeArg>("200hz..20hz").contains("low to high"));
    let w: BandWindowArg = ok("a:15min@1khz");
    assert_eq!((w.window.seconds, w.bands()), (900, 17..=17));
    let w: BandWindowArg = ok("z:60min@20hz..200hz");
    assert_eq!(
        (w.window.weighting, w.window.seconds, w.bands()),
        (Some(ac2_proto::model::Weighting::Z), 3600, 0..=10)
    );
    assert!(bad::<BandWindowArg>("z:60min").contains("names its band after @"));
    let l: BandLimitArg = ok("z:60min@63hz=42db");
    assert_eq!(
        (l.window.window.seconds, l.window.bands(), l.limit),
        (3600, 5..=5, Some(DbSpl(42.0)))
    );
    assert_eq!(ok::<BandLimitArg>("15min@1khz=none").limit, None);
    assert!(bad::<BandLimitArg>("z:60min@63hz").contains("window=limit"));
    let d: DayOffsetArg = ok("z:60min@20hz..200hz=5db");
    assert_eq!((d.window.bands(), d.offset), (0..=10, Some(Db(5.0))));
    assert_eq!(ok::<DayOffsetArg>("60min@63hz=none").offset, None);
    assert!(bad::<DayOffsetArg>("60min@63hz=40db").contains("day offset"));
}

#[test]
fn peak_limits_and_positions() {
    use ac2_proto::model::PeakQuantity;
    let p: PeakLimitArg = ok("lcpeak=135db");
    assert_eq!(
        (p.quantity, p.limit),
        (PeakQuantity::LcPeak, Some(DbSpl(135.0)))
    );
    let p: PeakLimitArg = ok("LAFmax=none");
    assert_eq!((p.quantity, p.limit), (PeakQuantity::LafMax, None));
    for bad in ["lzpeak=130db", "lcpeak", "lcpeak=135"] {
        assert!(bad.parse::<PeakLimitArg>().is_err(), "{bad}");
    }
    assert_eq!(ok::<PositionArg>("4db").0, Some(Db(4.0)));
    assert_eq!(ok::<PositionArg>("-2.5 dB").0, Some(Db(-2.5)));
    assert_eq!(ok::<PositionArg>("none").0, None);
    for bad in ["4", "31db", "4dbfs"] {
        assert!(bad.parse::<PositionArg>().is_err(), "{bad}");
    }
}

fn ok<T: FromStr<Err = UnitError>>(s: &str) -> T {
    match s.parse::<T>() {
        Ok(v) => v,
        Err(e) => panic!("{s}: {e}"),
    }
}

fn bad<T: FromStr<Err = UnitError> + std::fmt::Debug>(s: &str) -> String {
    match s.parse::<T>() {
        Ok(v) => panic!("{s}: parsed as {v:?}"),
        Err(e) => e.0,
    }
}

#[test]
fn frequencies() {
    assert_eq!(ok::<Freq>("20hz").0, Hz(20.0));
    assert_eq!(ok::<Freq>("1.5kHz").0, Hz(1500.0));
    assert_eq!(ok::<Freq>("2k").0, Hz(2000.0));
    assert_eq!(ok::<Freq>(" 31.5 Hz ").0, Hz(31.5));
    assert_eq!(ok::<Freq>("1e3hz").0, Hz(1000.0));
    for bad in [
        "20", "hz", "0hz", "-1hz", "2mhz", "infhz", "nanhz", "1.2.3hz", "", "2e9hz",
    ] {
        assert!(bad.parse::<Freq>().is_err(), "{bad}");
    }
}

#[test]
fn volts_and_sensitivities() {
    assert_eq!(ok::<VoltsArg>("15.0mV").0, Volts(0.015));
    assert_eq!(ok::<VoltsArg>("0.0150v").0, Volts(0.015));
    assert!((ok::<VoltsArg>("250uV").0.0 - 250e-6).abs() < 1e-15);
    assert_eq!(ok::<VoltsArg>("1.228 V").0, Volts(1.228));
    for bad in ["0.015", "15", "0mv", "-1mv", "2000v", "15ma"] {
        assert!(bad.parse::<VoltsArg>().is_err(), "{bad}");
    }
    assert_eq!(ok::<MicSensitivityArg>("15.0mV/Pa").0, MvPerPa(15.0));
    assert_eq!(ok::<MicSensitivityArg>("0.05V/Pa").0, MvPerPa(50.0));
    let db = ok::<MicSensitivityArg>("-36.5dBV/Pa").0.0;
    assert!((db - 14.962).abs() < 1e-3, "{db}");
    for bad in ["15", "15mv", "0mv/pa", "15pa"] {
        assert!(bad.parse::<MicSensitivityArg>().is_err(), "{bad}");
    }
}

#[test]
fn levels() {
    assert_eq!(ok::<LevelDbfs>("-20dbfs").0, Dbfs(-20.0));
    assert_eq!(ok::<LevelDbfs>("-12.5dBFS").0, Dbfs(-12.5));
    assert_eq!(ok::<LevelDbfs>("0dbfs").0, Dbfs(0.0));
    for bad in ["-20", "-20db", "3dbfs", "-200dbfs", "dbfs", "--3dbfs"] {
        assert!(bad.parse::<LevelDbfs>().is_err(), "{bad}");
    }
    assert_eq!(ok::<Gain>("-6db").0, Db(-6.0));
    assert_eq!(ok::<Gain>("+3dB").0, Db(3.0));
    assert!("3dbfs".parse::<Gain>().is_err());
    assert_eq!(ok::<SplLevel>("94db").0, DbSpl(94.0));
    assert_eq!(ok::<SplLevel>("114dBSPL").0, DbSpl(114.0));
    assert!("-3db".parse::<SplLevel>().is_err());
}

#[test]
fn times_samples_distances() {
    assert_eq!(ok::<Time>("1.5ms").0, Seconds(0.0015));
    assert_eq!(ok::<Time>("2s").0, Seconds(2.0));
    assert_eq!(ok::<Time>("250us").0, Seconds(0.000_25));
    assert!("-1s".parse::<Time>().is_err());
    assert!("1".parse::<Time>().is_err());
    assert_eq!(ok::<SampleCount>("480samples").0, 480);
    assert_eq!(ok::<SampleCount>("-12smp").0, -12);
    assert!("1.5samples".parse::<SampleCount>().is_err());
    assert!("9e99samples".parse::<SampleCount>().is_err());
    assert_eq!(ok::<Distance>("120cm").0, 1.2);
    assert!("-1m".parse::<Distance>().is_err());
    assert_eq!(ok::<Celsius>("20c").0, 20.0);
    assert_eq!(ok::<Celsius>("-5°C").0, -5.0);
    assert!("300c".parse::<Celsius>().is_err());
    assert!("20f".parse::<Celsius>().is_err());
}

#[test]
fn delay_amounts() {
    let c20 = Celsius(20.0);
    assert_eq!(
        ok::<DelayAmount>("12.5ms").seconds(None, c20),
        Ok(Seconds(0.0125))
    );
    assert_eq!(
        ok::<DelayAmount>("480samples").seconds(Some(48_000), c20),
        Ok(Seconds(0.01))
    );
    assert!(ok::<DelayAmount>("480samples").seconds(None, c20).is_err());
    assert_eq!(
        ok::<DelayAmount>("-0.25samples").seconds(Some(48_000), c20),
        Ok(Seconds(-0.25 / 48_000.0))
    );
    let d = ok::<DelayAmount>("3.432m")
        .seconds(None, c20)
        .unwrap_or(Seconds(0.0));
    // 343.2 m/s at 20 °C.
    assert!((d.0 - 0.01).abs() < 2e-5, "{}", d.0);
    assert_eq!(
        ok::<DelayAmount>("-2ms"),
        DelayAmount::Time(Seconds(-0.002))
    );
    assert!("11s".parse::<DelayAmount>().is_err());
    assert!("3".parse::<DelayAmount>().is_err());
}

#[test]
fn channel_lists() {
    assert_eq!(ok::<Channels>("1,2").0, vec![0, 1]);
    assert_eq!(ok::<Channels>("1,4-6").0, vec![0, 3, 4, 5]);
    assert_eq!(ok::<Channel>("3").0, 2);
    assert_eq!(Channel(2).to_string(), "3");
    assert_eq!(channels_text(&[0, 3]), "1,4");
    for bad in ["0", "1,,2", "3-1", "1,1", "a", "", "257", "1-", "-2"] {
        assert!(bad.parse::<Channels>().is_err(), "{bad}");
    }
}

#[test]
fn leq_windows_and_limits() {
    use ac2_proto::model::Weighting;
    let w: LeqWindowArg = ok("30min");
    assert_eq!((w.weighting(), w.seconds), (Weighting::A, 1800));
    let w: LeqWindowArg = ok("C:10s");
    assert_eq!((w.weighting, w.seconds), (Some(Weighting::C), 10));
    assert_eq!(ok::<LeqWindowArg>("1h").seconds, 3600);
    for bad in ["30", "0.5s", "2d", "x:1min", "25h", "1.5s"] {
        assert!(bad.parse::<LeqWindowArg>().is_err(), "{bad}");
    }
    let l: LeqLimitArg = ok("30min=99db");
    assert_eq!(l.limit, Some(DbSpl(99.0)));
    assert_eq!(l.window.seconds, 1800);
    let l: LeqLimitArg = ok("c:60min=None");
    assert_eq!(l.limit, None);
    for bad in ["30min", "30min=99", "=99db"] {
        assert!(bad.parse::<LeqLimitArg>().is_err(), "{bad}");
    }
}

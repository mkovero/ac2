#![allow(clippy::unwrap_used)]

use super::*;
use crate::eq::response;
use num_complex::Complex64;

const FS: f64 = 48_000.0;

fn digital() -> EqRealisation {
    EqRealisation::Digital { rate: Hz(FS) }
}

fn body(sections: Vec<EqSection>) -> EqBody {
    EqBody {
        name: "EQ 1".into(),
        realisation: digital(),
        peak_q: PeakQ::HalfGain,
        enabled: true,
        sections,
    }
}

fn on(shape: EqShape) -> EqSection {
    EqSection {
        enabled: true,
        shape,
    }
}

fn h(b: &EqBody, freqs: &[f64]) -> Vec<Complex64> {
    response(b, freqs, &|_| Err("no traces".into())).unwrap()
}

fn log_grid() -> Vec<f64> {
    (0..=480)
        .map(|k| 20.0 * 2f64.powf(k as f64 / 48.0))
        .collect()
}

fn db(v: Complex64) -> f64 {
    20.0 * v.norm().log10()
}

fn wrap(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

/// REW's Generic equaliser filter types as APO lines.
fn rew_line(f: &serde_json::Value) -> String {
    let ty = f["type"].as_str().unwrap();
    let fc = f["frequency"].as_f64().unwrap();
    let gain = f["gaindB"].as_f64();
    let q = f["q"].as_f64();
    let ty = match ty {
        "All pass" => "AP",
        "Notch" => "NO",
        t => t,
    };
    let mut l = format!("Filter: ON {ty} Fc {fc} Hz");
    if let Some(g) = gain {
        l += &format!(" Gain {g} dB");
    }
    if let Some(q) = q {
        l += &format!(" Q {q}");
    }
    l
}

fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("../../../tests/fixtures/rew_generic_eq.json")).unwrap()
}

/// Every REW Generic filter type read as an APO line predicts what REW predicts for it,
/// 20 Hz – 20 kHz (REW rounds to 1e-4 dB).
#[test]
fn reads_like_rew_generic() {
    let fx = fixture();
    for f in fx["filters"].as_array().unwrap() {
        let line = rew_line(f);
        let secs = read(&line, digital()).unwrap_or_else(|e| panic!("{line}: {e}"));
        let start = f["start_hz"].as_f64().unwrap();
        let ppo = f["ppo"].as_f64().unwrap();
        let mag = f["mag_db"].as_array().unwrap();
        let ph = f["phase_deg"].as_array().unwrap();
        let freqs: Vec<f64> = (0..mag.len())
            .map(|k| start * 2f64.powf(k as f64 / ppo))
            .collect();
        let ours = h(&body(secs), &freqs);
        let notch = line.contains(" NO ");
        let (mut worst_db, mut worst_deg) = (0f64, 0f64);
        for (k, &fr) in freqs.iter().enumerate() {
            if !(20.0..=20_000.0).contains(&fr) {
                continue;
            }
            let m = mag[k].as_f64().unwrap();
            // A low-pass's stopband near Nyquist is where REW's own evaluation drifts
            // from the exact cookbook section (0.05 dB at −54 dB).
            // REW resamples its response onto the log grid, which blurs a Q 30 notch; the
            // fine linear grid (`notch_default_q_like_rew`) checks it.
            if m < -40.0 || (notch && (fr - 60.0).abs() < 6.0) {
                continue;
            }
            worst_db = worst_db.max((db(ours[k]) - m).abs());
            worst_deg =
                worst_deg.max(wrap(ours[k].arg().to_degrees() - ph[k].as_f64().unwrap()).abs());
        }
        assert!(worst_db < 0.02, "{line}: {worst_db} dB");
        assert!(worst_deg < 0.5, "{line}: {worst_deg}°");
    }
}

/// The default notch Q against REW's notch on a fine linear grid around 60 Hz.
#[test]
fn notch_default_q_like_rew() {
    let fx = fixture();
    let n = &fx["notch_linear"];
    let start = n["start_hz"].as_f64().unwrap();
    let step = n["step_hz"].as_f64().unwrap();
    let first = n["first_index"].as_f64().unwrap();
    let mag = n["mag_db"].as_array().unwrap();
    let freqs: Vec<f64> = (0..mag.len())
        .map(|k| start + step * (first + k as f64))
        .collect();
    let ours = h(
        &body(read("Filter: ON NO Fc 60 Hz", digital()).unwrap()),
        &freqs,
    );
    for (k, v) in mag.iter().enumerate() {
        let m = v.as_f64().unwrap();
        if m > -40.0 {
            assert!(
                (db(ours[k]) - m).abs() < 0.02,
                "{} Hz: {} vs {m}",
                freqs[k],
                db(ours[k])
            );
        }
    }
}

/// A file as Equalizer APO's configuration reference writes it.
#[test]
fn reads_apo_config() {
    let text = "\
# from the Equalizer APO configuration reference
Preamp: -6 dB
Preamp: 1.5 dB
Filter: ON PK Fc 50 Hz Gain -3.0 dB Q 2.50
Filter: OFF LPQ Fc 15000 Hz Q 0.5
Filter: ON HP Fc 30 Hz
Filter: ON LSC Fc 300 Hz Gain 5.0 dB Q 0.9
Filter: ON HS Fc 8000 Hz Gain -2 dB
Filter: ON NO Fc 60 Hz
Filter: ON AP Fc 1000 Hz Q 0.7
Delay: 2.5 ms
";
    let s = read(text, digital()).unwrap();
    assert_eq!(s[0], on(EqShape::Gain { gain: Db(-4.5) }));
    assert_eq!(
        s[1],
        on(EqShape::Peaking {
            f: Hz(50.0),
            gain: Db(-3.0),
            bw: Bandwidth::Q { value: 2.5 },
            q_measured: Some(PeakQ::HalfGain),
        })
    );
    assert_eq!(
        s[2],
        EqSection {
            enabled: false,
            shape: EqShape::LowPassQ {
                f: Hz(15000.0),
                q: 0.5
            }
        }
    );
    assert_eq!(
        s[3].shape,
        EqShape::HighPassQ {
            f: Hz(30.0),
            q: FRAC_1_SQRT_2
        }
    );
    assert!(matches!(
        s[4].shape,
        EqShape::LowShelf {
            order: ShelfOrder::Second {
                slope: ShelfSlope::Q { value: 0.9 }
            },
            f_at: ShelfFreq::Midpoint,
            ..
        }
    ));
    assert!(matches!(
        s[5].shape,
        EqShape::HighShelf {
            order: ShelfOrder::Second {
                slope: ShelfSlope::S { value: 1.0 }
            },
            ..
        }
    ));
    assert_eq!(
        s[6].shape,
        EqShape::Notch {
            f: Hz(60.0),
            bw: Bandwidth::Q { value: NOTCH_Q }
        }
    );
    assert_eq!(
        s[7].shape,
        EqShape::AllPass2 {
            f: Hz(1000.0),
            q: 0.7
        }
    );
    assert_eq!(
        s[8].shape,
        EqShape::Delay {
            seconds: Seconds(0.0025)
        }
    );
    assert_eq!(s.len(), 9);
}

/// REW's "export filter settings as text" layout: free-form header lines up to the first
/// filter, numbered filter lines, empty `None` slots.
#[test]
fn reads_rew_layout() {
    let text = "\
Filter Settings file

Room EQ V5.40
Dated: 20-Oct-2026 12:00:00

Notes:Main L

Equaliser: Generic
Main L 54 cm
Filter  1: ON  PK       Fc   1000 Hz  Gain  -6.0 dB  Q  2.000
Filter  2: ON  None
Filter  3: OFF LS 6dB   Fc  200.0 Hz  Gain   6.0 dB
";
    let s = read(text, digital()).unwrap();
    assert_eq!(s.len(), 2);
    assert!(matches!(s[0].shape, EqShape::Peaking { .. }));
    assert!(!s[1].enabled);
    let EqShape::LowShelf {
        f,
        order: ShelfOrder::First,
        ..
    } = s[1].shape
    else {
        panic!("{:?}", s[1]);
    };
    // The plateau-side corner below the midpoint by √K on tan(πf/fs).
    let want = FS / PI * ((PI * 200.0 / FS).tan() * 10f64.powf(6.0 / 40.0)).atan();
    assert!((f.0 - want).abs() < 1e-9, "{}", f.0);
    assert!(is_apo(text));
    assert!(is_apo("Preamp: -3 dB"));
    assert!(!is_apo("# ac2 eq 1\nname = EQ 1"));
}

#[test]
fn refusals_name_the_line() {
    let cases = [
        (
            "Filter: ON PK Fc 1000 Hz Gain -3 dB Q 2\nChannel: L",
            2,
            "\"Channel\" is not read",
        ),
        ("Include: other.txt", 1, "\"Include\" is not read"),
        (
            "Filter: ON PK Fc 1000 Hz Gain -3 dB BW Oct 1",
            1,
            "`BW Oct` is not read",
        ),
        ("Filter: ON LSC 12 dB Fc 100 Hz Gain 3 dB", 1, "slope form"),
        ("Filter: ON BP Fc 100 Hz", 1, "BP needs Q"),
        (
            "Filter: ON Modal Fc 40 Hz Gain -6 dB Q 5",
            1,
            "`Modal` is not read",
        ),
        ("Filter: ON XX Fc 40 Hz", 1, "unknown filter type"),
        ("Delay: 48 samples", 1, "in samples is not read"),
        ("\n\nFilter: PK Fc 1 Hz", 3, "ON or OFF"),
        ("Equaliser: miniDSP", 1, "equaliser \"miniDSP\""),
        ("Filter: ON PK Fc 1000 Hz Gain -3 dB", 1, "PK needs Q"),
        ("Filter: ON LP Fc 1000 Hz Gain 3 dB", 1, "takes no Gain"),
    ];
    for (text, line, why) in cases {
        let e = read(text, digital()).unwrap_err();
        assert_eq!(e.line, line, "{text}: {e}");
        assert!(e.reason.contains(why), "{text}: {e}");
    }
}

fn expressible() -> EqBody {
    body(vec![
        on(EqShape::Gain { gain: Db(-3.0) }),
        on(EqShape::Peaking {
            f: Hz(1250.0),
            gain: Db(-4.0),
            bw: Bandwidth::Q { value: 2.0 },
            q_measured: None,
        }),
        on(EqShape::Peaking {
            f: Hz(300.0),
            gain: Db(8.0),
            bw: Bandwidth::Octaves { n: 0.7 },
            q_measured: Some(PeakQ::Minus3dbFromPeak),
        }),
        on(EqShape::HighPass {
            f: Hz(80.0),
            alignment: Alignment::LinkwitzRiley,
            order: 4,
        }),
        on(EqShape::LowPass {
            f: Hz(5000.0),
            alignment: Alignment::Butterworth,
            order: 6,
        }),
        on(EqShape::LowShelf {
            f: Hz(120.0),
            gain: Db(6.0),
            order: ShelfOrder::Second {
                slope: ShelfSlope::S { value: 0.8 },
            },
            f_at: ShelfFreq::Corner,
        }),
        on(EqShape::HighShelf {
            f: Hz(4000.0),
            gain: Db(-5.0),
            order: ShelfOrder::First,
            f_at: ShelfFreq::Midpoint,
        }),
        EqSection {
            enabled: false,
            shape: EqShape::Notch {
                f: Hz(60.0),
                bw: Bandwidth::Q { value: 10.0 },
            },
        },
        on(EqShape::BandPass {
            f: Hz(700.0),
            bw: Bandwidth::Q { value: 1.0 },
        }),
        on(EqShape::AllPass2 {
            f: Hz(900.0),
            q: 0.6,
        }),
        on(EqShape::HighPassQ {
            f: Hz(25.0),
            q: 0.9,
        }),
        on(EqShape::Delay {
            seconds: Seconds(0.004),
        }),
    ])
}

/// Export writes the same filter: the read-back cascade predicts the set exactly, and a
/// second round trip writes the same text.
#[test]
fn export_import_round_trip() {
    let b = expressible();
    let text = write(&b, false).unwrap();
    assert!(text.contains("Preamp: -3 dB"), "{text}");
    assert!(text.contains("Delay: 4 ms"), "{text}");
    assert!(text.contains("Filter: OFF NO Fc 60 Hz Q 10"), "{text}");
    assert_eq!(text.matches("HPQ Fc 80 Hz Q 0.7071").count(), 2, "{text}");
    assert_eq!(text.matches("LPQ Fc 5000 Hz").count(), 3, "{text}");
    let back = body(read(&text, digital()).unwrap());
    let freqs = log_grid();
    for (a, c) in h(&b, &freqs).iter().zip(h(&back, &freqs)) {
        assert!((db(*a) - db(c)).abs() < 1e-9 && wrap((a / c).arg().to_degrees()).abs() < 1e-6);
    }
    let again = write(&back, false).unwrap();
    let back2 = body(read(&again, digital()).unwrap());
    for (a, c) in h(&back, &freqs).iter().zip(h(&back2, &freqs)) {
        assert!((db(*a) - db(c)).abs() < 1e-9);
    }
}

#[test]
fn export_refusals_named_and_skipped() {
    let mut b = expressible();
    b.sections.push(on(EqShape::HighPass {
        f: Hz(40.0),
        alignment: Alignment::Butterworth,
        order: 3,
    }));
    b.sections.push(on(EqShape::Polarity));
    b.sections.push(on(EqShape::AllPass1 { f: Hz(100.0) }));
    b.sections.push(on(EqShape::Peaking {
        f: Hz(100.0),
        gain: Db(3.0),
        bw: Bandwidth::Q { value: 1.0 },
        q_measured: Some(PeakQ::BandPass),
    }));
    let e = write(&b, false).unwrap_err();
    let words: Vec<String> = e.iter().map(ToString::to_string).collect();
    assert_eq!(
        words,
        [
            "13 · high-pass: an odd-order section has no APO line",
            "14 · polarity: polarity is a channel operation in APO",
            "15 · all-pass 1st order: APO has no first-order all-pass line",
            "16 · peaking: a band-pass reading of Q is another filter family than PK",
        ]
    );
    let text = write(&b, true).unwrap();
    for w in &words {
        assert!(text.contains(&format!("# skipped {w}")), "{text}");
    }
    // The skipped list is at the top, before any command.
    assert!(text.find("# skipped").unwrap() < text.find("Preamp").unwrap());
    assert_eq!(
        read(&text, digital()).unwrap().len(),
        read(&write(&expressible(), false).unwrap(), digital())
            .unwrap()
            .len()
    );

    let mut analog = expressible();
    analog.realisation = EqRealisation::Analog;
    let e = write(&analog, true).unwrap_err();
    assert_eq!(
        e[0].to_string(),
        "set: an analog set has no APO form (APO filters are digital)"
    );
}

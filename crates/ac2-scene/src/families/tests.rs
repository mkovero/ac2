use super::*;
use crate::meas_list::tests::{captured, run, sweep_config, tf, trace, with_kind};
use ac2_proto::model::{MathConfig, MathDomain, MathExpr, MathOp, Operand};

fn all() -> [Theme; 3] {
    [Theme::dark(), Theme::light(), Theme::high_contrast()]
}

fn dist(a: Color, b: Color) -> f64 {
    let (a, b) = (oklab(a), oklab(b));
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt()
}

/// Hue difference in degrees, 0…180.
fn hue_gap(a: Color, b: Color) -> f64 {
    let d = (oklch(a).2 - oklch(b).2).to_degrees().rem_euclid(360.0);
    d.min(360.0 - d)
}

/// Two colours OKLab this far apart read as different curves side by side.
const DISTINCT: f64 = 0.08;
/// Shades of one family stay within this hue of its base (lightness moves, hue does not;
/// fitting into sRGB and 8-bit rounding move it a little).
const SAME_HUE_DEG: f64 = 8.0;

#[test]
fn every_family_has_shades_that_stand_out_from_the_plot_and_each_other() {
    for t in all() {
        for base in t.families.iter().chain([&t.neutral]) {
            let f = Family::of(&t, *base);
            assert!(f.shades.len() >= MAX_SHADES, "{:?} {base:?}: {f:?}", t.name);
            let all: Vec<Color> = std::iter::once(f.base).chain(f.shades.clone()).collect();
            for (i, a) in all.iter().enumerate() {
                let r = contrast_ratio(*a, t.plot_background);
                assert!(r >= 3.0, "{:?} {a:?}: contrast {r:.2}", t.name);
                if oklch(*base).1 > GREY_CHROMA {
                    let g = hue_gap(*a, *base);
                    assert!(g <= SAME_HUE_DEG, "{:?} {a:?}: hue off by {g:.1}°", t.name);
                }
                for b in &all[i + 1..] {
                    let d = dist(*a, *b);
                    assert!(d >= DISTINCT, "{:?} {a:?} {b:?}: {d:.3}", t.name);
                }
            }
        }
    }
}

#[test]
fn family_bases_are_far_apart_in_hue() {
    for t in all() {
        for (i, a) in t.families.iter().enumerate() {
            assert!(oklch(*a).1 > 0.06, "{:?} family {i} has a hue", t.name);
            assert!(
                oklch(t.neutral).1 < GREY_CHROMA,
                "{:?} neutral is grey",
                t.name
            );
            for (j, b) in t.families.iter().enumerate().skip(i + 1) {
                let g = hue_gap(*a, *b);
                assert!(g >= 25.0, "{:?} families {i} {j}: {g:.0}°", t.name);
                let d = dist(*a, *b);
                assert!(d >= 0.1, "{:?} families {i} {j}: {d:.3}", t.name);
            }
        }
    }
}

/// Machado, Oliveira & Fernandes (2009) full-severity simulation matrices, on linear RGB.
const PROTAN: [[f64; 3]; 3] = [
    [0.152_286, 1.052_583, -0.204_868],
    [0.114_503, 0.786_281, 0.099_216],
    [-0.003_882, -0.048_116, 1.051_998],
];
const DEUTAN: [[f64; 3]; 3] = [
    [0.367_322, 0.860_646, -0.227_968],
    [0.280_085, 0.672_501, 0.047_413],
    [-0.011_820, 0.042_940, 0.968_881],
];

fn simulate(c: Color, m: &[[f64; 3]; 3]) -> Color {
    let lin = [c.r, c.g, c.b].map(|v| to_linear(f64::from(v)));
    let out: Vec<f32> = m
        .iter()
        .map(|row| {
            let v = row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2];
            to_encoded(v.clamp(0.0, 1.0)) as f32
        })
        .collect();
    Color::rgb(out[0], out[1], out[2])
}

/// The first six families — the first six measurements — stay apart for protan and deutan
/// viewers, the common colour-vision deficiencies.
#[test]
fn the_first_six_families_survive_red_green_colour_blindness() {
    for t in all() {
        for (what, m) in [("protan", &PROTAN), ("deutan", &DEUTAN)] {
            for i in 0..6 {
                for j in i + 1..6 {
                    let d = dist(simulate(t.families[i], m), simulate(t.families[j], m));
                    assert!(d >= 0.06, "{:?} {what} {i} {j}: {d:.3}", t.name);
                }
            }
        }
    }
}

#[test]
fn families_follow_the_measurement_id_and_survive_a_deletion() {
    let ms: Vec<Measurement> = (1..=4).map(|i| tf(i, &format!("m{i}"))).collect();
    let refs: Vec<&Measurement> = ms.iter().collect();
    let all4 = family_indices(&refs);
    assert_eq!(all4.values().copied().collect::<Vec<_>>(), [0, 1, 2, 3]);
    // Deleting m2 repaints nobody.
    let without: Vec<&Measurement> = ms.iter().filter(|m| m.id != MeasId(2)).collect();
    let after = family_indices(&without);
    for (id, i) in &after {
        assert_eq!(all4[id], *i, "{id:?}");
    }
    // A ninth measurement whose preferred family (that of m1) is taken takes a free one.
    let ninth = tf(9, "m9");
    let mut with9 = without.clone();
    with9.push(&ninth);
    assert_eq!(family_indices(&with9)[&MeasId(9)], 1, "m2's family is free");
    // Past eight measurements, families repeat.
    let many: Vec<Measurement> = (1..=10).map(|i| tf(i, "m")).collect();
    let refs: Vec<&Measurement> = many.iter().collect();
    let idx = family_indices(&refs);
    assert_eq!(idx[&MeasId(9)], 0);
    assert_eq!(idx[&MeasId(10)], 1);
}

#[test]
fn a_measurements_curves_share_its_hue_and_differ_from_each_other() {
    let main = TraceOwner::Meas { meas: MeasId(1) };
    let sub = TraceOwner::Meas { meas: MeasId(2) };
    let math = with_kind(
        5,
        "pre ÷ post",
        MeasKind::Math {
            config: MathConfig::of(
                main,
                MathDomain::Transfer,
                MathExpr::Binary {
                    a: Operand::Trace { trace: TraceId(3) },
                    op: MathOp::Divide,
                    b: Operand::Trace { trace: TraceId(4) },
                },
            ),
        },
        true,
    );
    let ms = [tf(1, "Main L"), tf(2, "Sub"), math];
    let refs: Vec<&Measurement> = ms.iter().collect();
    let traces = [
        trace(3, "pre-EQ", main, captured()),
        trace(4, "post-EQ", main, captured()),
        trace(6, "on Sub", sub, captured()),
        trace(7, "import", TraceOwner::Imported, captured()),
    ];
    let trefs: Vec<&TraceMeta> = traces.iter().collect();
    for theme in all() {
        let c = curve_colours(&theme, &refs, &trefs);
        let main_curves = [
            c.meas(MeasId(1)),
            c.trace(TraceId(3)),
            c.trace(TraceId(4)),
            c.meas(MeasId(5)),
        ];
        assert_eq!(
            main_curves[0], theme.families[0],
            "the live curve is the base"
        );
        assert_eq!(c.meas(MeasId(2)), theme.families[1]);
        for (i, a) in main_curves.iter().enumerate() {
            assert!(hue_gap(*a, theme.families[0]) <= SAME_HUE_DEG, "{a:?}");
            for b in &main_curves[i + 1..] {
                assert!(dist(*a, *b) >= DISTINCT, "{:?} {a:?} {b:?}", theme.name);
            }
        }
        let sub_trace = c.trace(TraceId(6));
        assert!(hue_gap(sub_trace, theme.families[1]) <= SAME_HUE_DEG);
        assert_ne!(sub_trace, c.meas(MeasId(2)));
        // Imported: the neutral family's first shade (the base is no live curve's, so the
        // first member takes it).
        assert_eq!(c.trace(TraceId(7)), theme.neutral);

        // Moved under Sub, a capture takes Sub's family.
        let mut moved = traces.clone();
        moved[1].edit.owner = sub;
        let mrefs: Vec<&TraceMeta> = moved.iter().collect();
        let m = curve_colours(&theme, &refs, &mrefs);
        let post = m.trace(TraceId(4));
        assert!(hue_gap(post, theme.families[1]) <= SAME_HUE_DEG, "{post:?}");
        assert_ne!(post, m.trace(TraceId(6)));
        assert_ne!(post, m.meas(MeasId(2)));
        // An import moved under Main L leaves the neutral family.
        let mut adopted = traces.clone();
        adopted[3].edit.owner = main;
        let arefs: Vec<&TraceMeta> = adopted.iter().collect();
        let a = curve_colours(&theme, &refs, &arefs).trace(TraceId(7));
        assert!(hue_gap(a, theme.families[0]) <= SAME_HUE_DEG, "{a:?}");
    }
}

#[test]
fn a_sweeps_first_run_takes_the_base() {
    let ms = [with_kind(
        2,
        "Genelec 1 m",
        MeasKind::Sweep {
            config: sweep_config(),
        },
        false,
    )];
    let refs: Vec<&Measurement> = ms.iter().collect();
    let owner = TraceOwner::Meas { meas: MeasId(2) };
    let traces = [
        trace(10, "run 1", owner, run(1)),
        trace(11, "run 2", owner, run(2)),
    ];
    let trefs: Vec<&TraceMeta> = traces.iter().collect();
    let theme = Theme::dark();
    let c = curve_colours(&theme, &refs, &trefs);
    assert_eq!(c.trace(TraceId(10)), theme.families[1]);
    assert_eq!(
        c.trace(TraceId(11)),
        Family::of(&theme, theme.families[1]).shades[0]
    );
}

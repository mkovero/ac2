//! The Programme pane end to end: the Meters view of a pair under a loudness ballistic (the
//! true-peak block, then the LUFS block with its dashed target and max tick, the
//! correlation strip under the bars, the plate fitted beside or above), a reading below the
//! floor, a line + mic Inputs pane, the plate floating see-through and solid, the true-peak
//! fallback at *full* and *no grid*, and the Goniometer view filling its pane
//! (docs/design/programme-meter.md §6.3). The values are asserted headless in `ac2-scene`;
//! this checks that the renderer draws what the scene says, and that tabular labels keep
//! their digits in columns.

use crate::common;
use crate::embers::{self, Figure};

use ac2_plot::{Anchor, Color, HAlign, Label, Layer, VAlign, Viewport, ember_points};
use ac2_scene::gonio::EMBER_DECAY_DEFAULT_S;
use ac2_scene::plate::{PlatePlace, SEE_THROUGH_ALPHA};
use ac2_scene::programme::loudness::{LoudnessReadout, MIC_TEXT};
use ac2_scene::programme::{
    Ballistic, Correlation, ProgrammeBar, ProgrammeInput, ProgrammePair, ProgrammePane, Scale,
    SourceBar, TRUE_PEAK_SHOWN, programme_scene,
};
use ac2_scene::theme::Theme;
use ac2_scene::view::{PlotChrome, ProgrammeMode, Style};
use ac2_testkit::image::{Image, ImageTolerance};
use common::{golden, gpu, render, render_on, renderer, scene};

const TEXT: ImageTolerance = ImageTolerance {
    channel: 32,
    max_bad_fraction: 0.005,
};

fn input(label: &str, scale: Scale, level: f32) -> ProgrammeInput {
    ProgrammeInput {
        label: label.into(),
        scale,
        ppm: Some(level),
        true_peak_dbtp: Some(level + 2.0),
        max: Some(level + 3.0),
        overs: 0,
        clipped: false,
        flying_peak: Some(level + 1.0),
        offers_vu: false,
    }
}

/// A true-peak bar under a loudness ballistic: what the input reads, and its reason when it
/// has no source.
fn peak(input: ProgrammeInput, reason: Option<&str>) -> ProgrammeBar {
    ProgrammeBar::Peak {
        input,
        note: reason.map(|r| format!("{TRUE_PEAK_SHOWN} · {r}")),
    }
}

/// `Main` summed: a reading above target (the warm end of the zone), a max further up.
fn main_bar() -> SourceBar {
    SourceBar {
        name: "Main".into(),
        reading: Some(-21.4),
        max: Some(-18.2),
    }
}

fn mono_bar(reading: f64, max: f64) -> SourceBar {
    SourceBar {
        name: "Vox".into(),
        reading: Some(reading),
        max: Some(max),
    }
}

fn readout() -> LoudnessReadout {
    LoudnessReadout {
        title: "Main".into(),
        momentary: -20.06,
        short_term: -21.44,
        integrated: Some(-23.1),
        range: Some(6.21),
        duration_s: 75.0,
        max_true_peak: -1.2,
        max_momentary: -16.0,
        max_short_term: -18.2,
        since: "since 20:14 · R resets".into(),
    }
}

/// A pair pane on `ShortTerm` in `view`: L and R true peak (one over on L), the summed
/// LUFS bar, the plate where `plate` puts it.
fn pair_pane(view: ProgrammeMode, plate: PlatePlace) -> ProgrammePane {
    let mut l = input("Main L", Scale::Dbfs, -14.0);
    l.overs = 1;
    let r = input("Main R", Scale::Dbfs, -16.0);
    ProgrammePane {
        inputs: vec![l.clone(), r.clone()],
        bars: vec![
            peak(l, None),
            peak(r, None),
            ProgrammeBar::Source(main_bar()),
        ],
        pair: Some(ProgrammePair {
            name: "Main".into(),
            correlation: Correlation::Value(0.874),
            window_s: 1.0,
        }),
        ballistic: Ballistic::ShortTerm,
        loudness_target: -23.0,
        loudness: Some(readout()),
        plate,
        plate_hover: None,
        view,
    }
}

/// An Inputs pane of `inputs` drawing `bars`.
fn inputs_pane(bars: Vec<ProgrammeBar>, inputs: Vec<ProgrammeInput>) -> ProgrammePane {
    ProgrammePane {
        inputs,
        bars,
        pair: None,
        ballistic: Ballistic::ShortTerm,
        loudness_target: -23.0,
        loudness: None,
        plate: PlatePlace::default(),
        plate_hover: None,
        view: ProgrammeMode::Meters,
    }
}

fn draw(name: &str, p: &ProgrammePane, theme: &Theme, chrome: PlotChrome, w: f32, h: f32) {
    let Some(gpu) = gpu(name) else {
        return;
    };
    let s = programme_scene(
        p,
        Style { chrome, theme },
        Viewport {
            width: w,
            height: h,
        },
        None,
    );
    let mut r = renderer(gpu);
    let img = render_on(gpu, &mut r, &s.scene, 1.0, theme.background);
    golden(name, &img, TEXT);
}

/// A pair's Meters view on a wide pane: the plate fitted beside the bars at its largest.
#[test]
fn programme_meters_pair_1700x900() {
    let p = pair_pane(ProgrammeMode::Meters, PlatePlace::default());
    draw(
        "programme_meters_pair_1700x900",
        &p,
        &Theme::dark(),
        PlotChrome::Full,
        1700.0,
        900.0,
    );
}

/// Too narrow for the plate beside the bars: it sits above them at 1×.
#[test]
fn programme_meters_pair_600x500() {
    let p = pair_pane(ProgrammeMode::Meters, PlatePlace::default());
    draw(
        "programme_meters_pair_600x500",
        &p,
        &Theme::dark(),
        PlotChrome::Full,
        600.0,
        500.0,
    );
}

/// A source 22 dB under target: the bar is empty, the floor mark and `below −41` say why.
#[test]
fn programme_below_floor() {
    let vox = input("Vox", Scale::Dbfs, -40.0);
    let p = inputs_pane(
        vec![
            peak(vox.clone(), None),
            ProgrammeBar::Source(mono_bar(-45.0, -38.0)),
        ],
        vec![vox],
    );
    draw(
        "programme_below_floor",
        &p,
        &Theme::dark(),
        PlotChrome::Full,
        340.0,
        360.0,
    );
}

/// A line input and a mic: both read true peak in the first block, the mic with its reason;
/// the line's source alone in the LUFS block.
#[test]
fn programme_line_and_mic() {
    let vox = input("Vox", Scale::Dbfs, -20.0);
    let mic = input("Mic", Scale::Dbfs, -24.0);
    let p = inputs_pane(
        vec![
            peak(vox.clone(), None),
            peak(mic.clone(), Some(MIC_TEXT)),
            ProgrammeBar::Source(mono_bar(-31.7, -26.0)),
        ],
        vec![vox, mic],
    );
    draw(
        "programme_line_and_mic",
        &p,
        &Theme::dark(),
        PlotChrome::Full,
        420.0,
        360.0,
    );
}

/// A mic alone falls back to its true peak on its own dBFS ticks with the reason under it;
/// at *no grid* the scale's labels go, its bar stays.
#[test]
fn programme_true_peak_fallback() {
    let mic = input("Mic", Scale::Dbfs, -24.0);
    let p = inputs_pane(vec![peak(mic.clone(), Some(MIC_TEXT))], vec![mic]);
    let theme = Theme::dark();
    for (name, chrome) in [
        ("programme_true_peak_fallback", PlotChrome::Full),
        ("programme_true_peak_fallback_no_grid", PlotChrome::NoGrid),
    ] {
        draw(name, &p, &theme, chrome, 340.0, 320.0);
    }
}

/// Wide but short: the plate stays beside at 1× and drops its last slot; the pair's
/// strip comes off the height first.
#[test]
fn programme_plate_dropped_rows() {
    let theme = Theme::dark();
    let font = theme.small_font_size;
    let (_, h1) = ac2_scene::programme::loudness_plate::size(font, 1.0, 1);
    draw(
        "programme_plate_dropped_rows",
        &pair_pane(ProgrammeMode::Meters, PlatePlace::default()),
        &theme,
        PlotChrome::Full,
        1100.0,
        h1 + ac2_scene::programme::CHIP_H + 20.5,
    );
}

/// Moved over the bars: see-through lets the bars show under the fill, solid covers them.
#[test]
fn programme_plate_floating() {
    let mut place = PlatePlace::default();
    place.move_to(1.0, 0.3);
    let p = pair_pane(ProgrammeMode::Meters, place);
    for (name, alpha) in [
        ("programme_plate_floating_see_through", SEE_THROUGH_ALPHA),
        ("programme_plate_floating_solid", 1.0),
    ] {
        let theme = Theme {
            plate_alpha: alpha,
            ..Theme::dark()
        };
        draw(name, &p, &theme, PlotChrome::Full, 440.0, 360.0);
    }
}

/// The Goniometer view on uncorrelated noise: the square fills the pane's height, centred,
/// the chip under it.
#[test]
fn programme_goniometer() {
    let theme = Theme::dark();
    let mut p = pair_pane(ProgrammeMode::Goniometer, PlatePlace::default());
    if let Some(pair) = p.pair.as_mut() {
        pair.correlation = Correlation::Value(0.02);
    }
    let ring = embers::ring(Figure::Noise, EMBER_DECAY_DEFAULT_S);
    for (name, w, h) in [
        ("programme_goniometer_900x900", 900.0, 900.0),
        ("programme_goniometer_1600x600", 1600.0, 600.0),
    ] {
        let Some(gpu) = gpu(name) else {
            return;
        };
        let mut s = programme_scene(
            &p,
            Style {
                chrome: PlotChrome::Full,
                theme: &theme,
            },
            Viewport {
                width: w,
                height: h,
            },
            None,
        );
        assert!(s.plate.is_none(), "the Goniometer view has no plate");
        let square = s.gonio.expect("the Goniometer view has its square");
        assert!(
            (square.x + square.w / 2.0 - w / 2.0).abs() < 1.0,
            "{name}: the square is centred: {square:?}"
        );
        assert!(
            square.h > h * 0.85,
            "{name}: the square fills the pane: {square:?}"
        );
        let points = ember_points(&ring, embers::NOW, EMBER_DECAY_DEFAULT_S, square);
        s.scene.layers[0].points.push(points);
        let mut r = renderer(gpu);
        let img = render_on(gpu, &mut r, &s.scene, 1.0, theme.background);
        golden(name, &img, TEXT);
    }
}

/// Leftmost column with ink in `x0 … x1` × `y0 … y1` (white text on black).
fn left_ink(img: &Image, x0: u32, x1: u32, y0: u32, y1: u32) -> Option<u32> {
    (x0..x1).find(|&x| (y0..y1).any(|y| img.pixel(x, y)[0] > 96))
}

/// Right-aligned numbers whose digits differ: with tabular figures every digit takes one
/// advance, so the sign, which is not a digit, starts at the same column whatever follows;
/// with the font's default proportional figures a run of `1`s is narrower and the sign moves.
#[test]
fn tabular_digits_keep_their_columns() {
    let Some(gpu) = gpu("tabular_digits_keep_their_columns") else {
        return;
    };
    let mut r = renderer(gpu);
    let texts = ["−11.1", "−88.8", "−40.7", "−23.0"];
    let row = 40.0;
    let mut layer = Layer::default();
    for (col, tabular) in [false, true].into_iter().enumerate() {
        for (i, t) in texts.iter().enumerate() {
            layer.labels.push(Label {
                text: (*t).into(),
                pos: [180.0 + 200.0 * col as f32, 10.0 + row * i as f32],
                anchor: Anchor {
                    h: HAlign::Right,
                    v: VAlign::Top,
                },
                size: 28.0,
                color: Color::WHITE,
                clip: None,
                tabular,
            });
        }
    }
    let img = render(gpu, &mut r, &scene(400.0, 170.0, vec![layer]), 1.0);
    let lefts = |x0: u32| -> Vec<u32> {
        (0..texts.len() as u32)
            .map(|i| {
                let y = 10 + 40 * i;
                left_ink(&img, x0, x0 + 200, y, y + 36).expect("ink")
            })
            .collect()
    };
    let proportional = lefts(0);
    let tabular = lefts(200);
    assert!(
        tabular.iter().all(|&x| x == tabular[0]),
        "tabular signs start in one column: {tabular:?}"
    );
    assert!(
        proportional[0] > proportional[1] + 4,
        "proportional `1`s are narrower than `8`s: {proportional:?}"
    );
}

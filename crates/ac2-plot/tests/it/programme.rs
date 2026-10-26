//! The Programme pane under a loudness ballistic, end to end: the scene's loudness bars
//! (a pair's summed source and a mono one, the dashed target, the max ticks), the true-peak
//! fallback bar with its reason, and the loudness plate docked beside, above, with dropped
//! slots, floating see-through and solid, and hidden at *traces only* with its room kept
//! (docs/design/programme-meter.md §6.2). The values are asserted headless in `ac2-scene`;
//! this checks that the renderer draws what the scene says, and that tabular labels keep
//! their digits in columns.

use crate::common;

use ac2_plot::{Anchor, Color, HAlign, Label, Layer, VAlign, Viewport};
use ac2_scene::plate::{PlatePlace, SEE_THROUGH_ALPHA};
use ac2_scene::programme::loudness::{LoudnessReadout, MIC_TEXT};
use ac2_scene::programme::{
    Ballistic, Correlation, ProgrammeBar, ProgrammeInput, ProgrammePair, ProgrammePane, Scale,
    ScaleKind, SideFlags, SourceBar, loudness_plate, programme_scene, scale_w,
};
use ac2_scene::theme::Theme;
use ac2_scene::view::{PlotChrome, Style};
use ac2_testkit::image::{Image, ImageTolerance};
use common::{golden, gpu, render, render_on, renderer, scene};

const TEXT: ImageTolerance = ImageTolerance {
    channel: 32,
    max_bad_fraction: 0.005,
};

/// The scene's outer padding around the pane's parts (`programme_scene`).
const PAD: f32 = 10.0;

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

/// `Main` summed: a reading above target (the warm end of the zone), a max further up.
fn main_bar() -> SourceBar {
    SourceBar {
        name: "Main".into(),
        reading: Some(-21.4),
        max: Some(-18.2),
        true_peak: Some(-3.1),
        sides: vec![
            SideFlags {
                overs: 2,
                clipped: false,
            },
            SideFlags {
                overs: 0,
                clipped: false,
            },
        ],
    }
}

fn mono_bar() -> SourceBar {
    SourceBar {
        name: "Vox mono".into(),
        reading: Some(-31.7),
        max: Some(-26.0),
        true_peak: Some(-9.4),
        sides: vec![SideFlags {
            overs: 0,
            clipped: false,
        }],
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

/// A Pair pane on `ShortTerm`: one summed bar, the goniometer, the plate where `plate` puts it.
fn pair_pane(plate: PlatePlace) -> ProgrammePane {
    ProgrammePane {
        inputs: vec![
            input("Main L", Scale::Dbfs, -14.0),
            input("Main R", Scale::Dbfs, -16.0),
        ],
        bars: vec![ProgrammeBar::Source(main_bar())],
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
    }
}

fn scaled(scale: f32) -> PlatePlace {
    let mut p = PlatePlace::default();
    p.set_scale(scale);
    p
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

/// Room for the bar, its LUFS scale and the goniometer beside a plate of `(w, h)`.
fn beside(plate: (f32, f32)) -> (f32, f32) {
    let small = Theme::dark().small_font_size;
    let need = scale_w(ScaleKind::Lufs, small)
        + ac2_scene::programme::BAR_MIN_W
        + ac2_scene::programme::GONIO_MIN_W;
    (
        need + plate.0 + 40.0 + 2.0 * PAD,
        plate.1.max(260.0) + 2.0 * PAD,
    )
}

/// An Inputs pane of two sources: a pair's summed bar and a mono one, no plate (more than
/// one source), each with its dashed target and max ticks.
#[test]
fn programme_bars_pair_and_mono() {
    let p = ProgrammePane {
        inputs: vec![
            input("Main L", Scale::Dbfs, -14.0),
            input("Main R", Scale::Dbfs, -16.0),
            input("Vox", Scale::Dbfs, -20.0),
        ],
        bars: vec![
            ProgrammeBar::Source(main_bar()),
            ProgrammeBar::Source(mono_bar()),
        ],
        pair: None,
        ballistic: Ballistic::ShortTerm,
        loudness_target: -23.0,
        loudness: None,
        plate: PlatePlace::default(),
        plate_hover: None,
    };
    draw(
        "programme_bars_pair_and_mono",
        &p,
        &Theme::dark(),
        PlotChrome::Full,
        300.0,
        320.0,
    );
}

/// A line source beside a mic: the mic's bar falls back to the true peak on its own dBFS
/// ticks with the reason under it; at *no grid* the target label goes, its line stays.
#[test]
fn programme_true_peak_fallback() {
    let mic = input("Mic", Scale::Dbfs, -24.0);
    let p = ProgrammePane {
        inputs: vec![input("Vox", Scale::Dbfs, -20.0), mic.clone()],
        bars: vec![
            ProgrammeBar::Source(mono_bar()),
            ProgrammeBar::Peak {
                input: mic,
                note: Some(MIC_TEXT.into()),
            },
        ],
        pair: None,
        ballistic: Ballistic::ShortTerm,
        loudness_target: -23.0,
        loudness: None,
        plate: PlatePlace::default(),
        plate_hover: None,
    };
    let theme = Theme::dark();
    for (name, chrome) in [
        ("programme_true_peak_fallback", PlotChrome::Full),
        ("programme_true_peak_fallback_no_grid", PlotChrome::NoGrid),
    ] {
        draw(name, &p, &theme, chrome, 340.0, 320.0);
    }
}

#[test]
fn programme_plate_beside() {
    let theme = Theme::dark();
    let font = theme.small_font_size;
    for (name, s) in [
        ("programme_plate_beside_1x", 1.0),
        ("programme_plate_beside_2x", 2.0),
    ] {
        let (w, h) = beside(loudness_plate::size(font, s, 0));
        draw(name, &pair_pane(scaled(s)), &theme, PlotChrome::Full, w, h);
    }
}

/// Too narrow for the plate beside, tall enough for it above the bars.
#[test]
fn programme_plate_above() {
    let theme = Theme::dark();
    let (pw, ph) = loudness_plate::size(theme.small_font_size, 1.0, 0);
    let w = pw + 2.0 * PAD + 10.0;
    let h = ph + ac2_scene::programme::BAR_MIN_H + 2.0 * PAD + 60.0;
    draw(
        "programme_plate_above",
        &pair_pane(PlatePlace::default()),
        &theme,
        PlotChrome::Full,
        w,
        h,
    );
}

/// Wide but short: the plate stays beside and drops its last slots.
#[test]
fn programme_plate_dropped_rows() {
    let theme = Theme::dark();
    let font = theme.small_font_size;
    let (w, _) = beside(loudness_plate::size(font, 1.0, 0));
    let (_, h2) = loudness_plate::size(font, 1.0, 2);
    draw(
        "programme_plate_dropped_rows",
        &pair_pane(PlatePlace::default()),
        &theme,
        PlotChrome::Full,
        w,
        h2 + 2.0 * PAD + 0.5,
    );
}

/// Moved over the bars: see-through lets the bar and target show under the fill, solid
/// covers them.
#[test]
fn programme_plate_floating() {
    let mut place = PlatePlace::default();
    place.move_to(1.0, 0.3);
    let p = pair_pane(place);
    for (name, alpha) in [
        ("programme_plate_floating_see_through", SEE_THROUGH_ALPHA),
        ("programme_plate_floating_solid", 1.0),
    ] {
        let theme = Theme {
            plate_alpha: alpha,
            ..Theme::dark()
        };
        draw(name, &p, &theme, PlotChrome::Full, 340.0, 340.0);
    }
}

/// *Traces only*: the plate and the scale's numbers go, their room stays (the bar does not
/// widen), the bar, target line and max ticks stay.
#[test]
fn programme_traces_only() {
    let theme = Theme::dark();
    let (w, h) = beside(loudness_plate::size(theme.small_font_size, 1.0, 0));
    draw(
        "programme_traces_only",
        &pair_pane(PlatePlace::default()),
        &theme,
        PlotChrome::Bare,
        w,
        h,
    );
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

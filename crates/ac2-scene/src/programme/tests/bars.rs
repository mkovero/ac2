//! Bars per source under a loudness ballistic, the fallback rows, the LUFS scale, zones,
//! target and max ticks (docs/design/programme-meter.md §6.2).

use ac2_proto::frame::{LoudnessSource, SourceLoudness};
use ac2_proto::units::{Db, Dbtp, Lufs, Seconds, WallNs};

use super::super::draw::{TICK_W, above_target};
use super::super::loudness::{LoudnessFacts, NamedPair, plate_readout};
use super::super::*;
use super::{pane_with_pair, texts};
use crate::primitives::{FillRect, Scene, Viewport};
use crate::theme::Theme;
use crate::view::{PlotChrome, Style};

pub(super) fn source(source: LoudnessSource) -> SourceLoudness {
    SourceLoudness {
        source,
        momentary: Lufs(-23.06),
        short_term: Lufs(-22.84),
        integrated: Some(Lufs(-23.0)),
        range: Some(Db(6.21)),
        max_momentary: Lufs(-20.0),
        max_short_term: Lufs(-21.0),
        max_true_peak: Dbtp(-1.2),
        duration: Seconds(75.0),
        since: WallNs(1),
    }
}

pub(super) const MAIN: LoudnessSource = LoudnessSource::Pair { left: 2, right: 3 };

/// Inputs 1 (line) and 3+4 (the line pair Main) captured at 48 kHz; 2 a mic; 5+6 the pair
/// Spare with 6 not captured; 7+8 the pair Talk with 8 a mic. Input `c` peaks at
/// −(c + 1) dBTP.
pub(super) struct Rig {
    pub rate_hz: u32,
    pub pairs: Vec<NamedPair>,
    pub captured: Vec<u16>,
    pub sources: Vec<SourceLoudness>,
    pub overs: [u32; 8],
    pub clipped: [bool; 8],
}

impl Rig {
    pub fn new() -> Self {
        let pair = |left, right, name: &str| NamedPair {
            left,
            right,
            name: name.into(),
        };
        Self {
            rate_hz: 48_000,
            pairs: vec![pair(2, 3, "Main"), pair(4, 5, "Spare"), pair(6, 7, "Talk")],
            captured: vec![0, 1, 2, 3, 4, 6, 7],
            sources: vec![source(MAIN), source(LoudnessSource::Input { channel: 0 })],
            overs: [0; 8],
            clipped: [false; 8],
        }
    }

    pub fn input(&self, c: u16) -> ProgrammeInput {
        let level = -(f32::from(c) + 1.0);
        ProgrammeInput {
            label: format!("In {}", c + 1),
            scale: Scale::Dbfs,
            ppm: Some(level),
            true_peak_dbtp: Some(level),
            max: Some(level),
            overs: self.overs[usize::from(c)],
            clipped: self.clipped[usize::from(c)],
            flying_peak: Some(level),
            offers_vu: false,
        }
    }

    /// Runs `f` with the rig's facts.
    pub fn with<R>(&self, f: impl FnOnce(&LoudnessFacts<'_>) -> R) -> R {
        let is_mic = |c: u16| c == 1 || c == 7;
        let label = |c: u16| format!("In {}", c + 1);
        let own_name = |c: u16| (c == 5).then(|| "Spare R".to_owned());
        let clock = |_: WallNs| "20:14".to_owned();
        let input = |c: u16| self.captured.contains(&c).then(|| self.input(c));
        f(&LoudnessFacts {
            rate_hz: self.rate_hz,
            pairs: &self.pairs,
            captured: &self.captured,
            sources: &self.sources,
            is_mic: &is_mic,
            label: &label,
            input: &input,
            own_name: &own_name,
            clock: &clock,
            reset_key: Some("R"),
        })
    }

    pub fn bars(&self, shown: &[u16], b: Ballistic) -> Vec<ProgrammeBar> {
        self.with(|f| bars_of(&sources(shown, f), b, f))
    }

    /// The pane as the UI fills it: bars, plate data, the pair when `shown` is one.
    pub fn pane(&self, shown: &[u16], b: Ballistic) -> ProgrammePane {
        self.with(|f| {
            let r = sources(shown, f);
            let pair = self
                .pairs
                .iter()
                .find(|p| shown == [p.left, p.right])
                .map(|p| ProgrammePair {
                    name: p.name.clone(),
                    correlation: Correlation::Value(0.5),
                    window_s: 1.0,
                });
            ProgrammePane {
                inputs: shown.iter().map(|&c| self.input(c)).collect(),
                bars: bars_of(&r, b, f),
                pair,
                ballistic: b,
                loudness_target: -23.0,
                loudness: plate_readout(&r, f),
                plate: crate::plate::PlatePlace::default(),
                plate_hover: None,
            }
        })
    }
}

fn source_bar(b: &ProgrammeBar) -> &SourceBar {
    match b {
        ProgrammeBar::Source(s) => s,
        other => panic!("a source bar, got {other:?}"),
    }
}

/// A peak bar's label and note.
fn peak(b: &ProgrammeBar) -> (&str, Option<&str>) {
    match b {
        ProgrammeBar::Peak { input, note } => (&input.label, note.as_deref()),
        other => panic!("a peak bar, got {other:?}"),
    }
}

fn names(bars: &[ProgrammeBar]) -> Vec<String> {
    bars.iter()
        .map(|b| match b {
            ProgrammeBar::Peak { input, .. } => format!("peak {}", input.label),
            ProgrammeBar::Source(s) => format!("source {}", s.name),
        })
        .collect()
}

/// Under a loudness ballistic every per-input reading is the true peak: the Inputs list
/// row, the unit and the bar's text say so, with no note.
#[test]
fn loudness_ballistics_read_the_true_peak_per_input() {
    let i = Rig::new().input(0);
    for b in [Ballistic::Momentary, Ballistic::ShortTerm] {
        assert!(b.is_loudness());
        assert_eq!(i.reads(b), Ballistic::TruePeak);
        assert_eq!(i.unit(b), "dBTP");
        assert_eq!(i.note(b), None);
        assert_eq!(i.row_lines(b), i.row_lines(Ballistic::TruePeak));
        assert_eq!(i.ppm_text(b), "−1.0 dBTP");
    }
    assert_eq!(Ballistic::Momentary.word(), "Mom loudness");
    assert_eq!(Ballistic::ShortTerm.word(), "Short loudness");
    for b in [
        Ballistic::TruePeak,
        Ballistic::Type1,
        Ballistic::Type2,
        Ballistic::Vu,
    ] {
        assert!(!b.is_loudness());
    }
}

/// A pair reads one bar, both channels summed; the pane's bars are its sources in the
/// order the pane first names one of their inputs.
#[test]
fn a_bar_per_source() {
    let rig = Rig::new();
    let pair = rig.bars(&[2, 3], Ballistic::ShortTerm);
    assert_eq!(names(&pair), ["source Main"]);
    let s = source_bar(&pair[0]);
    assert_eq!(s.reading, Some(-22.84));
    assert_eq!(s.max, Some(-21.0));
    assert_eq!(s.true_peak, Some(-3.0), "the higher side's");
    assert_eq!(s.sides.len(), 2);
    assert_eq!(s.reading_text(), "−22.8 LUFS");
    assert_eq!(s.max_value_text(), "−21.0 LUFS");
    assert_eq!(s.true_peak_value_text(), "−3.0 dBTP");
    let mom = rig.bars(&[2, 3], Ballistic::Momentary);
    let m = source_bar(&mom[0]);
    assert_eq!((m.reading, m.max), (Some(-23.06), Some(-20.0)));
    // `1, 5, 2` with `1+2` a pair, here `3, 1, 4` with `3+4` Main: Main where 3 was, then
    // input 1 alone.
    assert_eq!(
        names(&rig.bars(&[2, 0, 3], Ballistic::ShortTerm)),
        ["source Main", "source In 1 mono"]
    );
    // Half a pair alone reads the pair, both sides' flags.
    let half = rig.bars(&[3], Ballistic::ShortTerm);
    assert_eq!(names(&half), ["source Main"]);
    assert_eq!(source_bar(&half[0]).sides.len(), 2);
    // So does one half among other inputs.
    assert_eq!(
        names(&rig.bars(&[2, 0], Ballistic::ShortTerm)),
        ["source Main", "source In 1 mono"]
    );
    // Under a peak ballistic a pair keeps its L and R.
    assert_eq!(
        names(&rig.bars(&[2, 3], Ballistic::TruePeak)),
        ["peak In 3", "peak In 4"]
    );
    // A source the frame does not carry yet: an empty bar with — readings.
    let mut fresh = Rig::new();
    fresh.sources.clear();
    let b = fresh.bars(&[2, 3], Ballistic::ShortTerm);
    let s = source_bar(&b[0]);
    assert_eq!((s.reading, s.max), (None, None));
    assert_eq!(s.reading_text(), "—");
    assert_eq!(s.max_value_text(), "—");
}

/// An input without a source reads its true peak, the reason last, what the bar reads
/// first.
#[test]
fn inputs_without_a_source_say_why_on_their_bar() {
    let rig = Rig::new();
    let mic = rig.bars(&[1], Ballistic::ShortTerm);
    assert_eq!(
        peak(&mic[0]),
        ("In 2", Some("true peak shown · no loudness on a mic input"))
    );
    let partner = rig.bars(&[4], Ballistic::Momentary);
    assert_eq!(
        peak(&partner[0]).1,
        Some("true peak shown · loudness: Spare R (input 6) not captured")
    );
    // The other side a mic: the mic reason, the bar its own input's.
    let talk = rig.bars(&[6], Ballistic::ShortTerm);
    assert_eq!(
        peak(&talk[0]),
        ("In 7", Some("true peak shown · no loudness on a mic input"))
    );
    let mut slow = Rig::new();
    slow.rate_hz = 32_000;
    let rate = slow.bars(&[2, 3], Ballistic::ShortTerm);
    let want = format!("true peak shown · {}", super::super::loudness::RATE_TEXT);
    assert_eq!(names(&rate), ["peak In 3", "peak In 4"]);
    for b in &rate {
        assert_eq!(peak(b).1, Some(want.as_str()));
    }
    // Under true peak nothing claims loudness: no note, no plate.
    let tp = slow.pane(&[2, 3], Ballistic::TruePeak);
    assert!(tp.bars.iter().all(|b| peak(b).1.is_none()));
    assert_eq!(tp.loudness, None);
    let t = texts(&scene(&tp, PlotChrome::Full, 800.0, 400.0));
    assert!(!t.iter().any(|x| x.contains("LUFS")), "{t:?}");
    // The VU fallback reads in the same order.
    assert_eq!(VU_FALLBACK, "true peak shown · VU needs a line calibration");
    let vu = rig.bars(&[0], Ballistic::Vu);
    assert_eq!(peak(&vu[0]).1, Some(VU_FALLBACK));
}

/// A pane has the plate iff its inputs resolve to exactly one source, whatever the
/// ballistic.
#[test]
fn one_source_one_plate() {
    let rig = Rig::new();
    let readout = |shown: &[u16], b| rig.pane(shown, b).loudness;
    let main = readout(&[2, 3], Ballistic::TruePeak).expect("a pair pane under true peak");
    assert_eq!(main.title, "Main");
    assert_eq!(main.since, "since 20:14 · R resets");
    assert_eq!(main.max_momentary, -20.0);
    assert_eq!(main.max_short_term, -21.0);
    assert!(readout(&[3], Ballistic::ShortTerm).is_some());
    assert_eq!(readout(&[2, 0], Ballistic::ShortTerm), None, "two sources");
    assert_eq!(readout(&[1], Ballistic::ShortTerm), None, "a mic");
    assert_eq!(readout(&[1, 7], Ballistic::ShortTerm), None, "two mics");
    let line_and_mic = readout(&[0, 1], Ballistic::ShortTerm).expect("one source");
    assert_eq!(line_and_mic.title, "In 1 mono");
    // Before the frame carries the source: there, every field −∞ / none, a blank foot.
    let mut fresh = Rig::new();
    fresh.sources.clear();
    let r = fresh.pane(&[2, 3], Ballistic::ShortTerm).loudness;
    assert_eq!(
        r,
        Some(super::super::loudness::LoudnessReadout::empty(
            "Main".into()
        ))
    );
}

/// A pair's flags named by side, left first, overs before CLIP; a mono source's as a peak
/// bar's.
#[test]
fn flags_name_their_side() {
    let flags = |sides: &[(u32, bool)]| {
        SourceBar {
            name: "Main".into(),
            reading: None,
            max: None,
            true_peak: None,
            sides: sides
                .iter()
                .map(|&(overs, clipped)| SideFlags { overs, clipped })
                .collect(),
        }
        .flags_text()
    };
    assert_eq!(flags(&[(0, false), (0, false)]), "");
    assert_eq!(flags(&[(1, false), (0, false)]), "1 over L");
    assert_eq!(flags(&[(2, false), (1, false)]), "2 overs L · 1 over R");
    assert_eq!(flags(&[(0, false), (0, true)]), "CLIP R");
    assert_eq!(
        flags(&[(2, true), (0, true)]),
        "2 overs L · CLIP L · CLIP R"
    );
    assert_eq!(
        flags(&[(1000, true), (5000, true)]),
        "999+ overs L · 999+ overs R · CLIP L · CLIP R"
    );
    assert_eq!(flags(&[(3, true)]), "3 overs");
    assert_eq!(flags(&[(0, true)]), "");
}

#[test]
fn lufs_fill_spans_target_minus_18_to_plus_9() {
    assert_eq!(lufs_fill(Some(-23.0), -23.0), 18.0 / 27.0);
    assert_eq!(lufs_fill(Some(-41.0), -23.0), 0.0);
    assert_eq!(lufs_fill(Some(-14.0), -23.0), 1.0);
    assert_eq!(lufs_fill(Some(-60.0), -23.0), 0.0);
    assert_eq!(lufs_fill(Some(0.0), -23.0), 1.0);
    assert_eq!(lufs_fill(Some(-14.0), -14.0), 18.0 / 27.0);
    assert_eq!(lufs_fill(None, -23.0), 0.0);
    assert_eq!(lufs_fill(Some(f64::NEG_INFINITY), -23.0), 0.0);
}

pub(super) fn scene(p: &ProgrammePane, chrome: PlotChrome, w: f32, h: f32) -> Scene {
    scene_of(p, chrome, w, h).scene
}

pub(super) fn scene_of(p: &ProgrammePane, chrome: PlotChrome, w: f32, h: f32) -> ProgrammeScene {
    let theme = Theme::dark();
    programme_scene(
        p,
        Style {
            chrome,
            theme: &theme,
        },
        Viewport {
            width: w,
            height: h,
        },
        None,
    )
}

/// The bars' backgrounds: the tall plot-background rects of the base layer.
pub(super) fn bar_rects(s: &Scene) -> Vec<crate::primitives::Rect> {
    let bg = Theme::dark().plot_background;
    s.layers[0]
        .rects
        .iter()
        .filter(|r| r.color == bg && r.rect.h > 40.0)
        .map(|r| r.rect)
        .collect()
}

#[test]
fn the_title_says_what_the_bars_read() {
    let rig = Rig::new();
    assert_eq!(
        rig.pane(&[2, 3], Ballistic::ShortTerm).title(),
        "Short loudness · target −23 LUFS · Main · 1 s"
    );
    assert_eq!(
        rig.pane(&[0], Ballistic::Momentary).title(),
        "Mom loudness · target −23 LUFS"
    );
    assert_eq!(rig.pane(&[1], Ballistic::ShortTerm).title(), "true peak");
    assert_eq!(rig.pane(&[0], Ballistic::Vu).title(), "true peak");
    let mut vu = rig.pane(&[0], Ballistic::Vu);
    for b in &mut vu.bars {
        if let ProgrammeBar::Peak { input, .. } = b {
            input.offers_vu = true;
        }
    }
    assert_eq!(vu.title(), "VU");
    assert_eq!(
        rig.pane(&[2, 3], Ballistic::Type1).title(),
        "Type I · Main · 1 s"
    );
}

/// The LUFS scale: every 3 LU on a tall bar, every 9 on a short one, the target named at
/// *full*, its plain number at *no grid*, no numbers at *traces only*.
#[test]
fn lufs_scale_labels_follow_the_target() {
    let rig = Rig::new();
    let mut p = rig.pane(&[0], Ballistic::ShortTerm);
    let tall = texts(&scene(&p, PlotChrome::Full, 600.0, 600.0));
    let want: Vec<String> = [-41, -38, -35, -32, -29, -26]
        .iter()
        .map(|v| crate::format::fixed(f64::from(*v), 0))
        .chain(["target −23".to_owned()])
        .chain(["−20", "−17", "−14"].map(str::to_owned))
        .collect();
    for w in &want {
        assert!(tall.contains(w), "{w} in {tall:?}");
    }
    let short = texts(&scene(&p, PlotChrome::Full, 600.0, 230.0));
    for w in ["−41", "−32", "target −23", "−14"] {
        assert!(short.iter().any(|t| t == w), "{w} in {short:?}");
    }
    for w in ["−38", "−26"] {
        assert!(!short.iter().any(|t| t == w), "{w} in {short:?}");
    }
    let no_grid = texts(&scene(&p, PlotChrome::NoGrid, 600.0, 600.0));
    assert!(no_grid.iter().any(|t| t == "−23"), "{no_grid:?}");
    assert!(
        !no_grid.iter().any(|t| t.starts_with("target")),
        "{no_grid:?}"
    );
    let bare = texts(&scene(&p, PlotChrome::Bare, 600.0, 600.0));
    assert!(!bare.iter().any(|t| t == "−41" || t == "−23"), "{bare:?}");
    p.loudness_target = -14.0;
    let t = texts(&scene(&p, PlotChrome::Full, 600.0, 600.0));
    for w in ["−32", "target −14", "−5"] {
        assert!(t.iter().any(|x| x == w), "{w} in {t:?}");
    }
    assert_eq!(p.title(), "Short loudness · target −14 LUFS");
}

/// The source bar's fill rects: green below the target, the lighter green above.
fn fills(s: &Scene) -> Vec<FillRect> {
    s.layers[1]
        .rects
        .iter()
        .filter(|r| r.rect.w > TICK_W + 1.0)
        .copied()
        .collect()
}

#[test]
fn zones_split_at_the_target_and_ticks_mark_the_max() {
    let theme = Theme::dark();
    let mut rig = Rig::new();
    rig.sources[1].short_term = Lufs(-17.0);
    rig.sources[1].max_short_term = Lufs(-15.5);
    let p = rig.pane(&[0], Ballistic::ShortTerm);
    let s = scene(&p, PlotChrome::Full, 600.0, 400.0);
    let bar = bar_rects(&s)[0];
    let f = fills(&s);
    assert_eq!(f.len(), 2, "{f:?}");
    let y_of = |v: f64| bar.y + bar.h * (1.0 - lufs_fill(Some(v), -23.0));
    assert_eq!(f[0].color, theme.level_ok);
    assert!((f[0].rect.y - y_of(-23.0)).abs() < 1e-3);
    assert!((f[0].rect.bottom() - bar.bottom()).abs() < 1e-3);
    let blend = above_target(&theme);
    assert_eq!(f[1].color, blend);
    for (c, a, b) in [
        (blend.r, theme.level_ok.r, theme.text.r),
        (blend.g, theme.level_ok.g, theme.text.g),
        (blend.b, theme.level_ok.b, theme.text.b),
    ] {
        assert!((c - 0.5 * (a + b)).abs() < 1e-6);
    }
    assert!((f[1].rect.y - y_of(-17.0)).abs() < 1e-3);
    assert!((f[1].rect.bottom() - y_of(-23.0)).abs() < 1e-3);
    // No yellow or red, however near full scale.
    assert!(f.iter().all(|r| r.color != theme.banner_warning.background));
    // The max: a tick just outside each edge, 4 × 2 px, in the dim text colour.
    let ticks: Vec<&FillRect> = s.layers[1]
        .rects
        .iter()
        .filter(|r| r.rect.w == TICK_W)
        .collect();
    assert_eq!(ticks.len(), 2);
    for t in &ticks {
        assert_eq!(t.rect.h, 2.0);
        assert_eq!(t.color, theme.text_dim);
        assert!((t.rect.y + 1.0 - y_of(-15.5)).abs() < 1e-3);
    }
    assert!(ticks[0].rect.right() < bar.x && ticks[1].rect.x > bar.right());
    // No flying peak: nothing bar-wide and 2 px high on a loudness bar.
    assert!(
        !s.layers[1]
            .rects
            .iter()
            .any(|r| r.rect.h == 2.0 && r.rect.w == bar.w)
    );
    // The target: dashed, 1 px, the text colour, across the bar.
    let line = s.layers[1]
        .polylines
        .iter()
        .find(|l| l.stroke.dash.is_some())
        .expect("the target line");
    assert_eq!(line.stroke.width, 1.0);
    assert_eq!(line.stroke.color, theme.text);
    assert!((line.points[0][1] - y_of(-23.0)).abs() < 1e-3);
    assert!(line.points[0][0] < bar.x && line.points[1][0] > bar.right());
    let t = texts(&s);
    for w in [
        "−17.0 LUFS",
        "−15.5 LUFS",
        "−1.0 dBTP",
        "In 1 mono",
        "max",
        "TP",
    ] {
        assert!(t.iter().any(|x| x == w), "{w} in {t:?}");
    }
    // A max outside the span, or none, draws no tick.
    for m in [-60.0, 3.0, f64::NEG_INFINITY] {
        let mut r = Rig::new();
        r.sources[1].max_short_term = Lufs(m);
        let s = scene(
            &r.pane(&[0], Ballistic::ShortTerm),
            PlotChrome::Full,
            600.0,
            400.0,
        );
        assert!(!s.layers[1].rects.iter().any(|r| r.rect.w == TICK_W), "{m}");
    }
}

/// A line beside a mic under a loudness ballistic: the pane's scale is LUFS, the mic's bar
/// carries its own dBFS numbers and its reason row; a pane of mics only has the dBFS scale
/// and no target line.
#[test]
fn a_fallback_bar_carries_its_own_ticks() {
    let rig = Rig::new();
    let p = rig.pane(&[0, 1], Ballistic::ShortTerm);
    assert_eq!(p.scale_kind(), ScaleKind::Lufs);
    let s = scene(&p, PlotChrome::Full, 800.0, 400.0);
    let t = texts(&s);
    for w in ["0", "−20", "−40", "−60", "target −23"] {
        assert!(t.iter().any(|x| x == w), "{w} in {t:?}");
    }
    assert!(t.iter().any(|x| x.starts_with("true peak shown")), "{t:?}");
    let mics = rig.pane(&[1], Ballistic::ShortTerm);
    assert_eq!(mics.scale_kind(), ScaleKind::Dbfs);
    let s = scene(&mics, PlotChrome::Full, 800.0, 400.0);
    assert!(
        !s.layers[1]
            .polylines
            .iter()
            .any(|l| l.stroke.dash.is_some())
    );
    let t = texts(&s);
    assert!(!t.iter().any(|x| x.contains("target")), "{t:?}");
    assert!(
        t.iter().any(|x| x == "−50"),
        "the pane's own dBFS scale: {t:?}"
    );
}

/// **T** moves nothing: every bar keeps its rect at every step, the scale its width with
/// its numbers gone.
#[test]
fn t_moves_no_bar() {
    let rig = Rig::new();
    for (shown, b) in [
        (&[2u16, 3][..], Ballistic::ShortTerm),
        (&[0, 1][..], Ballistic::Momentary),
        (&[2, 3][..], Ballistic::TruePeak),
    ] {
        let p = rig.pane(shown, b);
        let full = bar_rects(&scene(&p, PlotChrome::Full, 900.0, 400.0));
        assert!(!full.is_empty());
        for chrome in [PlotChrome::NoGrid, PlotChrome::Bare] {
            assert_eq!(
                bar_rects(&scene(&p, chrome, 900.0, 400.0)),
                full,
                "{shown:?} {b:?} {chrome:?}"
            );
        }
    }
    // The pair's L / R bars of a peak pane are the same as before the plate existed.
    let p = pane_with_pair(Correlation::Value(0.5));
    assert_eq!(
        bar_rects(&scene(&p, PlotChrome::Full, 600.0, 400.0)).len(),
        2
    );
}

fn reasons(rig: &Rig, shown: &[u16]) -> Vec<Result<LoudnessSource, String>> {
    rig.with(|f| {
        sources(shown, f)
            .into_iter()
            .map(|r| match r {
                Resolved::Source { source, .. } => Ok(source),
                Resolved::Reason { reason, .. } => Err(reason),
            })
            .collect()
    })
}

/// Each input resolves to its pair (either side, in either order), else its mono source,
/// else why not: the rate, then a mic, then a partner not captured.
#[test]
fn sources_resolve_each_input() {
    let rig = Rig::new();
    let mono = LoudnessSource::Input { channel: 0 };
    assert_eq!(reasons(&rig, &[3, 2]), [Ok(MAIN), Ok(MAIN)]);
    assert_eq!(reasons(&rig, &[0, 3]), [Ok(mono), Ok(MAIN)]);
    let mic = Err(super::super::loudness::MIC_TEXT.to_owned());
    assert_eq!(reasons(&rig, &[1, 6]), [mic.clone(), mic.clone()]);
    assert_eq!(
        reasons(&rig, &[4]),
        [Err("loudness: Spare R (input 6) not captured".to_owned())]
    );
    let mut unnamed = Rig::new();
    unnamed.pairs[1].right = 9;
    assert_eq!(
        reasons(&unnamed, &[4]),
        [Err("loudness: input 10 not captured".to_owned())]
    );
    // A mic beats the partner: pair 7+8 with 8 a mic and not captured.
    let mut gone = Rig::new();
    gone.captured.retain(|c| *c != 7);
    assert_eq!(reasons(&gone, &[6]), [mic]);
    // The rate beats everything.
    gone.rate_hz = 32_000;
    let rate = Err(super::super::loudness::RATE_TEXT.to_owned());
    assert_eq!(
        reasons(&gone, &[0, 1, 4]),
        [rate.clone(), rate.clone(), rate]
    );
    assert!(reasons(&rig, &[]).is_empty());
}

#[test]
fn loudness_rates_and_since() {
    use super::super::loudness::{rate_has_loudness, since_text};
    for hz in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
        assert!(rate_has_loudness(hz), "{hz}");
    }
    for hz in [32_000, 47_952, 384_000, 44_000 - 100, 192_100] {
        assert!(!rate_has_loudness(hz), "{hz}");
    }
    assert_eq!(since_text("20:14", None), "since 20:14");
    assert_eq!(since_text("20:14", Some("R")), "since 20:14 · R resets");
}

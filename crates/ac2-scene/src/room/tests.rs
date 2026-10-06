use super::*;
use crate::primitives::Viewport;
use ac2_proto::units::{Db, Hz, Seconds};

fn band(centre: Option<f64>, t: f64, range: f64) -> RoomBand {
    let v = |value: f64| RoomValue::Value { value };
    let need = |needed: f64, x: f64| {
        if range >= needed {
            v(x)
        } else {
            RoomValue::Refused {
                reason: RoomRefusal::InsufficientRange {
                    range: Db(range),
                    needed: Db(needed),
                },
            }
        }
    };
    RoomBand {
        centre: centre.map(Hz),
        onset: Seconds(0.0),
        truncation: Seconds(0.8),
        decay_range: Some(Db(range)),
        edt: v(t * 0.9),
        t20: need(35.0, t),
        t30: need(45.0, t * 1.15),
        c50: v(-1.54),
        c80: v(2.06),
        d50: v(0.4149),
        curvature: (range >= 45.0).then_some(15.0),
    }
}

fn room() -> RoomAcoustics {
    let mut low = band(Some(62.5), 1.2, 40.0);
    low.edt = RoomValue::Refused {
        reason: RoomRefusal::FilterLimited {
            bandwidth_decay: 6.0,
        },
    };
    RoomAcoustics {
        broadband: band(None, 0.95, 58.0),
        octave: vec![
            low,
            band(Some(125.89), 1.1, 50.0),
            band(Some(1000.0), 0.9, 55.0),
            band(Some(7943.3), 0.6, 48.0),
        ],
        third: vec![band(Some(1258.9), 0.9, 50.0)],
        span_end: Seconds(0.99),
    }
}

#[test]
fn bands_are_named_by_their_nominal_frequency() {
    assert_eq!(band_name(Some(62.5)), "63");
    assert_eq!(band_name(Some(125.89)), "125");
    assert_eq!(band_name(Some(1000.0)), "1k");
    assert_eq!(band_name(Some(1258.9)), "1.25k");
    assert_eq!(band_name(Some(7943.3)), "8k");
    assert_eq!(band_name(Some(31.62)), "31.5");
    assert_eq!(band_name(None), "All");
}

#[test]
fn refused_values_are_words_and_the_legend_says_why() {
    let t = room_table(&room(), BandSet::Octave);
    assert_eq!(t.bands, ["63", "125", "1k", "8k", "All"]);
    assert_eq!(
        t.caption,
        "Room (ISO 3382-1) · octave bands · decay to 990 ms"
    );
    let row = |p: Param| {
        t.rows
            .iter()
            .find(|r| r.param == p)
            .map(|r| r.cells.iter().map(|c| c.text.clone()).collect::<Vec<_>>())
            .expect("row")
    };
    assert_eq!(row(Param::Edt), ["short", "0.99", "0.81", "0.54", "0.85"]);
    assert_eq!(row(Param::T20), ["1.20", "1.10", "0.90", "0.60", "0.95"]);
    // 40 dB of range: no T30; curved decays carry a star.
    assert_eq!(
        row(Param::T30),
        ["noise", "1.26*", "1.03*", "0.69*", "1.09*"]
    );
    assert_eq!(row(Param::C50)[0], "−1.5");
    assert_eq!(row(Param::C80)[0], "2.1");
    assert_eq!(row(Param::D50)[0], "41");
    assert_eq!(row(Param::Range), ["40", "50", "55", "48", "58"]);
    assert_eq!(
        t.legend,
        [
            "noise: the decay meets the noise too soon (EDT and C/D need 20 dB, T20 35 dB, T30 \
             45 dB of range)",
            "short: the decay is too short for the band's filter",
            "*: curved decay (T30 more than 10 % above T20)",
        ]
    );
    assert_eq!(
        refusal_text(RoomRefusal::InsufficientRange {
            range: Db(40.2),
            needed: Db(45.0)
        }),
        "the decay meets the noise after 40 dB; this needs 45 dB"
    );
    let third = room_table(&room(), BandSet::Third);
    assert_eq!(third.bands, ["1.25k", "All"]);
    assert!(third.caption.contains("⅓-octave bands"));
}

#[test]
fn plain_text_lines_up_the_columns() {
    let text = room_table(&room(), BandSet::Octave).text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "Room (ISO 3382-1) · octave bands · decay to 990 ms"
    );
    assert_eq!(lines[1], "               63    125     1k     8k    All");
    assert_eq!(lines[4], "T30 (s)     noise  1.26*  1.03*  0.69*  1.09*");
    assert!(lines.iter().any(|l| l.starts_with("short: ")));
}

fn texts(l: &Layer) -> Vec<String> {
    l.labels.iter().map(|l| l.text.clone()).collect()
}

#[test]
fn a_narrow_pane_drops_outer_bands_and_keeps_broadband() {
    let theme = Theme::dark();
    let t = room_table(&room(), BandSet::Octave);
    let wide = table_layer(&t, Rect::new(0.0, 0.0, 800.0, 300.0), &theme);
    let all = texts(&wide);
    assert!(all.contains(&"63".to_owned()) && all.contains(&"All".to_owned()));
    assert_eq!(all[0], t.caption);
    let narrow = table_layer(&t, Rect::new(0.0, 0.0, 230.0, 300.0), &theme);
    let some = texts(&narrow);
    assert!(!some.contains(&"63".to_owned()), "{some:?}");
    assert!(some.contains(&"All".to_owned()) && some.contains(&"1k".to_owned()));
    assert!(
        some[0].ends_with("bands hidden (narrow pane)"),
        "{}",
        some[0]
    );
    // Refused cells are dimmed, numbers are not.
    let cell = |s: &str| wide.labels.iter().find(|l| l.text == s).expect(s).color;
    assert_eq!(cell("short"), theme.text_dim);
    assert_eq!(cell("0.99"), theme.text);
}

#[test]
fn the_sweep_ir_view_carries_the_table_when_tall_enough() {
    use crate::banner::Status;
    use crate::view::ViewState;
    let theme = Theme::dark();
    let mut d = crate::distortion::tests::data();
    if let Some(s) = d.sweep.as_mut() {
        s.room = Some(room());
    }
    let color = theme.trace_color(0);
    let status = Status::default();
    let view = ViewState::default();
    let tall = Viewport {
        width: 900.0,
        height: 520.0,
    };
    let sc =
        crate::distortion::sweep_ir_scene(&d, color, &status, &view, &theme, tall).expect("scene");
    let t = sc.room.as_ref().expect("table");
    assert_eq!(t.bands.last().map(String::as_str), Some("All"));
    assert_eq!(sc.scene.viewport, tall);
    assert!(sc.plot.bottom() < tall.height - table_height(t, &theme));
    let short = Viewport {
        width: 900.0,
        height: 250.0,
    };
    let sc =
        crate::distortion::sweep_ir_scene(&d, color, &status, &view, &theme, short).expect("scene");
    assert!(sc.room.is_none());
}

/// The room view: the table alone, as large as the pane allows with every band shown (read
/// across a room); in a small pane the narrow-pane rules still hold; without parameters it
/// says why.
#[test]
fn the_room_view_takes_the_pane_and_grows_to_fit() {
    use crate::banner::Status;
    let theme = Theme::dark();
    let r = room();
    let full = Viewport {
        width: 1100.0,
        height: 600.0,
    };
    let s = room_scene(Some(&r), Some("Sweep 1"), &Status::default(), &theme, full);
    let t = s.table.as_ref().expect("table");
    assert!(
        t.caption
            .starts_with("Sweep 1 · Room (ISO 3382-1) · octave bands")
    );
    assert!(s.font_size > theme.font_size, "{}", s.font_size);
    assert!(table_height_at(t, s.font_size) <= s.rect.h);
    assert!(table_width_at(t, s.font_size) <= s.rect.w);
    let all: Vec<&str> = s
        .scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|x| x.text.as_str()))
        .collect();
    assert!(all.contains(&"63") && all.contains(&"All") && all.contains(&"T30 (s)"));
    assert!(!all.iter().any(|x| x.contains("hidden")), "{all:?}");
    // Every label stays inside the pane.
    for l in s.scene.layers.iter().flat_map(|l| &l.labels) {
        let right = l.pos[0] + text_width(&l.text, l.size);
        assert!(right <= full.width + 0.5, "{} at {right}", l.text);
    }
    // Small: the small font, outer bands dropped and said.
    let small = room_scene(
        Some(&r),
        Some("Sweep 1"),
        &Status::default(),
        &theme,
        Viewport {
            width: 260.0,
            height: 300.0,
        },
    );
    assert_eq!(small.font_size, theme.small_font_size);
    let some: Vec<&str> = small
        .scene
        .layers
        .iter()
        .flat_map(|l| l.labels.iter().map(|x| x.text.as_str()))
        .collect();
    assert!(
        some.iter()
            .any(|x| x.ends_with("bands hidden (narrow pane)")),
        "{some:?}"
    );
    // No parameters, no sweep: the note says why.
    let none = room_scene(None, Some("Sweep 2"), &Status::default(), &theme, full);
    assert_eq!(
        none.note.as_deref(),
        Some("Sweep 2: no room parameters (a sweep with silence after it measures them)")
    );
    let nothing = room_scene(None, None, &Status::default(), &theme, full);
    assert_eq!(
        nothing.note.as_deref(),
        Some("no sweep results yet: Shift+S sets one up")
    );
}

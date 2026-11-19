use super::*;
use crate::keys::{Keymap, LabelStyle};

/// Every pane's line at every width from 300 to 1920 px, measured with egui's fonts:
/// nothing drawn past the strip, no two hints overlapping, the help key always there,
/// and the whole line once the pane is wide enough.
#[test]
fn hint_lines_fit_every_width() {
    let ctx = egui::Context::default();
    crate::app::install_fonts(&ctx);
    let ch = crate::theme::chrome(&Theme::dark());
    let keymap = Keymap::default();
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        let ctx = ui.ctx();
        for scope in Scope::ALL {
            for style in [LabelStyle::Pc, LabelStyle::Mac] {
                let items = hints::line(&keymap, scope, style);
                let (galleys, sep) = hint_galleys(ctx, &items, &ch);
                let widths: Vec<(u8, f32)> = items
                    .iter()
                    .zip(&galleys)
                    .map(|(h, g)| (h.priority, g.size().x))
                    .collect();
                let sep = sep.size().x;
                let mut shown_before = 0;
                for w in (300..=1920).step_by(10) {
                    let w = w as f32;
                    let placed = place_hints(&widths, sep, 0.0, w);
                    let help = items.len() - 1;
                    assert_eq!(placed.last().map(|p| p.0), Some(help), "{scope:?} {w}");
                    for pair in placed.windows(2) {
                        let ((i, x), (_, next)) = (pair[0], pair[1]);
                        assert!(x + widths[i].1 + sep <= next + 0.01, "{scope:?} {w}");
                    }
                    let (i, x) = placed[placed.len() - 1];
                    assert!(x + widths[i].1 <= w - HINT_PAD + 0.01, "{scope:?} {w}");
                    // Wider never shows fewer.
                    assert!(placed.len() >= shown_before, "{scope:?} {w}");
                    shown_before = placed.len();
                }
                assert_eq!(shown_before, items.len(), "{scope:?}: all at 1920 px");
                // At the narrowest the most used still shows beside the help key.
                let narrow = place_hints(&widths, sep, 0.0, 300.0);
                let want = if scope == Scope::Global { 1 } else { 3 };
                assert!(narrow.len() >= want, "{scope:?} {style:?}: {narrow:?}");
            }
        }
    });
    out.textures_delta.clear();
}

/// The spectrum over its own spectrograph, as two panes of one column (and the transfer
/// pane below them): one frequency at one x in each, so the shared cursor, the 1 kHz line
/// and a click in the spectrograph all land where the spectrum draws them. A spectrum pane
/// alone keeps its narrow margin.
#[test]
fn stacked_frequency_panes_share_their_x() {
    use crate::state::{AppState, Layout, PaneNode, View};
    use ac2_scene::freq_column::{FreqPlot, natural_right};
    use std::collections::BTreeMap;
    let theme = Theme::dark();
    let now = crate::view::now();
    let area = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(1000.0, 900.0));
    let leaf = |n| Box::new(PaneNode::Leaf(PaneId(n)));
    let mut st = AppState::default();
    st.pane_area = (area.width(), area.height());
    st.view.cursor_hz = Some(744.0);
    // The plot inside a pane, as `pane` cuts it: the frame's inset and the title strip.
    let vp = |r: egui::Rect| Viewport::new(r.width() - 4.0, r.height() - TITLE_H - 2.0);
    let alone = scenes::spectrum(&st, PaneId(1), &theme, vp(layout(&st, area)[0].1), now);
    let own = natural_right(FreqPlot::Spectrum, &ac2_scene::view::ViewState::default());
    assert_eq!(st.column_right(PaneId(1)), own);

    st.layout = Layout::of(
        PaneNode::Split {
            axis: Axis::Column,
            ratio: 0.66,
            a: Box::new(PaneNode::Split {
                axis: Axis::Column,
                ratio: 0.5,
                a: leaf(1),
                b: leaf(2),
            }),
            b: leaf(3),
        },
        BTreeMap::from([
            (PaneId(1), View::of(PaneKind::Spectrum)),
            (PaneId(2), sg_view()),
            (PaneId(3), View::of(PaneKind::Transfer)),
        ]),
        PaneId(1),
    );
    let rects = layout(&st, area);
    let rect = |id| rects.iter().find(|(p, _)| *p == id).expect("laid out").1;
    let sp = scenes::spectrum(&st, PaneId(1), &theme, vp(rect(PaneId(1))), now);
    let sg = scenes::spectrograph(&st, PaneId(2), &theme, vp(rect(PaneId(2))), now);
    let tf = scenes::transfer(&st, PaneId(3), &theme, vp(rect(PaneId(3))), now);
    let tick_1k = |a: &ac2_scene::axis::Axis| {
        a.ticks
            .iter()
            .find(|t| t.value == 1000.0)
            .map(|t| t.pos)
            .expect("1 kHz tick")
    };
    for (name, x) in [("spectrum", &sp.x_axis), ("transfer", &tf.x_axis)] {
        let (m, g) = (x.mapping, sg.x_axis.mapping);
        assert!(
            (m.to_px(744.0) - g.to_px(744.0)).abs() < 0.5,
            "{name} cursor"
        );
        assert!(
            (tick_1k(x) - tick_1k(&sg.x_axis)).abs() < 0.5,
            "{name} 1 kHz"
        );
        // A click at x in the spectrograph sets the frequency the other draws at x.
        let click_x = 431.0;
        assert!(
            (m.to_px(g.from_px(click_x)) - click_x).abs() < 0.5,
            "{name} click"
        );
    }
    assert!(
        sp.plot.right() < alone.plot.right(),
        "room for the colour bar"
    );

    // Side by side, each keeps its own.
    st.layout = Layout::of(
        PaneNode::Split {
            axis: Axis::Row,
            ratio: 0.5,
            a: leaf(1),
            b: leaf(2),
        },
        BTreeMap::from([
            (PaneId(1), View::of(PaneKind::Spectrum)),
            (PaneId(2), sg_view()),
        ]),
        PaneId(1),
    );
    assert_eq!(st.column_right(PaneId(1)), own);
}

fn sg_view() -> crate::state::View {
    let mut v = crate::state::View::of(PaneKind::Spectrum);
    v.modes.spectrum = ac2_scene::view::SpectrumMode::Spectrograph;
    v
}

/// One pane fills the area; two splits give a left half and a right half split top
/// and bottom, with the gap between and nothing past the area.
#[test]
fn layout_fills_the_area_with_the_tree() {
    use crate::state::{AppState, Msg};
    let keys = Keymap::default();
    let area = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(1000.0, 600.0));
    let mut st = AppState::default();
    let one = layout(&st, area);
    assert_eq!(one, vec![(st.layout.focus, area)]);
    st.pane_area = (1000.0, 600.0);
    for _ in 0..2 {
        st.update(Msg::Command(crate::keys::CommandId::SplitPane), &keys);
    }
    let l = layout(&st, area);
    assert_eq!(l.len(), 3);
    let half = (1000.0 - GAP) / 2.0;
    assert!((l[0].1.width() - half).abs() < 1e-3 && l[0].1.height() == 600.0);
    assert!((l[1].1.height() - (600.0 - GAP) / 2.0).abs() < 1e-3);
    assert!((l[1].1.min.x - (10.0 + half + GAP)).abs() < 1e-3);
    for (_, r) in &l {
        assert!(area.contains_rect(*r), "{r:?}");
    }
}

/// Each button of the walk pane's bar sends the command titled as the button reads: one
/// name for an action on the pane, in the palette and in `keys.toml`'s reference.
#[test]
fn walk_bar_buttons_are_their_commands() {
    use ac2_scene::walk::controls::Action;
    let mut seen = Vec::new();
    for a in Action::ALL {
        let c = crate::view::walk::action_command(a);
        assert_eq!(c.title(), a.label(), "{a:?}");
        assert!(a.label().starts_with(a.short()) || a == Action::Swap);
        assert_eq!(c.scopes(), [Scope::Walk]);
        seen.push(c);
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), Action::ALL.len());
}

/// Ctrl+wheel zooms the level axis about the level under the pointer only while the
/// pointer is on that axis; anywhere else in the pane (a margin, the banners, under the
/// plot) it zooms about the middle of the range, never about a level extrapolated from
/// beyond the axis.
#[test]
fn level_zoom_is_about_the_pointer_only_on_the_level_axis() {
    use ac2_scene::axis::{Range, Scale};
    // A magnitude plot from y = 40 (+30 dB) down to y = 340 (−30 dB).
    let m = Mapping::new(Range::new(-30.0, 30.0), Scale::Linear, 340.0, 40.0);
    let about = |y: f32| zoom_about_db(Some(m), Some(y));
    assert_eq!(about(190.0), Some(0.0));
    assert_eq!(about(40.0), Some(30.0));
    assert_eq!(about(340.0), Some(-30.0));
    // Under the plot the mapping would say −92 dB; over it, +37 dB.
    assert_eq!(m.from_px(650.0), -92.0);
    assert_eq!(about(650.0), None);
    assert_eq!(about(5.0), None);
    // No pointer, or a view without a level axis mapping (a 3D spectrograph).
    assert_eq!(zoom_about_db(Some(m), None), None);
    assert_eq!(zoom_about_db(None, Some(190.0)), None);
    // A log axis is percent of the fundamental: 1 % is −40 dB.
    let pct = Mapping::new(Range::new(0.01, 100.0), Scale::Log, 400.0, 0.0);
    let db = zoom_about_db(Some(pct), Some(200.0)).expect("on the axis");
    assert!((db + 40.0).abs() < 1e-9, "{db}");
    assert_eq!(zoom_about_db(Some(pct), Some(401.0)), None);
}

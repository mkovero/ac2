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

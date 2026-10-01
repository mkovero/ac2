//! egui chrome styled from the same `ac2-scene` [`Theme`] the plots use, so panels, text and
//! plots share one palette per theme (dark default, light, high contrast).

use ac2_scene::primitives::Color;
use ac2_scene::theme::{Theme, ThemeName};
use eframe::egui::{self, Color32, Stroke, Visuals};

pub fn c32(c: Color) -> Color32 {
    let [r, g, b, a] = c.to_rgba8();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Chrome colours not in the plot theme.
#[derive(Clone, Copy, Debug)]
pub struct Chrome {
    pub panel: Color32,
    pub raised: Color32,
    pub border: Color32,
    pub focus: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub fault: Color32,
    pub armed: Color32,
}

pub fn chrome(t: &Theme) -> Chrome {
    let panel = c32(t.background);
    let raised = c32(t.plot_background);
    let border = c32(t.grid_major.color);
    let (ok, armed) = match t.name {
        ThemeName::Light => (
            Color32::from_rgb(0x00, 0x84, 0x63),
            Color32::from_rgb(0xb0, 0x78, 0x00),
        ),
        _ => (
            Color32::from_rgb(0x00, 0xc0, 0x8b),
            Color32::from_rgb(0xe6, 0x9f, 0x00),
        ),
    };
    Chrome {
        panel,
        raised,
        border,
        focus: c32(t.traces[0]),
        text: c32(t.text),
        dim: c32(t.text_dim),
        ok,
        warn: c32(t.banner_warning.background),
        fault: c32(t.banner_fault.background),
        armed,
    }
}

/// egui visuals for `t`.
pub fn visuals(t: &Theme) -> Visuals {
    let ch = chrome(t);
    let mut v = match t.name {
        ThemeName::Light => Visuals::light(),
        ThemeName::Dark | ThemeName::HighContrast => Visuals::dark(),
    };
    v.panel_fill = ch.panel;
    v.window_fill = ch.raised;
    v.extreme_bg_color = ch.raised;
    v.faint_bg_color = ch.raised;
    v.override_text_color = Some(ch.text);
    v.window_stroke = Stroke::new(1.0, ch.border);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, ch.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, ch.text);
    v.selection.bg_fill = ch.focus.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, ch.focus);
    if t.name == ThemeName::HighContrast {
        for w in [
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
        ] {
            w.fg_stroke = Stroke::new(1.5, ch.text);
            w.bg_stroke = Stroke::new(1.0, ch.text);
        }
    }
    v
}

pub fn apply(ctx: &egui::Context, t: &Theme) {
    ctx.set_visuals(visuals(t));
    let size = t.font_size + 1.0;
    ctx.global_style_mut(|s| {
        for (style, font) in s.text_styles.iter_mut() {
            font.size = match style {
                egui::TextStyle::Small => t.small_font_size,
                egui::TextStyle::Heading => size + 4.0,
                _ => size,
            };
        }
    });
}

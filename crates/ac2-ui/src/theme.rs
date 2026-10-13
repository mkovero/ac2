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
///
/// `ok`, `armed`, `warn` and `fault` are foregrounds: text and strokes drawn on `panel` or
/// `raised`, so each keeps WCAG AA contrast (4.5:1) against both. A banner's fill is a
/// background with its own text colour and never doubles as a foreground: the light theme's
/// amber fill is far too pale to read as text on a light panel.
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
    /// Warning banner fill (an armed badge), read with `on_warn_fill`.
    pub warn_fill: Color32,
    pub on_warn_fill: Color32,
    /// Fault banner fill (a firing badge, the Stop button), read with `on_fault_fill`.
    pub fault_fill: Color32,
    pub on_fault_fill: Color32,
}

pub fn chrome(t: &Theme) -> Chrome {
    let panel = c32(t.background);
    let raised = c32(t.plot_background);
    let border = c32(t.grid_major.color);
    let rgb = |v: u32| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
    let (ok, armed, warn, fault) = match t.name {
        ThemeName::Light => (
            rgb(0x00_78_5a),
            rgb(0x8a_5a_00),
            rgb(0x8a_5a_00),
            c32(t.banner_fault.background),
        ),
        ThemeName::Dark => (
            rgb(0x00_c0_8b),
            rgb(0xe6_9f_00),
            c32(t.banner_warning.background),
            rgb(0xef_5b_4f),
        ),
        ThemeName::HighContrast => (
            rgb(0x00_c0_8b),
            rgb(0xe6_9f_00),
            c32(t.banner_warning.background),
            c32(t.banner_fault.background),
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
        warn,
        fault,
        armed,
        warn_fill: c32(t.banner_warning.background),
        on_warn_fill: c32(t.banner_warning.text),
        fault_fill: c32(t.banner_fault.background),
        on_fault_fill: c32(t.banner_fault.text),
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

/// No shadows under popups and windows: each is a feathered mesh as large as its window
/// plus the blur, filled pixel by pixel, which a CPU rasteriser pays for on every frame.
pub fn drop_shadows(ctx: &egui::Context) {
    ctx.global_style_mut(|s| {
        s.visuals.window_shadow = egui::Shadow::NONE;
        s.visuals.popup_shadow = egui::Shadow::NONE;
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 2.x relative luminance of an sRGB colour.
    fn luminance(c: Color32) -> f64 {
        let lin = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// Every chrome foreground the views draw text in reads at AA (4.5:1) on both surfaces
    /// labels sit on, in every theme; and the fault button's text reads on its fill.
    #[test]
    fn chrome_text_reads_on_panel_and_raised_in_every_theme() {
        for t in [Theme::dark(), Theme::light(), Theme::high_contrast()] {
            let ch = chrome(&t);
            let fgs = [
                ("text", ch.text),
                ("dim", ch.dim),
                ("ok", ch.ok),
                ("armed", ch.armed),
                ("warn", ch.warn),
                ("fault", ch.fault),
            ];
            for (name, fg) in fgs {
                for (surface, bg) in [("panel", ch.panel), ("raised", ch.raised)] {
                    let r = contrast(fg, bg);
                    assert!(r >= 4.5, "{:?}: {name} on {surface} is {r:.2}:1", t.name);
                }
            }
            for (name, ink, fill) in [
                ("warning", ch.on_warn_fill, ch.warn_fill),
                ("fault", ch.on_fault_fill, ch.fault_fill),
            ] {
                let r = contrast(ink, fill);
                assert!(
                    r >= 4.5,
                    "{:?}: text on the {name} fill is {r:.2}:1",
                    t.name
                );
            }
        }
    }

    /// The light theme's warning text is not the banner's pale amber fill.
    #[test]
    fn light_warning_text_is_not_the_banner_fill() {
        let t = Theme::light();
        assert_ne!(chrome(&t).warn, c32(t.banner_warning.background));
    }
}

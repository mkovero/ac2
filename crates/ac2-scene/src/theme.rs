//! Themes: every colour, width and text size the builders use, as data.
//!
//! Trace palettes are based on Okabe & Ito's colour-blind-safe set (orange, sky blue,
//! bluish green, yellow, blue, vermillion, reddish purple, black/grey), reordered and, for
//! the light theme, darkened so every trace keeps at least 3:1 contrast against the plot
//! background (WCAG non-text contrast) — tested below.

use crate::primitives::{Color, Colormap, Stroke};

/// How many measurement colour families a theme has before they repeat.
pub const FAMILIES: usize = 8;

/// Which built-in theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeName {
    Dark,
    Light,
    /// Black background, white text, thicker traces: for sunlight.
    HighContrast,
}

/// Banner colours for one severity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BannerColors {
    pub background: Color,
    pub text: Color,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: ThemeName,
    /// Behind everything (margins, axis labels).
    pub background: Color,
    /// Inside the plot rectangle.
    pub plot_background: Color,
    pub grid_major: Stroke,
    pub grid_minor: Stroke,
    /// The 0 dB / 0° / t = 0 reference line.
    pub zero_line: Stroke,
    pub cursor: Stroke,
    pub axis_text: Color,
    pub text: Color,
    pub text_dim: Color,
    pub banner_fault: BannerColors,
    pub banner_warning: BannerColors,
    pub banner_info: BannerColors,
    /// Categorical colours for things that are not a measurement's curves (harmonic
    /// orders, Leq columns, the focus ring).
    pub traces: [Color; 8],
    /// Base colour of each measurement's colour family ([`crate::families`]): one hue
    /// apiece, far enough apart in OKLCH hue that a family's lighter and darker shades do
    /// not run into the next family. The first six are Okabe & Ito's hues, which stay
    /// apart under the common colour-vision deficiencies; violet and cyan extend the set.
    /// The lightness of each is chosen (per theme) so the six stay apart in simulated
    /// protan and deutan vision too, where hue alone collapses (tested in `families`).
    pub families: [Color; FAMILIES],
    /// Base of the neutral family (grey): imported traces, which belong to no measurement.
    pub neutral: Color,
    /// OKLab lightness range a family's shades stay in: inside it every shade keeps 3:1
    /// contrast against the plot and still shows its hue (not washed to white or black).
    pub shade_lightness: (f64, f64),
    pub trace_width: f32,
    /// Opacity multiplier of a STALE trace (decision 2a: stale traces dim).
    pub stale_alpha: f32,
    /// Opacity of RTA bars (lines on top stay opaque).
    pub bar_alpha: f32,
    /// Opacity of the peak-hold overlay.
    pub peak_alpha: f32,
    pub font_size: f32,
    pub small_font_size: f32,
    /// SPL big number.
    pub big_font_size: f32,
    /// A level judged within its limit (the Leq columns' bars).
    pub level_ok: Color,
    /// Level → colour of the spectrograph: perceptually uniform, so equal steps in dB look
    /// like equal steps, and readable with the common colour-vision deficiencies.
    pub colormap: Colormap,
}

fn hex(v: u32) -> Color {
    Color::from_rgba8([(v >> 16) as u8, (v >> 8) as u8, v as u8, 255])
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            name: ThemeName::Dark,
            background: hex(0x0d0f12),
            plot_background: hex(0x14171c),
            grid_major: Stroke::solid(hex(0x3a4049), 1.0),
            grid_minor: Stroke::solid(hex(0x23272e), 1.0),
            zero_line: Stroke::solid(hex(0x5a626e), 1.0),
            cursor: Stroke::solid(hex(0xd8dce2).with_alpha(0.7), 1.0),
            axis_text: hex(0x9aa3ad),
            text: hex(0xe6e9ed),
            text_dim: hex(0x8a929c),
            banner_fault: BannerColors {
                background: hex(0xc0392b),
                text: hex(0xffffff),
            },
            banner_warning: BannerColors {
                background: hex(0xe69f00),
                text: hex(0x000000),
            },
            banner_info: BannerColors {
                background: hex(0x3a4a5e),
                text: hex(0xffffff),
            },
            traces: [
                hex(0x56b4e9), // sky blue
                hex(0xe69f00), // orange
                hex(0x009e73), // bluish green
                hex(0xf0e442), // yellow
                hex(0xd55e00), // vermillion
                hex(0xcc79a7), // reddish purple
                hex(0x3d8fd1), // blue, lightened from 0072b2 for contrast on dark
                hex(0xbbbbbb), // grey in place of black
            ],
            families: [
                hex(0x1cb1ff), // sky blue
                hex(0xdf9900), // orange
                hex(0x2eefb1), // bluish green
                hex(0xc7519a), // reddish purple
                hex(0xe6da39), // yellow
                hex(0xc45000), // vermillion
                hex(0x8562d4), // violet
                hex(0x00818c), // cyan
            ],
            neutral: hex(0xbbbbbb),
            shade_lightness: (0.45, 0.92),
            trace_width: 1.6,
            stale_alpha: 0.35,
            bar_alpha: 0.55,
            peak_alpha: 0.8,
            font_size: 12.0,
            small_font_size: 10.5,
            big_font_size: 64.0,
            level_ok: hex(0x2fa66a),
            colormap: Colormap::Viridis,
        }
    }

    pub fn light() -> Self {
        Self {
            name: ThemeName::Light,
            background: hex(0xeceef1),
            plot_background: hex(0xffffff),
            grid_major: Stroke::solid(hex(0xc3c8cf), 1.0),
            grid_minor: Stroke::solid(hex(0xe6e8ec), 1.0),
            zero_line: Stroke::solid(hex(0x8d949e), 1.0),
            cursor: Stroke::solid(hex(0x22262b).with_alpha(0.7), 1.0),
            axis_text: hex(0x4b525b),
            text: hex(0x16191d),
            text_dim: hex(0x5f6670),
            banner_fault: BannerColors {
                background: hex(0xb3261e),
                text: hex(0xffffff),
            },
            banner_warning: BannerColors {
                background: hex(0xe69f00),
                text: hex(0x000000),
            },
            banner_info: BannerColors {
                background: hex(0x44546a),
                text: hex(0xffffff),
            },
            traces: [
                hex(0x0072b2), // blue
                hex(0xd55e00), // vermillion
                hex(0x008463), // bluish green, darkened
                hex(0xb07800), // orange, darkened
                hex(0xb85c94), // reddish purple, darkened
                hex(0x2f86b8), // sky blue, darkened
                hex(0x000000), // black
                hex(0x857a00), // yellow, darkened to olive
            ],
            families: [
                hex(0x0089c9), // blue
                hex(0x916200), // orange, darkened
                hex(0x008761), // bluish green, darkened
                hex(0x95216e), // reddish purple, darkened
                hex(0x968d00), // yellow, darkened to olive
                hex(0x883500), // vermillion, darkened
                hex(0x8c68dc), // violet
                hex(0x005e66), // cyan, darkened to teal
            ],
            neutral: hex(0x6b6b6b),
            shade_lightness: (0.30, 0.70),
            trace_width: 1.6,
            stale_alpha: 0.35,
            bar_alpha: 0.5,
            peak_alpha: 0.8,
            font_size: 12.0,
            small_font_size: 10.5,
            big_font_size: 64.0,
            level_ok: hex(0x1b7a45),
            colormap: Colormap::Viridis,
        }
    }

    pub fn high_contrast() -> Self {
        Self {
            name: ThemeName::HighContrast,
            background: hex(0x000000),
            plot_background: hex(0x000000),
            grid_major: Stroke::solid(hex(0x8c8c8c), 1.0),
            grid_minor: Stroke::solid(hex(0x404040), 1.0),
            zero_line: Stroke::solid(hex(0xd0d0d0), 1.5),
            cursor: Stroke::solid(hex(0xffffff), 1.5),
            axis_text: hex(0xffffff),
            text: hex(0xffffff),
            text_dim: hex(0xd0d0d0),
            banner_fault: BannerColors {
                background: hex(0xff3b30),
                text: hex(0x000000),
            },
            banner_warning: BannerColors {
                background: hex(0xffd60a),
                text: hex(0x000000),
            },
            banner_info: BannerColors {
                background: hex(0xffffff),
                text: hex(0x000000),
            },
            traces: [
                hex(0x56b4e9),
                hex(0xffb000),
                hex(0x00c08b),
                hex(0xf0e442),
                hex(0xff6a1a),
                hex(0xe58ac0),
                hex(0x5aa9ff),
                hex(0xffffff),
            ],
            families: [
                hex(0x00a7f4),
                hex(0xde9800),
                hex(0x2aedaf),
                hex(0xd15ba4),
                hex(0xe3d734),
                hex(0xcf5605),
                hex(0xa07ef4),
                hex(0x6ef1ff),
            ],
            neutral: hex(0xffffff),
            shade_lightness: (0.45, 0.95),
            trace_width: 2.6,
            stale_alpha: 0.45,
            bar_alpha: 0.7,
            peak_alpha: 1.0,
            font_size: 14.0,
            small_font_size: 12.0,
            big_font_size: 72.0,
            level_ok: hex(0x00e676),
            colormap: Colormap::Viridis,
        }
    }

    pub fn by_name(name: ThemeName) -> Self {
        match name {
            ThemeName::Dark => Self::dark(),
            ThemeName::Light => Self::light(),
            ThemeName::HighContrast => Self::high_contrast(),
        }
    }

    /// Colour for the `i`-th live trace (wraps around the palette).
    pub fn trace_color(&self, i: usize) -> Color {
        self.traces[i % self.traces.len()]
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// WCAG relative luminance of a display-encoded (sRGB) colour.
pub fn relative_luminance(c: Color) -> f64 {
    let lin = |v: f32| {
        let v = f64::from(v);
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// WCAG contrast ratio between two opaque colours (1…21).
pub fn contrast_ratio(a: Color, b: Color) -> f64 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> [Theme; 3] {
        [Theme::dark(), Theme::light(), Theme::high_contrast()]
    }

    #[test]
    fn traces_stand_out_from_the_plot() {
        for t in all() {
            for (i, c) in t.traces.iter().enumerate() {
                let r = contrast_ratio(*c, t.plot_background);
                assert!(r >= 3.0, "{:?} trace {i}: contrast {r:.2}", t.name);
            }
        }
    }

    #[test]
    fn text_is_readable() {
        for t in all() {
            for (what, c, min) in [
                ("text", t.text, 7.0),
                ("axis", t.axis_text, 4.5),
                ("dim", t.text_dim, 4.5),
            ] {
                let r = contrast_ratio(c, t.background);
                assert!(r >= min, "{:?} {what}: {r:.2}", t.name);
            }
            for (what, b) in [
                ("fault", t.banner_fault),
                ("warning", t.banner_warning),
                ("info", t.banner_info),
            ] {
                let r = contrast_ratio(b.text, b.background);
                assert!(r >= 4.5, "{:?} banner {what}: {r:.2}", t.name);
            }
        }
    }

    #[test]
    fn level_ok_stands_out_from_the_plot() {
        for t in all() {
            let r = contrast_ratio(t.level_ok, t.plot_background);
            assert!(r >= 3.0, "{:?} level_ok: contrast {r:.2}", t.name);
        }
    }

    #[test]
    fn family_bases_stand_out_from_the_plot() {
        for t in all() {
            for (i, c) in t.families.iter().chain([&t.neutral]).enumerate() {
                let r = contrast_ratio(*c, t.plot_background);
                assert!(r >= 3.0, "{:?} family {i}: contrast {r:.2}", t.name);
            }
        }
    }

    #[test]
    fn palette_colours_are_distinct() {
        for t in all() {
            for i in 0..t.traces.len() {
                for j in i + 1..t.traces.len() {
                    assert_ne!(t.traces[i], t.traces[j], "{:?} {i} {j}", t.name);
                }
            }
        }
    }

    #[test]
    fn high_contrast_is_heavier() {
        assert!(Theme::high_contrast().trace_width > Theme::dark().trace_width);
        assert_eq!(Theme::default(), Theme::dark());
        assert_eq!(Theme::by_name(ThemeName::Light), Theme::light());
    }
}

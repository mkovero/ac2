//! The top bar's middle segment: the session line and the health meter, in the room
//! between the bar's two groups, scrolling through it when wider.

use ac2_scene::health::HealthLine;
use eframe::egui::{self, Color32, RichText};

use super::chrome::{BAR_GAP, text_width};
use crate::app::App;
use crate::marquee::Marquee;
use crate::theme::Chrome;

/// Narrower than this, a scrolling segment shows too little at once to follow: the bar
/// shortens or drops other items first.
pub(super) const SEGMENT_MIN_W: f32 = 200.0;
/// The fade over the text where the segment's edge cuts it.
const FADE_W: f32 = 16.0;

/// One piece of text at `x` from its part's start.
struct Piece {
    text: egui::WidgetText,
    x: f32,
}

/// One part of the segment, with its own tooltip: its pieces and the width it takes.
pub(super) struct Part {
    pieces: Vec<Piece>,
    width: f32,
    /// What a screen reader (and a test) reads for it.
    label: String,
    hover: Option<String>,
}

impl Part {
    pub(super) fn single(ui: &egui::Ui, text: egui::WidgetText, hover: Option<String>) -> Self {
        Self {
            width: text_width(ui, &text),
            label: text.text().to_owned(),
            pieces: vec![Piece { text, x: 0.0 }],
            hover,
        }
    }
}

/// What a placed span of the health meter is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Span {
    Sep,
    Label,
    Value,
    Unit,
}

#[derive(Clone, Debug, PartialEq)]
struct Placed {
    span: Span,
    text: String,
    x: f32,
    warn: bool,
}

/// Places the meter's spans given each text's `width`: every number right-aligned in a slot
/// as wide as its widest expected text (each digit as wide as the font's widest), every
/// unit in a slot as wide as its widest, so a figure's digits changing moves nothing after
/// it. Returns the spans and the meter's width.
fn lay_out_health(line: &HealthLine, width: impl Fn(&str) -> f32) -> (Vec<Placed>, f32) {
    let digit = (b'0'..=b'9')
        .map(|d| width(&char::from(d).to_string()))
        .fold(0.0, f32::max);
    // A lone space may measure as nothing: the space is what it adds between two glyphs.
    let space = (width("x x") - width("xx")).max(0.0);
    let slot = |s: &str| -> f32 {
        s.chars()
            .map(|c| {
                if c.is_ascii_digit() {
                    digit
                } else {
                    width(&c.to_string())
                }
            })
            .sum()
    };
    let mut out = Vec::new();
    let mut x = 0.0;
    for (k, p) in line.parts.iter().enumerate() {
        let mut put = |span, text: &str, at: f32| {
            out.push(Placed {
                span,
                text: text.to_owned(),
                x: at,
                warn: p.warn && span != Span::Sep,
            });
        };
        let sep = if k == 0 { "· " } else { " · " };
        put(Span::Sep, sep, x);
        x += width(sep);
        if !p.label.is_empty() {
            put(Span::Label, &p.label, x);
            x += width(&p.label) + space;
        }
        let vw = width(&p.value);
        let sw = slot(&p.value_slot).max(vw);
        put(Span::Value, &p.value, x + sw - vw);
        x += sw + space;
        let uw = p
            .unit_slot
            .iter()
            .map(|u| width(u))
            .fold(width(&p.unit), f32::max);
        put(Span::Unit, &p.unit, x);
        x += uw;
    }
    (out, x)
}

/// The health meter after the session line: its figures, warnings in the warning colour.
pub(super) fn health_part(
    app: &App,
    ui: &egui::Ui,
    ch: &Chrome,
    now: std::time::Instant,
) -> Option<Part> {
    let st = &app.state;
    // A daemon that stopped answering has no current figures.
    let daemon = st
        .rig
        .mirror
        .as_ref()
        .filter(|m| m.responding(now))
        .and_then(|m| m.health.as_ref());
    let wanted_fps = 1.0 / st.display_period().as_secs_f64();
    let inputs = app.health.inputs(
        daemon,
        app.link_embedded,
        app.daemon_addr.as_deref(),
        wanted_fps,
        now,
    );
    let line = ac2_scene::health::health_line(&inputs)?;
    let (placed, width) = lay_out_health(&line, |s| {
        text_width(ui, &egui::WidgetText::from(RichText::new(s)))
    });
    let pieces = placed
        .into_iter()
        .map(|p| Piece {
            text: RichText::new(p.text)
                .color(if p.warn { ch.warn } else { ch.dim })
                .into(),
            x: p.x,
        })
        .collect();
    Some(Part {
        pieces,
        width,
        label: format!("· {}", line.text()),
        hover: Some(line.detail),
    })
}

pub(super) fn segment_width(parts: &[Part]) -> f32 {
    let texts: f32 = parts.iter().map(|p| p.width).sum();
    texts + BAR_GAP * parts.len().saturating_sub(1) as f32
}

/// Draws the segment's parts in `room`: still when they fit, otherwise scrolled by
/// `marquee` and clipped at the room's edges (faded into `fade`, the bar's fill). Each part
/// is a label over its visible stretch, with its tooltip. Returns when to draw next.
pub(super) fn draw_segment(
    ui: &mut egui::Ui,
    room: egui::Rect,
    parts: &[Part],
    marquee: &mut Marquee,
    fade: Color32,
) -> Option<f64> {
    let text_w = segment_width(parts);
    let hovered = ui.rect_contains_pointer(room);
    let now = ui.input(|i| i.time);
    let f = marquee.frame(now, text_w, room.width(), hovered);
    let painter = ui.painter_at(room);
    let id = ui.id().with("session-segment");
    let mut x = room.left() - f.offset;
    for (k, p) in parts.iter().enumerate() {
        let mut height: f32 = 0.0;
        for piece in &p.pieces {
            let g = piece.text.clone().into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            );
            height = height.max(g.size().y);
            let at = egui::pos2(x + piece.x, room.center().y - g.size().y / 2.0);
            painter.galley(at, g, ui.visuals().text_color());
        }
        let r = egui::Rect::from_min_size(
            egui::pos2(x, room.center().y - height / 2.0),
            egui::vec2(p.width, height),
        );
        x = r.right() + BAR_GAP;
        let shown = r.intersect(room);
        if shown.width() < 0.5 {
            continue;
        }
        let resp = ui.interact(shown, id.with(k), egui::Sense::hover());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &p.label));
        if let Some(h) = &p.hover {
            resp.on_hover_text(h);
        }
    }
    let overflow = text_w - room.width();
    if f.offset > 0.0 {
        fade_edge(&painter, room, true, fade);
    }
    if overflow > 0.0 && f.offset < overflow {
        fade_edge(&painter, room, false, fade);
    }
    f.wake
}

/// A strip at the room's left (or right) edge, `c` at the edge and clear inwards.
fn fade_edge(painter: &egui::Painter, room: egui::Rect, left: bool, c: Color32) {
    let w = FADE_W.min(room.width() / 4.0);
    let (strip, l, r) = if left {
        (
            egui::Rect::from_min_size(room.min, egui::vec2(w, room.height())),
            c,
            Color32::TRANSPARENT,
        )
    } else {
        (
            egui::Rect::from_min_max(egui::pos2(room.right() - w, room.top()), room.max),
            Color32::TRANSPARENT,
            c,
        )
    };
    let mut m = egui::Mesh::default();
    m.colored_vertex(strip.left_top(), l);
    m.colored_vertex(strip.right_top(), r);
    m.colored_vertex(strip.right_bottom(), r);
    m.colored_vertex(strip.left_bottom(), l);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    painter.add(m);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac2_proto::frame::{AudioHealth, DaemonHealth};
    use ac2_scene::health::{HealthInputs, health_line};

    /// A proportional font: digits of different widths, as in the UI's.
    fn width(s: &str) -> f32 {
        s.chars()
            .map(|c| match c {
                '1' => 4.0,
                '0' | '8' => 8.0,
                c if c.is_ascii_digit() => 7.0,
                ' ' => 3.0,
                '.' | '·' => 3.0,
                'M' | 'm' | 'w' => 10.0,
                _ => 6.0,
            })
            .sum()
    }

    fn line(cpu: f64, local: f64, dsp: f64, xruns: u64, rate: f64, fps: f64) -> HealthLine {
        let d = DaemonHealth {
            host: "pupu".into(),
            cpu: Some(cpu),
            audio: Some(AudioHealth {
                load: Some(dsp),
                peak: None,
                session_load: None,
                session_peak: None,
                xruns,
            }),
        };
        let i = HealthInputs {
            daemon: Some(&d),
            embedded: false,
            local_host: "ketunkolo",
            daemon_addr: Some("192.168.9.27"),
            local_cpu: Some(local),
            received_per_s: Some(rate),
            frames: Some(ac2_scene::health::UiFrames {
                fps,
                longest_gap_s: 0.02,
                streaming: true,
                wanted_fps: 60.0,
            }),
            since_xrun_s: None,
            rate_unit: None,
        };
        health_line(&i).expect("figures")
    }

    #[test]
    fn changing_digits_move_no_label_and_keep_the_width() {
        let (a, wa) = lay_out_health(&line(0.09, 0.41, 0.01, 0, 1.8e6, 9.0), width);
        let (b, wb) = lay_out_health(&line(1.0, 0.18, 0.88, 12, 34e3, 60.0), width);
        assert_eq!(wa, wb);
        assert_eq!(a.len(), b.len());
        for (pa, pb) in a.iter().zip(&b) {
            assert_eq!(pa.span, pb.span);
            match pa.span {
                // Every number ends where its slot does.
                Span::Value => assert_eq!(pa.x + width(&pa.text), pb.x + width(&pb.text)),
                _ => assert_eq!(pa.x, pb.x, "{pa:?} {pb:?}"),
            }
        }
        let texts: Vec<&str> = b.iter().map(|p| p.text.as_str()).collect();
        assert!(texts.contains(&"100") && texts.contains(&"12") && texts.contains(&"kbit/s"));
    }
}

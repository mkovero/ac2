//! The top bar's line: where the daemon runs and how it is doing, then the session, side
//! by side in the room between the bar's fixed ends, scrolling through it when wider.

use ac2_scene::health::HealthLine;
use ac2_scene::top_bar::{Bar, Click, Lead, Tone};
use eframe::egui::{self, Color32, RichText};

use super::chrome::{BAR_GAP, text_width};
use crate::keys::CommandId;
use crate::marquee::Marquee;
use crate::theme::Chrome;

/// With less room than this a scrolling line shows too little at once to follow: the
/// bar's key hints go first.
pub(super) const LINE_MIN_W: f32 = 320.0;
/// The fade over the text where the room's edge cuts it.
const FADE_W: f32 = 16.0;

/// One piece of text at `x` from its part's (or, for a separator, the line's) start.
struct Piece {
    text: egui::WidgetText,
    x: f32,
}

/// One part of the line, with its own tooltip and click.
struct Part {
    pieces: Vec<Piece>,
    /// Where it starts in the line.
    x: f32,
    width: f32,
    /// What a screen reader (and a test) reads for it.
    label: String,
    hover: Option<String>,
    click: Option<CommandId>,
}

/// The line laid out: its parts, the dots and rules between them, and its width.
pub(super) struct Line {
    parts: Vec<Part>,
    dots: Vec<Piece>,
    /// Where each rule between two segments stands.
    rules: Vec<f32>,
    pub(super) width: f32,
}

pub(super) fn tone_color(ch: &Chrome, t: Tone) -> Color32 {
    match t {
        Tone::Plain => ch.text,
        Tone::Dim => ch.dim,
        Tone::Warn => ch.warn,
        Tone::Fault => ch.fault,
        Tone::Armed => ch.armed,
    }
}

/// Lays `bar`'s line out from x = 0: each part after what its lead asks for (a space, a
/// dot, a rule), the health meter in its fixed slots.
pub(super) fn lay_out_line(ui: &egui::Ui, ch: &Chrome, bar: &Bar) -> Line {
    let width = |s: &str| text_width(ui, &egui::WidgetText::from(RichText::new(s)));
    // A lone space may measure as nothing: the space is what it adds between two glyphs.
    let space = (width("x x") - width("xx")).max(0.0);
    let mut line = Line {
        parts: Vec::new(),
        dots: Vec::new(),
        rules: Vec::new(),
        width: 0.0,
    };
    let mut x = 0.0;
    for p in &bar.line {
        match p.lead {
            Lead::First => {}
            Lead::Space => x += space,
            Lead::Dot => {
                line.dots.push(Piece {
                    text: RichText::new(" · ").color(ch.dim).into(),
                    x,
                });
                x += width(" · ");
            }
            Lead::Rule => {
                x += BAR_GAP;
                line.rules.push(x);
                x += BAR_GAP;
            }
        }
        let (pieces, w) = match &p.figures {
            Some(h) => {
                let (placed, w) = lay_out_health(h, width);
                let pieces = placed
                    .into_iter()
                    .map(|s| Piece {
                        text: RichText::new(s.text)
                            .color(if s.warn { ch.warn } else { ch.dim })
                            .into(),
                        x: s.x,
                    })
                    .collect();
                (pieces, w)
            }
            None => {
                let text: egui::WidgetText =
                    RichText::new(&p.text).color(tone_color(ch, p.tone)).into();
                let w = text_width(ui, &text);
                (vec![Piece { text, x: 0.0 }], w)
            }
        };
        line.parts.push(Part {
            pieces,
            x,
            width: w,
            label: p.text.clone(),
            hover: p.hover.clone(),
            click: p.click.map(|c| match c {
                Click::Connection => CommandId::BetweenRigs,
                Click::Sessions => CommandId::Sessions,
            }),
        });
        x += w;
    }
    line.width = x;
    line
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
        // The line puts its own dot before the meter.
        if k > 0 {
            let sep = " · ";
            put(Span::Sep, sep, x);
            x += width(sep);
        }
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

/// What [`draw_line`] reports back.
pub(super) struct Drawn {
    /// When to draw next, seconds from now, while the line scrolls.
    pub(super) wake: Option<f64>,
    /// The command of the part clicked.
    pub(super) clicked: Option<CommandId>,
}

/// Draws `line` in `room`: still when it fits, otherwise scrolled by `marquee`, following
/// itself after a gap, and clipped at the room's edges (faded into `fade`, the bar's fill).
/// Each part is a label over its visible stretch, with its tooltip and click. The scroll
/// holds while the pointer is over the room, so a part can be read, hovered and clicked.
pub(super) fn draw_line(
    ui: &mut egui::Ui,
    room: egui::Rect,
    line: &Line,
    marquee: &mut Marquee,
    ch: &Chrome,
) -> Drawn {
    let hovered = ui.rect_contains_pointer(room);
    let now = ui.input(|i| i.time);
    let f = marquee.frame(now, line.width, room.width(), hovered);
    let painter = ui.painter_at(room);
    let id = ui.id().with("bar-line");
    let mut clicked = None;
    let galley = |ui: &egui::Ui, t: &egui::WidgetText| {
        t.clone().into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
    };
    // Scrolling, the line is drawn twice: itself and, a period on, its start coming round.
    let copies = [Some(0.0), f.period];
    for (copy, shift) in copies.into_iter().enumerate() {
        let Some(shift) = shift else { continue };
        let left = room.left() - f.offset + shift;
        if left >= room.right() {
            continue;
        }
        for d in &line.dots {
            let g = galley(ui, &d.text);
            let at = egui::pos2(left + d.x, room.center().y - g.size().y / 2.0);
            painter.galley(at, g, ch.dim);
        }
        for x in &line.rules {
            painter.vline(
                left + x,
                room.shrink2(egui::vec2(0.0, 3.0)).y_range(),
                egui::Stroke::new(1.0, ch.border),
            );
        }
        for (k, p) in line.parts.iter().enumerate() {
            let mut height: f32 = 0.0;
            for piece in &p.pieces {
                let g = galley(ui, &piece.text);
                height = height.max(g.size().y);
                let at = egui::pos2(left + p.x + piece.x, room.center().y - g.size().y / 2.0);
                painter.galley(at, g, ui.visuals().text_color());
            }
            let r = egui::Rect::from_min_size(
                egui::pos2(left + p.x, room.center().y - height / 2.0),
                egui::vec2(p.width, height),
            );
            let shown = r.intersect(room);
            if shown.width() < 0.5 {
                continue;
            }
            let sense = if p.click.is_some() {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            };
            let mut resp = ui.interact(shown, id.with((copy, k)), sense);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &p.label));
            if p.click.is_some() {
                resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    clicked = p.click;
                }
            }
            if let Some(h) = &p.hover {
                resp.on_hover_text(h);
            }
        }
    }
    if f.period.is_some() {
        // At its start the line's first letter stands whole at the room's edge.
        if f.offset > 0.0 {
            fade_edge(&painter, room, true, ch.panel);
        }
        fade_edge(&painter, room, false, ch.panel);
    }
    Drawn {
        wake: f.wake,
        clicked,
    }
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

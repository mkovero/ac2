//! SPL meter: the held time-weighted level as a big centred number, its name and unit, a
//! live level bar, the meter's own statistics since it was last reset (headed once by when
//! that was: they are not the Leq windows) and the calibration state (decisions 7a/7b), and
//! the display hold that keeps the number readable ([`SplHold`]).
//!
//! Metric names follow IEC 61672 notation: `L` + frequency weighting + time weighting
//! (`LAF` = A-weighted, Fast), `LAeq`, `LCpeak`, `LAFmax`, `LAFmin`.

use ac2_proto::frame::SplFrame;
use ac2_proto::model::{CalStatus, LevelScale, TimeWeighting, Weighting};
use ac2_proto::units::{Rev, WallNs};

use crate::banner::{BannerRow, Status};
use crate::canvas::{self, Canvas, anchor, label};
use crate::format;
use crate::primitives::{FillRect, HAlign, Rect, Scene, VAlign, Viewport};
use crate::theme::Theme;
use crate::time::{self, ClockOffset, Freshness};

/// `LAF`, `LCS`: L + frequency weighting + time weighting (IEC 61672-1).
pub fn metric_name(w: Weighting, t: TimeWeighting) -> String {
    format!("L{}{}", w.letter(), t.letter())
}

/// How long the meter's number holds one reading, in seconds: twice a second for F and I,
/// once for S, as hand-held meters update their display. Faster, the last digit of a Fast
/// level of music changes too often to be read; the time weighting already is the
/// averaging, so the held number is the time-weighted level at the update, never an
/// average of displayed levels. I needs no longer hold: its detector holds peaks itself
/// (1.5 s fall).
pub fn display_period_s(t: TimeWeighting) -> f64 {
    match t {
        TimeWeighting::Fast | TimeWeighting::Impulse => 0.5,
        TimeWeighting::Slow => 1.0,
    }
}

/// The reading an SPL meter's number shows: the frame taken at the last display update.
#[derive(Clone, Debug, PartialEq)]
pub struct SplHold {
    pub frame: SplFrame,
    /// Capture time of the frame's newest sample (daemon clock, ns) the hold counts from:
    /// the update instants fall on a grid of display periods, so the number updates at its
    /// rate however the frames arrive.
    pub at_ns: u64,
    /// The measurement's configuration the frame was made under.
    pub rev: Rev,
}

impl SplHold {
    /// Takes `next` (captured at `at_ns` under `rev`) when the display period since the
    /// held reading has passed, or at once when the reading means something else now: a
    /// new configuration (other weightings), another scale or calibration, the first frame,
    /// a clock going backwards (another daemon). Returns whether the held reading changed.
    pub fn update(
        held: &mut Option<SplHold>,
        next: &SplFrame,
        at_ns: u64,
        rev: Rev,
        period_s: f64,
    ) -> bool {
        let period_ns = (period_s.max(0.0) * 1e9) as u64;
        let same = |h: &SplHold| {
            let (a, b) = (&h.frame.meta, &next.meta);
            h.rev == rev
                && h.frame.meas == next.meas
                && a.scale == b.scale
                && a.weighting == b.weighting
                && a.time_weighting == b.time_weighting
                && a.peak_weighting == b.peak_weighting
                && a.cal == b.cal
                && a.mic_curve == b.mic_curve
        };
        match held {
            Some(h) if same(h) && at_ns >= h.at_ns => {
                let since = at_ns - h.at_ns;
                if since < period_ns.max(1) {
                    return false;
                }
                let steps = since / period_ns.max(1);
                // More than a period late (frames stopped for a while): start a new grid.
                h.at_ns = if steps > 1 {
                    at_ns
                } else {
                    h.at_ns + period_ns
                };
                h.frame = next.clone();
                true
            }
            _ => {
                *held = Some(SplHold {
                    frame: next.clone(),
                    at_ns,
                    rev,
                });
                true
            }
        }
    }
}

/// One secondary statistic.
#[derive(Clone, Debug, PartialEq)]
pub struct SplStat {
    pub label: String,
    pub value: String,
}

/// The slim level bar under the number: the live level (smooth, every frame) on a fixed
/// 100 dB scale with 10 dB ticks.
#[derive(Clone, Debug, PartialEq)]
pub struct SplBar {
    /// Filled part, 0 … 1.
    pub fill: f32,
    /// Tick positions, 0 … 1.
    pub ticks: Vec<f32>,
}

/// The bar's scale: 30 … 130 dB SPL, or −100 … 0 dBFS.
fn bar_range(scale: LevelScale) -> (f64, f64) {
    match scale {
        LevelScale::DbSpl => (30.0, 130.0),
        LevelScale::Dbfs => (-100.0, 0.0),
    }
}

fn bar(scale: LevelScale, level: f64) -> SplBar {
    let (lo, hi) = bar_range(scale);
    let fill = if level.is_finite() {
        ((level - lo) / (hi - lo)).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    SplBar {
        fill,
        ticks: (1..10).map(|k| k as f32 / 10.0).collect(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplReadout {
    /// `LAF`.
    pub metric: String,
    /// `94.0`, the held reading.
    pub value: String,
    /// `dB SPL` or `dBFS`.
    pub unit: String,
    /// `LAF · dB SPL`, under the number; `LAF · dB SPL corrected +5.0 dB` with a
    /// measuring-position correction in the levels.
    pub caption: String,
    /// `corrected +5.0 dB` (`crate::leq::position_text`): every level of the readout
    /// includes the measuring-position correction.
    pub corrected: Option<String>,
    /// Leq, Lpeak, Lmax, Lmin in the meter's weightings, over the meter's interval.
    pub stats: Vec<SplStat>,
    /// `meter since 4:01 · R resets`: the statistics' interval, stated once over them (the
    /// meter's own, not the Leq windows').
    pub since: String,
    /// `cal 3 h ago`, `cal from other mic / input`, `uncalibrated`; `· mic curve` when the
    /// curve is applied.
    pub cal: String,
    /// `STALE 3.2 s` when the frame is stale.
    pub stale: Option<String>,
    /// The live level.
    pub bar: SplBar,
}

/// Calibration text of a readout (decisions 7a/7b, `docs/design/q7-calibration.md` §3):
/// what the calibration rests on and its age when it belongs to this device + input + mic
/// (`cal 94 dB · 3 h ago`, `electrical cal (in-line, data sheet 15.0 mV/Pa) ±1 dB · 3 h
/// ago`), a mismatch warning when it belongs to another mic or input, `uncalibrated`
/// otherwise; `· mic curve` when the
/// mic's correction curve is applied. The age is on the daemon clock (`offset`).
pub fn cal_text(
    cal: CalStatus,
    mic_curve: bool,
    client_now: WallNs,
    offset: ClockOffset,
) -> String {
    let base = match cal {
        CalStatus::Uncalibrated => None,
        CalStatus::Verified { calibrated_at, .. }
        | CalStatus::OtherMicOrInput { calibrated_at, .. } => {
            crate::cal::status_text(cal, time::age_s(calibrated_at, client_now, offset))
        }
    }
    .unwrap_or_else(|| "uncalibrated".to_string());
    if mic_curve {
        format!("{base} · mic curve")
    } else {
        base
    }
}

/// The heading of the meter's statistics: since when they run (`meter since 4:01`, the date
/// too when not today) and the key that starts them again. `start` and `newest` are on the
/// daemon clock: the interval's start (the newest frame's capture less its interval) and the
/// newest frame's capture, whose local day is "today".
pub fn meter_since(
    start: WallNs,
    newest: WallNs,
    offset_s: impl Fn(WallNs) -> i32,
    reset_key: Option<&str>,
) -> String {
    let at = time::local_clock(start, newest, offset_s);
    match reset_key {
        Some(k) => format!("meter since {at} · {k} resets"),
        None => format!("meter since {at}"),
    }
}

/// The readout of `held` (the reading the number shows, [`SplHold`]) with the bar at
/// `live_level`, the newest frame's level in the same scale; `since` heads the statistics
/// ([`meter_since`]).
pub fn spl_readout(
    held: &SplFrame,
    live_level: f64,
    cal: String,
    freshness: Option<Freshness>,
    since: String,
) -> SplReadout {
    let m = &held.meta;
    let w = m.weighting.letter();
    let metric = metric_name(m.weighting, m.time_weighting);
    let stat = |label: String, v: f64| SplStat {
        label,
        value: format::level(v),
    };
    let unit = match m.scale {
        LevelScale::Dbfs => "dBFS",
        LevelScale::DbSpl => "dB SPL",
    }
    .to_string();
    let corrected = m.position.as_ref().map(crate::leq::position_text);
    SplReadout {
        caption: match &corrected {
            Some(c) => format!("{metric} · {unit} {c}"),
            None => format!("{metric} · {unit}"),
        },
        corrected,
        value: format::level(m.level),
        unit,
        stats: vec![
            stat(format!("L{w}eq"), m.leq),
            stat(format!("L{}peak", m.peak_weighting.letter()), m.lpeak),
            stat(format!("{metric}max"), m.lmax),
            stat(format!("{metric}min"), m.lmin),
        ],
        metric,
        since,
        cal,
        stale: match freshness {
            Some(Freshness::Stale { age_s }) => Some(format!("STALE {}", format::age(age_s))),
            // A stopped meter's number is no current level: it says so, dimmed like STALE.
            Some(Freshness::Stopped { .. }) => Some("STOPPED".into()),
            // The banner says why and since when; the number only says it is not current.
            Some(Freshness::AudioStopped { .. }) => Some("AUDIO STOPPED".into()),
            _ => None,
        },
        bar: bar(m.scale, live_level),
    }
}

/// Between the parts of the footer when it wraps.
const FOOTER_SEP: &str = " · ";

/// `text` cut to `width` at `size` with a trailing `…` (whole characters).
pub(crate) fn cut(text: &str, width: f32, size: f32) -> String {
    if canvas::text_width(text, size) <= width {
        return text.to_owned();
    }
    let mut out: String = text.to_owned();
    while !out.is_empty() && canvas::text_width(&format!("{out}…"), size) > width {
        out.pop();
    }
    format!("{}…", out.trim_end())
}

/// The footer's rows from the top, each `(text, align)` pieces: the calibration on the right
/// when it fits one row, else its parts (`MM1 34804`, `uncalibrated`, `mic curve: …`) packed
/// into rows left to right, a part too wide for a row cut with `…`. At most `max_rows`; what
/// does not fit goes into the last row, cut.
fn footer_rows(cal: &str, width: f32, size: f32, max_rows: usize) -> Vec<Vec<(String, HAlign)>> {
    if cal.is_empty() {
        return Vec::new();
    }
    if canvas::text_width(cal, size) <= width {
        return vec![vec![(cal.to_owned(), HAlign::Right)]];
    }
    let parts = cal.split(FOOTER_SEP);
    let mut rows: Vec<String> = Vec::new();
    for p in parts.filter(|p| !p.is_empty()) {
        let full = rows.len() >= max_rows.max(1);
        match rows.last_mut() {
            Some(row)
                if canvas::text_width(&format!("{row}{FOOTER_SEP}{p}"), size) <= width || full =>
            {
                row.push_str(FOOTER_SEP);
                row.push_str(p);
            }
            _ => rows.push(p.to_owned()),
        }
    }
    rows.into_iter()
        .map(|r| vec![(cut(&r, width, size), HAlign::Left)])
        .collect()
}

/// Width, in em, the number is sized for: at least a typical reading's (`000.0`, `-00.0`),
/// so the number keeps its size as its digits change.
fn number_em(value: &str) -> f32 {
    let typical = if value.starts_with('-') {
        "-00.0"
    } else {
        "000.0"
    };
    canvas::text_width(value, 1.0).max(canvas::text_width(typical, 1.0))
}

/// The number's block, by the number's size: the number (1.25 em), its caption under it,
/// and the bar under that when `bar`.
struct Stack {
    big: f32,
    caption: f32,
    gap: f32,
    bar_gap: f32,
    bar_h: f32,
}

impl Stack {
    fn new(big: f32, fs: f32) -> Self {
        Self {
            big,
            caption: (big * 0.14).max(fs),
            gap: (big * 0.05).max(2.0),
            bar_gap: (big * 0.12).max(4.0),
            bar_h: (big * 0.06).clamp(4.0, 18.0),
        }
    }

    fn height(&self, bar: bool) -> f32 {
        let text = 1.25 * self.big + self.gap + 1.25 * self.caption;
        if bar {
            text + self.bar_gap + self.bar_h
        } else {
            text
        }
    }
}

/// Lays the meter out in `size` below the banner strip, to be read across a room: the held
/// level as large as the pane allows, centred, its name and unit under it (`LAS · dB SPL`),
/// the live level as a slim bar, then the statistics under their one heading (since when
/// the meter has run) and, at the bottom, the calibration. Secondary text grows with the
/// pane. Laid out from the bottom up, so on a small pane the footer wraps or is cut, the
/// statistics wrap and the number shrinks rather than anything running into anything else.
/// On the `stage` (full screen) the meter is read across a room: the calibration line is
/// drawn only while the reading is stale or stopped, when what the level rests on matters
/// for judging it; otherwise its room goes to the number. (The history, Shift+B, belongs to
/// the Leq windows: it leaves this view.)
pub fn spl_scene(
    r: &SplReadout,
    stage: bool,
    status: &Status,
    theme: &Theme,
    size: Viewport,
) -> SplScene {
    let mut c = Canvas::new(size, theme);
    let pad = 12.0;
    let strip = canvas::banner_strip(&mut c, status, pad, size.width - 2.0 * pad, size, theme);
    let top = strip.rect.bottom();
    let area = Rect::new(
        pad,
        top + pad,
        size.width - 2.0 * pad,
        (size.height - top - 2.0 * pad).max(1.0),
    );
    let fs = theme.font_size;
    let stat_size = (area.h.min(area.w * 0.6) * 0.045).clamp(fs, 2.6 * fs);
    let small = (stat_size * 0.8).max(theme.small_font_size);
    let mut room_top = area.y;
    if let Some(s) = &r.stale {
        c.overlay.labels.push(label(
            s.clone(),
            [area.right(), area.y],
            anchor(HAlign::Right, VAlign::Top),
            stat_size,
            theme.banner_warning.background,
        ));
        room_top += 1.25 * stat_size + 4.0;
    }
    // Bottom up: the footer, the statistics above it, the number's block above them.
    // A short pane spends fewer rows on the footer (cut instead) to keep the number large.
    let max_rows = match area.h {
        h if h >= 200.0 => 3,
        h if h >= 160.0 => 2,
        _ => 1,
    };
    let captioned = !stage || r.stale.is_some();
    let footer = if captioned {
        footer_rows(&r.cal, area.w, small, max_rows)
    } else {
        Vec::new()
    };
    let line_h = 1.25 * small;
    let footer_top = area.bottom() - footer.len() as f32 * line_h;
    let gap = (0.5 * stat_size).max(6.0);
    // Every statistic on one row when each fits its share of the width, else two per row,
    // else one.
    let texts: Vec<String> = r
        .stats
        .iter()
        .map(|s| format!("{} {}", s.label, s.value))
        .collect();
    let widest = texts
        .iter()
        .map(|t| canvas::text_width(t, stat_size))
        .fold(0.0, f32::max);
    let n = texts.len().max(1);
    let fits = |per: usize| widest + 6.0 <= area.w / per as f32;
    let per_row = [n, n.div_ceil(2), 1]
        .into_iter()
        .find(|p| fits(*p))
        .unwrap_or(1);
    let stat_rows = texts.len().div_ceil(per_row);
    let row_h = 1.4 * stat_size;
    let stats_top =
        footer_top - gap - stat_rows.saturating_sub(1) as f32 * row_h - 1.25 * stat_size;
    // The statistics' heading sits right over them: it belongs to them, not to the number.
    let since_top = stats_top - 0.25 * stat_size - line_h;
    let room = (since_top - gap - room_top).max(0.0);
    let block = draw_number_block(&mut c, r, Rect::new(area.x, room_top, area.w, room), theme);
    let cx = area.x + area.w / 2.0;
    c.overlay.labels.push(label(
        cut(&r.since, area.w, small),
        [cx, since_top],
        anchor(HAlign::Center, VAlign::Top),
        small,
        theme.text_dim,
    ));
    for (i, t) in texts.into_iter().enumerate() {
        let (r_i, c_i) = (i / per_row, i % per_row);
        c.overlay.labels.push(label(
            t,
            [
                area.x + area.w * (c_i as f32 + 0.5) / per_row as f32,
                stats_top + 0.625 * stat_size + r_i as f32 * row_h,
            ],
            anchor(HAlign::Center, VAlign::Center),
            stat_size,
            theme.text,
        ));
    }
    for (i, pieces) in footer.into_iter().enumerate() {
        let y = footer_top + (i + 1) as f32 * line_h;
        for (text, h) in pieces {
            let x = match h {
                HAlign::Right => area.right(),
                _ => area.x,
            };
            c.overlay.labels.push(label(
                text,
                [x, y],
                anchor(h, VAlign::Bottom),
                small,
                theme.text_dim,
            ));
        }
    }
    SplScene {
        scene: c.into_scene(size),
        area,
        strip: strip.rect,
        banners: strip.rows,
        bar: block.bar,
    }
}

/// Where the number's block went.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumberBlock {
    /// The number's type size.
    pub size: f32,
    /// The level bar, when there was room for it.
    pub bar: Option<Rect>,
}

/// Draws the number's block in `room`: the held level as wide as `room` allows, then as
/// high, its name and unit under it and the slim live bar under that when it fits, the
/// whole centred. Dimmed while the frame is stale.
pub(crate) fn draw_number_block(
    c: &mut Canvas,
    r: &SplReadout,
    room: Rect,
    theme: &Theme,
) -> NumberBlock {
    let main = if r.stale.is_some() {
        theme.text_dim
    } else {
        theme.text
    };
    let s = fit_stack(&r.value, room, theme.font_size);
    let with_bar = s.height(true) <= room.h + 0.5;
    let y0 = room.y + ((room.h - s.height(with_bar)) / 2.0).max(0.0);
    let cx = room.x + room.w / 2.0;
    c.overlay.labels.push(label(
        r.value.clone(),
        [cx, y0],
        anchor(HAlign::Center, VAlign::Top),
        s.big,
        main,
    ));
    let caption_y = y0 + 1.25 * s.big + s.gap;
    c.overlay.labels.push(label(
        r.caption.clone(),
        [cx, caption_y],
        anchor(HAlign::Center, VAlign::Top),
        s.caption,
        theme.text_dim,
    ));
    let bar_rect = with_bar.then(|| {
        let w = (canvas::text_width("000.0", s.big)).min(room.w * 0.94);
        Rect::new(
            cx - w / 2.0,
            caption_y + 1.25 * s.caption + s.bar_gap,
            w,
            s.bar_h,
        )
    });
    if let Some(b) = bar_rect {
        c.base.rects.push(FillRect {
            rect: b,
            color: theme.plot_background,
            clip: None,
        });
        c.data.rects.push(FillRect {
            rect: Rect::new(b.x, b.y, b.w * r.bar.fill, b.h),
            color: if r.stale.is_some() {
                theme.text_dim
            } else {
                theme.level_ok
            },
            clip: None,
        });
        for t in &r.bar.ticks {
            c.overlay.rects.push(FillRect {
                rect: Rect::new(b.x + b.w * t - 0.5, b.y, 1.0, b.h),
                color: theme.background,
                clip: None,
            });
        }
    }
    NumberBlock {
        size: s.big,
        bar: bar_rect,
    }
}

/// The number's block as large as `room` allows: as wide as its width, then as high as its
/// height; never under 1.2 em of the caption type.
fn fit_stack(value: &str, room: Rect, fs: f32) -> Stack {
    let mut s = Stack::new(room.w * 0.94 / number_em(value), fs);
    for _ in 0..4 {
        let h = s.height(true);
        if h <= room.h {
            break;
        }
        s = Stack::new(s.big * room.h / h, fs);
    }
    Stack::new(s.big.max(1.2 * fs), fs)
}

/// The number's type size [`draw_number_block`] gives in `room` with the bar, `None` when
/// the bar does not fit.
pub(crate) fn number_block_size(value: &str, room: Rect, fs: f32) -> Option<f32> {
    let s = fit_stack(value, room, fs);
    (s.height(true) <= room.h + 0.5).then_some(s.big)
}

/// Draws the number with its name and unit after it on the same baseline, centred
/// together in `room` (a short pane), the number as large as the line allows. Returns the
/// number's type size.
pub(crate) fn draw_number_line(c: &mut Canvas, r: &SplReadout, room: Rect, theme: &Theme) -> f32 {
    let fs = theme.font_size;
    let main = if r.stale.is_some() {
        theme.text_dim
    } else {
        theme.text
    };
    // The name at 0.4 of the number (at least the caption type), 0.3 em after it.
    let em = number_em(&r.value);
    let by_height = room.h / 1.25;
    let by_width = room.w * 0.94 / (em + 0.3 + 0.4 * canvas::text_width(&r.caption, 1.0));
    let big = by_height.min(by_width).max(fs);
    let cap = (0.4 * big).max(fs).min(big);
    let value_w = canvas::text_width(&r.value, big);
    let caption = cut(
        &r.caption,
        (room.w * 0.94 - value_w - 0.3 * big).max(fs),
        cap,
    );
    let total = value_w + 0.3 * big + canvas::text_width(&caption, cap);
    let x0 = room.x + ((room.w - total) / 2.0).max(0.0);
    let baseline = room.y + ((room.h - 1.25 * big) / 2.0).max(0.0) + 0.95 * big;
    c.overlay.labels.push(label(
        r.value.clone(),
        [x0, baseline],
        anchor(HAlign::Left, VAlign::Baseline),
        big,
        main,
    ));
    c.overlay.labels.push(label(
        caption,
        [x0 + value_w + 0.3 * big, baseline],
        anchor(HAlign::Left, VAlign::Baseline),
        cap,
        theme.text_dim,
    ));
    big
}

/// The SPL meter as drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct SplScene {
    pub scene: Scene,
    /// Where the readout is laid out, below the banner strip.
    pub area: Rect,
    /// Banner strip above the readout; zero height when no banner is up.
    pub strip: Rect,
    pub banners: Vec<BannerRow>,
    /// The level bar, when the pane has room for it.
    pub bar: Option<Rect>,
}

#[cfg(test)]
mod tests;

use super::*;
use crate::grid::column_edges;
use crate::view::FreqRange;
use ac2_proto::GridDef;
use ac2_proto::units::Hz;

/// Frames 1/30 s apart, as a short FFT publishes them.
const FRAME_NS: u64 = 33_333_334;
const T0: u64 = 1_000 * 1_000_000_000;

fn third_octaves() -> GridDef {
    GridDef::IecBands {
        fraction: ac2_proto::model::BandFraction::Third,
        centres: (-17..=13)
            .map(|x| Hz(1000.0 * 10f64.powf(f64::from(x) / 10.0)))
            .collect(),
    }
}

fn log_bins() -> GridDef {
    GridDef::LogBins {
        fs: Hz(48_000.0),
        n: 16_384,
        ppo: 96,
    }
}

struct Feed {
    edges: Vec<(f64, f64)>,
    grid: GridId,
    seq: u64,
}

impl Feed {
    fn new(g: &GridDef) -> Self {
        Self {
            edges: column_edges(g),
            grid: g.id(),
            seq: 0,
        }
    }

    /// A frame at `at_ns` with every column at `level`.
    fn push(&mut self, h: &mut SpectrographHistory, at_ns: u64, level: f32) -> bool {
        let l = vec![level; self.edges.len()];
        self.push_levels(h, at_ns, &l, LevelScale::Dbfs)
    }

    fn push_levels(
        &mut self,
        h: &mut SpectrographHistory,
        at_ns: u64,
        level: &[f32],
        scale: LevelScale,
    ) -> bool {
        self.seq += 1;
        h.push(&SpectrographFrame {
            seq: self.seq,
            at: WallNs(at_ns),
            grid: self.grid,
            edges: &self.edges,
            scale,
            level,
            validity: None,
        })
    }
}

/// The level shown `back` slots before the newest at 1 kHz.
fn at(h: &SpectrographHistory, back: usize) -> Option<f32> {
    h.value_at(1000.0, (back as f64 + 0.5) * h.slot_s())
}

#[test]
fn rows_are_even_in_log_frequency_and_keep_narrow_peaks() {
    let g = log_bins();
    let edges = column_edges(&g);
    let m = RowMap::new(&edges).expect("rows");
    let octaves = (m.hi_hz() / m.lo_hz()).log2();
    assert_eq!(m.rows(), (octaves * ROWS_PER_OCTAVE).ceil() as usize);
    // Up to the top column's upper edge, half a bin above Nyquist.
    assert!(m.lo_hz() > 0.0 && m.hi_hz() < 24_002.0, "{}", m.hi_hz());
    // A tone in one column near 10 kHz keeps its level in its row; elsewhere the floor.
    let freqs = crate::grid::column_frequencies(&g);
    let k = crate::grid::nearest_column(&freqs, 10_000.0).expect("column");
    let mut level = vec![-100.0f32; edges.len()];
    level[k] = -20.0;
    let rows = m.resample(&level, None);
    let r = m.row_of(freqs[k]).expect("row");
    assert_eq!(rows[r], -20.0);
    assert!(rows.iter().filter(|v| **v == -20.0).count() <= 2);
    // A single-bin column at 40 Hz (bins 2.9 Hz apart) spans several rows: all of them
    // show it, with no gap.
    let k = crate::grid::nearest_column(&freqs, 40.0).expect("column");
    let (lo, hi) = edges[k];
    let r0 = m.row_of(lo * 1.0001).expect("row");
    let r1 = m.row_of(hi * 0.9999).expect("row");
    assert!(r1 > r0 + 1, "{r0}..{r1}");
    let mut level = vec![f32::NAN; edges.len()];
    level[k] = -40.0;
    let rows = m.resample(&level, None);
    assert!((r0..=r1).all(|r| rows[r] == -40.0));
}

#[test]
fn bands_fill_their_rows_and_invalid_bands_are_gaps() {
    let g = third_octaves();
    let edges = column_edges(&g);
    let m = RowMap::new(&edges).expect("rows");
    let level: Vec<f32> = (0..edges.len()).map(|i| i as f32).collect();
    let mut validity = vec![ValidityMask::NONE; edges.len()];
    validity[17] = ValidityMask(1);
    let rows = m.resample(&level, Some(&validity));
    // The 1 kHz band (index 17) is invalid: its middle row has no value; 2 kHz (index 20)
    // reads its own level.
    assert!(rows[m.row_of(1000.0).expect("row")].is_nan());
    assert_eq!(rows[m.row_of(2000.0).expect("row")], 20.0);
}

#[test]
fn frames_scroll_through_a_bounded_ring() {
    let mut h = SpectrographHistory::new(30);
    let mut f = Feed::new(&third_octaves());
    assert!(h.is_empty() && at(&h, 0).is_none());
    for i in 0..10u64 {
        assert!(f.push(&mut h, T0 + i * FRAME_NS, i as f32));
    }
    assert_eq!(at(&h, 0), Some(9.0));
    assert_eq!(at(&h, 9), Some(0.0));
    // The newest is drawn at the top: the oldest ring position follows it.
    let newest = (T0 + 9 * FRAME_NS) / h.slot_ns;
    assert_eq!(u64::from(h.scroll()), (newest + 1) % SLOTS as u64);
    // A full history later the first frames are gone and memory stays SLOTS columns.
    for i in 10..(SLOTS as u64 + 20) {
        f.push(&mut h, T0 + i * FRAME_NS, i as f32);
    }
    assert_eq!(h.ring().len(), SLOTS);
    assert!(at(&h, SLOTS - 1).is_some_and(|v| v >= 19.0));
    assert_eq!(h.value_at(1000.0, 31.0), None);
    // The same frame delivered again changes nothing.
    let level = vec![99.0; f.edges.len()];
    assert!(!h.push(&SpectrographFrame {
        seq: f.seq,
        at: WallNs(T0),
        grid: f.grid,
        edges: &f.edges,
        scale: LevelScale::Dbfs,
        level: &level,
        validity: None,
    }));
}

#[test]
fn a_frame_stands_for_the_time_since_the_previous_one() {
    let mut h = SpectrographHistory::new(30);
    let mut f = Feed::new(&third_octaves());
    // A long FFT: a new spectrum every 0.17 s; the slots in between hold the previous one.
    f.push(&mut h, T0, -50.0);
    f.push(&mut h, T0 + 170_000_000, -40.0);
    assert_eq!(at(&h, 0), Some(-40.0));
    assert!((1..=4).all(|b| at(&h, b) == Some(-50.0)));
    // The held column is one allocation: holding copies nothing.
    let filled: Vec<_> = h.ring().iter().flatten().collect();
    assert!(
        filled
            .windows(2)
            .filter(|w| Arc::ptr_eq(w[0], w[1]))
            .count()
            >= 3
    );
}

#[test]
fn breaks_show_as_gaps() {
    let mut h = SpectrographHistory::new(30);
    let mut f = Feed::new(&third_octaves());
    f.push(&mut h, T0, -50.0);
    // STALE (or stopped) for 2 s, then frames again: the 2 s are empty, not −50.
    h.mark_break();
    f.push(&mut h, T0 + 2_000_000_000, -30.0);
    assert_eq!(at(&h, 0), Some(-30.0));
    let back = (2.0 / h.slot_s()) as usize;
    assert!((1..back - 1).all(|b| at(&h, b).is_none()));
    assert!((back - 1..=back + 1).any(|b| at(&h, b) == Some(-50.0)));
    // Longer than the history away: only the new frame is left.
    f.push(&mut h, T0 + 60_000_000_000, -10.0);
    assert_eq!(h.ring().iter().flatten().count(), 1);
    // The clock going back starts over too.
    f.push(&mut h, T0, -20.0);
    assert_eq!(h.ring().iter().flatten().count(), 1);
    assert_eq!(at(&h, 0), Some(-20.0));
}

#[test]
fn frames_in_one_slot_keep_the_highest_level() {
    let mut h = SpectrographHistory::new(30);
    let mut f = Feed::new(&third_octaves());
    let t0 = 1_000 * h.slot_ns;
    f.push(&mut h, t0, -50.0);
    f.push(&mut h, t0 + 1, -60.0);
    assert_eq!(at(&h, 0), Some(-50.0));
    f.push(&mut h, t0 + 2, -40.0);
    assert_eq!(at(&h, 0), Some(-40.0));
}

#[test]
fn a_new_grid_or_scale_starts_over() {
    let mut h = SpectrographHistory::new(30);
    let mut a = Feed::new(&third_octaves());
    a.push(&mut h, T0, -50.0);
    a.push(&mut h, T0 + FRAME_NS, -50.0);
    let mut b = Feed::new(&log_bins());
    b.seq = a.seq;
    b.push(&mut h, T0 + 2 * FRAME_NS, -30.0);
    assert_eq!(h.ring().iter().flatten().count(), 1);
    assert_eq!(at(&h, 0), Some(-30.0));
    let l = vec![80.0; b.edges.len()];
    b.push_levels(&mut h, T0 + 3 * FRAME_NS, &l, LevelScale::DbSpl);
    assert_eq!(h.ring().iter().flatten().count(), 1);
    assert_eq!(h.scale(), Some(LevelScale::DbSpl));
}

fn scene_of(h: &SpectrographHistory, view: &ViewState) -> SpectrographScene {
    let input = SpectrographInput {
        history: h,
        name: "Main".into(),
        range: Range::new(-100.0, 0.0),
        offset_db: 0.0,
        freshness: None,
    };
    spectrograph_scene(
        &[],
        &Status::default(),
        Some(&input),
        view,
        &Theme::dark(),
        Viewport {
            width: 800.0,
            height: 500.0,
        },
    )
}

fn heatmaps(s: &Scene) -> Vec<&Heatmap> {
    s.layers.iter().flat_map(|l| l.heatmaps.iter()).collect()
}

#[test]
fn the_picture_follows_the_frequency_axis_without_new_columns() {
    let mut h = SpectrographHistory::new(30);
    let mut f = Feed::new(&log_bins());
    f.push(&mut h, T0, -40.0);
    let mut view = ViewState::default();
    let a = scene_of(&h, &view);
    let hm = heatmaps(&a.scene);
    assert_eq!(hm.len(), 2, "history and colour bar");
    let g = hm[1];
    assert_eq!(g.axes, HeatmapAxes::TimeUp);
    assert_eq!((g.columns, g.range), (SLOTS as u32, [-100.0, 0.0]));
    let rows = h.rows().expect("rows");
    // The rows span the pixels the frequency axis gives their edges.
    let xm = a.x_axis.mapping;
    assert!((g.rect.x - xm.to_px(rows.lo_hz())).abs() < 1e-3);
    assert!((g.rect.right() - xm.to_px(rows.hi_hz())).abs() < 0.1);
    assert_eq!(g.clip, Some(a.plot));
    // Lined up with the spectrum above.
    assert_eq!(a.spectrum.plot.x, a.plot.x);
    assert_eq!(a.spectrum.plot.w, a.plot.w);
    assert!(a.spectrum.plot.bottom() < a.plot.y);
    // Zoomed: the rect moves, the columns are the very same.
    view.freq = FreqRange::default().zoom(1000.0, 4.0);
    let b = scene_of(&h, &view);
    let gb = heatmaps(&b.scene)[1];
    assert_ne!(gb.rect, g.rect);
    assert!(g.data.iter().zip(&gb.data).all(|(x, y)| match (x, y) {
        (Some(x), Some(y)) => Arc::ptr_eq(x, y),
        (None, None) => true,
        _ => false,
    }));
}

#[test]
fn axes_bar_caption_and_cursor_read_as_numbers() {
    let mut h = SpectrographHistory::new(30);
    let mut f = Feed::new(&third_octaves());
    f.push(&mut h, T0, -23.5);
    f.push(&mut h, T0 + 5_000_000_000, -60.0);
    let mut view = ViewState::default();
    let s = scene_of(&h, &view);
    assert_eq!(s.caption, "Main · last 30 s · dBFS");
    assert_eq!(s.time_axis.labels(), ["0 s", "10 s", "20 s", "30 s"]);
    assert!((s.time_axis.mapping.to_px(0.0) - s.plot.y).abs() < 1e-3);
    assert!(s.bar_axis.labels().contains(&"\u{2212}50"));
    assert_eq!((s.bar.y, s.bar.h), (s.plot.y, s.plot.h));
    assert_eq!(s.cursor, None);
    assert_eq!(s.message, None);
    // A point 4.2 s ago at 1 kHz: the hold of the −23.5 dB frame.
    view.cursor_hz = Some(1000.0);
    view.spectrum.spectrograph.cursor_s = Some(4.2);
    let s = scene_of(&h, &view);
    assert_eq!(
        s.cursor.expect("cursor").text,
        "1.00 kHz · 4.2 s ago · \u{2212}23.5 dBFS"
    );
    // Before the first frame: no value.
    view.spectrum.spectrograph.cursor_s = Some(12.0);
    let s = scene_of(&h, &view);
    assert_eq!(s.cursor.expect("cursor").text, "1.00 kHz · 12 s ago · —");
}

#[test]
fn stale_dims_offset_shifts_and_empty_says_so() {
    let mut h = SpectrographHistory::new(10);
    let theme = Theme::dark();
    let size = Viewport {
        width: 600.0,
        height: 400.0,
    };
    let view = ViewState::default();
    let input = |h: &SpectrographHistory, off: f64, fr: Option<Freshness>| {
        let s = SpectrographInput {
            history: h,
            name: "Sub".into(),
            range: Range::new(-90.0, -10.0),
            offset_db: off,
            freshness: fr,
        };
        spectrograph_scene(&[], &Status::default(), Some(&s), &view, &theme, size)
    };
    let s = input(&h, 0.0, None);
    assert_eq!(s.message.as_deref(), Some("Sub: no frames yet"));
    assert_eq!(heatmaps(&s.scene).len(), 1, "colour bar only");
    let none = spectrograph_scene(&[], &Status::default(), None, &view, &theme, size);
    assert_eq!(
        none.message.as_deref(),
        Some("no spectrum or RTA measurement")
    );
    let mut f = Feed::new(&third_octaves());
    f.push(&mut h, T0, -40.0);
    let s = input(&h, 6.0, Some(Freshness::Stale { age_s: 2.0 }));
    let g = heatmaps(&s.scene)[1];
    assert_eq!(g.opacity, theme.stale_alpha);
    assert_eq!(g.range, [-96.0, -16.0]);
    assert_eq!(s.caption, "Sub · offset +6.0 dB · last 10 s · dBFS");
    let s = input(&h, 0.0, Some(Freshness::Stopped { age_s: 2.0 }));
    assert!(s.caption.ends_with(" · stopped"), "{}", s.caption);
}

/// Split with the spectrograph, the spectrum part keeps its own legend (above its plot,
/// never in the spectrograph), its per-bin axis unit and the selected trace's mark.
#[test]
fn the_spectrum_part_keeps_its_legend_unit_and_selection() {
    let g = log_bins();
    let cols = crate::grid::columns(&g);
    let level = vec![-60.0f32; cols.freqs.len()];
    let trace = |id: u32, name: &str, selected: bool| SpectrumTrace {
        key: crate::trace::TraceKey::Stored(ac2_proto::units::TraceId(id)),
        name: name.into(),
        color: crate::primitives::Color::WHITE,
        freqs: &cols.freqs,
        edges: &cols.edges,
        level: &level,
        validity: None,
        peak: None,
        scale: LevelScale::Dbfs,
        quantity: crate::spectrum::Quantity::Tone,
        bin_hz: cols.bin_hz,
        caption: "stored".into(),
        freshness: None,
        offset_db: 0.0,
        selected,
    };
    let traces = [trace(1, "Main L S1", false), trace(2, "Main L S2", true)];
    let h = SpectrographHistory::new(30);
    let input = SpectrographInput {
        history: &h,
        name: "Main".into(),
        range: Range::new(-100.0, 0.0),
        offset_db: 0.0,
        freshness: None,
    };
    let theme = Theme::dark();
    let s = spectrograph_scene(
        &traces,
        &Status::default(),
        Some(&input),
        &ViewState::default(),
        &theme,
        Viewport {
            width: 800.0,
            height: 500.0,
        },
    );
    let sp = &s.spectrum;
    assert_eq!(sp.unit, "dBFS per 2.93 Hz bin (tone)");
    assert_eq!(sp.legend_shown, ["Main L S1", "Main L S2"]);
    let selected: Vec<bool> = sp.legend.iter().map(|e| e.selected).collect();
    assert_eq!(selected, [false, true]);
    assert!(sp.legend_rect.bottom() <= sp.plot.y);
    assert!(
        sp.plot.bottom() < s.plot.y,
        "the spectrum part is above the spectrograph"
    );
    // The selected curve is drawn twice as wide in the spectrum part.
    let widths: Vec<f32> = s
        .scene
        .layers
        .iter()
        .flat_map(|l| &l.polylines)
        .filter(|p| p.clip == Some(sp.plot))
        .map(|p| p.stroke.width)
        .collect();
    assert_eq!(
        widths,
        [
            theme.trace_width,
            theme.trace_width * crate::tf::SELECTED_WIDTH
        ]
    );
}

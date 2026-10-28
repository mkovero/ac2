//! SPL panes side by side: their Leq columns share one scale.

use super::*;

/// The Leq columns of pane `p` as drawn at 900 × 600.
fn columns_of(t: &T, p: PaneId) -> ac2_scene::leq::LeqColumns {
    let now = crate::scenes::Now {
        instant: Instant::now(),
        wall: WallNs(0),
    };
    let size = ac2_scene::primitives::Viewport {
        width: 900.0,
        height: 600.0,
    };
    crate::scenes::leq(&t.st, p, &ac2_scene::theme::Theme::dark(), size, now)
        .and_then(|s| s.columns)
        .expect("the Leq columns")
}

/// `FOH SPL` judged at 100 dB on one pane, `SPL 2` at 110 dB on the one beside it: both
/// columns span 70…116 dB, the union of their own (70…106, 80…116), so one level sits at
/// one height in both. With the setting off, or left alone again, a pane's scale is its own.
#[test]
fn spl_panes_side_by_side_share_the_leq_scale() {
    use ac2_proto::frame::LeqFlags as F;
    let mut t = T::new();
    let mut s = with_spl();
    let mut second = meas(5, "SPL 2", spl_meter());
    if let MeasKind::Spl { config } = &mut second.config.kind {
        config.input = 2;
    }
    s.measurements.push(second);
    for m in &mut s.measurements {
        let limit = match m.id {
            MeasId(4) => 100.0,
            _ => 110.0,
        };
        if let MeasKind::Spl { config } = &mut m.config.kind {
            for w in &mut config.leq.windows {
                w.limit = Some(DbSpl(limit));
            }
        }
    }
    t.conn(mirror(s));
    let judged = F::LIMIT.with(F::JUDGED);
    let mut latest = ac2_client::Latest::default();
    for (m, leq) in [(4, 95.0), (5, 104.0)] {
        let ConnEvent::Data(d) = leq_data_of(MeasId(m), 1, 1, 100, leq, judged) else {
            unreachable!()
        };
        latest.frames.extend(d.latest.frames.clone());
    }
    t.conn(ConnEvent::Data(Arc::new(crate::conn::DataSnapshot {
        latest,
        grids: Default::default(),
        drained: Instant::now(),
    })));
    t.key("Alt+4");
    let foh = t.st.layout.focus;
    assert_eq!(t.focus_kind(), PaneKind::Spl);
    t.st.layout.view_mut(foh).expect("pane").meas = Some(MeasId(4));
    t.key("N");
    let other = t.st.layout.focus;
    assert_ne!(other, foh);
    assert_eq!(t.focus_kind(), PaneKind::Spl);
    t.st.layout.view_mut(other).expect("pane").meas = Some(MeasId(5));

    crate::scenes::refresh_spl_scales(&mut t.st);
    let (a, b) = (columns_of(&t, foh), columns_of(&t, other));
    assert_eq!((a.range.lo, a.range.hi), (70.0, 116.0));
    assert_eq!(a.range, b.range);
    let tracks = |k: &ac2_scene::leq::LeqColumns| -> Vec<(f32, f32)> {
        k.columns.iter().map(|c| (c.track.y, c.track.h)).collect()
    };
    assert_eq!(tracks(&a), tracks(&b));

    // Settings › Display › Leq scale off: each pane on its own scale.
    t.st.prefs.shared_scale = false;
    crate::scenes::refresh_spl_scales(&mut t.st);
    let (a, b) = (columns_of(&t, foh), columns_of(&t, other));
    assert_eq!((a.range.lo, a.range.hi), (70.0, 106.0));
    assert_eq!((b.range.lo, b.range.hi), (80.0, 116.0));
    t.st.prefs.shared_scale = true;

    // The second pane closed: the first is on its own scale again.
    t.key("Q");
    crate::scenes::refresh_spl_scales(&mut t.st);
    let alone = columns_of(&t, foh);
    assert_eq!((alone.range.lo, alone.range.hi), (70.0, 106.0));
}

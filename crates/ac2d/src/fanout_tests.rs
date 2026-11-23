use super::*;

#[test]
fn stats_match_a_per_channel_scan() {
    let data: Vec<f32> = (0..300)
        .map(|i| ((i * 37 % 101) as f32 - 50.0) / 51.0)
        .collect();
    let b = Block::new(0, 100, 3, BlockFlags::NONE, 0, data.clone());
    for ch in 0..3 {
        let (s, p) = data
            .iter()
            .skip(ch)
            .step_by(3)
            .fold((0.0f64, 0.0f32), |(s, p), v| {
                (s + f64::from(*v) * f64::from(*v), p.max(v.abs()))
            });
        assert_eq!(b.stats[ch], ChanStats { sum_sq: s, peak: p });
    }
}

#[test]
fn handoff_follows_the_faster_of_publish_and_meter_rates_but_not_below_a_block() {
    let ms = |d: Duration| (d.as_secs_f64() * 1000.0 * 10.0).round() / 10.0;
    assert_eq!(ms(handoff_period(60, None)), 16.7);
    assert_eq!(ms(handoff_period(30, None)), 16.7);
    assert_eq!(ms(handoff_period(120, None)), 8.3);
    let long_block = Duration::from_secs_f64(2048.0 / 48_000.0);
    assert_eq!(handoff_period(60, Some(long_block)), long_block);
}

#[test]
fn a_stream_is_stopped_after_twenty_periods_but_never_under_a_second_nor_when_stepped() {
    let p = |frames: u32| Some(Duration::from_secs_f64(f64::from(frames) / 48_000.0));
    assert_eq!(
        stall_after(Delivery::Device, p(128)),
        Some(Duration::from_secs(1))
    );
    assert_eq!(
        stall_after(Delivery::Device, None),
        Some(Duration::from_secs(1))
    );
    let long = stall_after(Delivery::Device, p(8192)).unwrap_or_default();
    assert!((long.as_secs_f64() - 20.0 * 8192.0 / 48_000.0).abs() < 1e-6);
    assert_eq!(stall_after(Delivery::Stepped, p(128)), None);
}

#[test]
fn older_blocks_of_a_hand_off_are_stamped_earlier_by_the_audio_after_them() {
    let mut batch: Vec<Block> = (0..3)
        .map(|i| {
            Block::new(
                i * 480,
                480,
                1,
                BlockFlags::NONE,
                1_000_000_000,
                vec![0.0; 480],
            )
        })
        .collect();
    stamp_arrivals(&mut batch, 48_000.0);
    let walls: Vec<u64> = batch.iter().map(|b| b.wall_ns).collect();
    assert_eq!(walls, [980_000_000, 990_000_000, 1_000_000_000]);
}

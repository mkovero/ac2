//! JACK backend against a running server. Skips cleanly without one.
//!
//! Local run: `jackd -d dummy -r 48000 -p 256 &` then
//! `AC2_JACK_DUMMY=1 cargo test -p ac2-audio --test it jack::`.
//!
//! Audio safety: tests that emit a generator signal or connect our outputs to physical
//! playback ports require `AC2_JACK_DUMMY=1`, the operator's statement that the server runs
//! the dummy driver, whose playback ports lead nowhere. Emitted signals only travel through
//! graph connections from our own outputs to our own inputs.
#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use ac2_audio::generator::WhiteNoise;
use ac2_audio::{
    AudioError, Backend, BackendKind, BlockFlags, BlockHeader, ClockRelation, DuplexRequest,
    DuplexStream, HistoryRequest, IndexExactness, JackBackend, JackConfig, MaxLevel, OutputSource,
    StopOutcome, Unsupported, generator,
};

fn level() -> MaxLevel {
    MaxLevel::from_peak_db(-20.0).expect("level")
}

fn backend(name: &str, connect_inputs: bool) -> JackBackend {
    JackBackend::new(JackConfig {
        client_name: name.into(),
        connect_inputs,
    })
}

/// `None` (after printing why) when no server is reachable.
fn server_or_skip(b: &JackBackend) -> Option<()> {
    match b.enumerate() {
        Ok(_) => Some(()),
        Err(AudioError::Unavailable { reason, .. }) => {
            eprintln!("skipping: no JACK server ({reason})");
            None
        }
        Err(e) => panic!("unexpected JACK error: {e}"),
    }
}

fn collect(s: &mut DuplexStream, wall: Duration) -> Vec<(BlockHeader, Vec<f32>)> {
    let mut out = Vec::new();
    let mut buf = Vec::new();
    let end = Instant::now() + wall;
    while Instant::now() < end {
        while let Some(h) = s.capture().pop_into(&mut buf) {
            out.push((h, buf.clone()));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    out
}

#[test]
fn silent_duplex_has_exact_flagged_indices() {
    let b = backend("ac2-test-silent", false);
    if server_or_skip(&b).is_none() {
        return;
    }
    let caps = b.enumerate().expect("caps");
    assert_eq!(caps[0].duplex_clock, ClockRelation::SingleCallback);
    assert_eq!(caps[0].index, IndexExactness::Exact);

    let mut s = b
        .open(DuplexRequest::new(vec![0, 1], 2, level()))
        .expect("open");
    let n = s.negotiated().clone();
    assert_eq!(n.backend, BackendKind::Jack);
    let rate = n.sample_rate;
    let frames = n.buffer_frames.expect("fixed");

    let blocks = collect(&mut s, Duration::from_millis(500));
    assert!(blocks.len() as f64 > 0.2 * f64::from(rate) / f64::from(frames));
    assert!(blocks[0].0.flags.contains(BlockFlags::FIRST));
    for w in blocks.windows(2) {
        let (a, b) = (&w[0].0, &w[1].0);
        assert_eq!(b.channels, 2);
        assert!(b.start_sample >= a.end_sample(), "index never regresses");
        if b.start_sample != a.end_sample() {
            assert!(b.flags.breaks_continuity(), "every gap is flagged: {b:?}");
            assert!(
                !b.flags.contains(BlockFlags::GAP_ESTIMATED),
                "JACK gaps are exact"
            );
        }
    }
    assert!(blocks.iter().all(|(_, d)| d.iter().all(|&v| v == 0.0)));
    // Output ticks carry the capture index of the same cycle.
    let first = s.pop_output_tick().expect("tick");
    assert_eq!(first.start_sample, 0);
    assert_eq!(
        s.stop(Duration::from_millis(200)),
        StopOutcome::NeverEmitted
    );
}

/// Every physical port is listed with a name an operator recognises (its alias, else its
/// short name), and the server's period is the default buffer.
#[test]
fn enumerate_names_every_port() {
    let b = backend("ac2-test-names", false);
    if server_or_skip(&b).is_none() {
        return;
    }
    let caps = b.enumerate().expect("caps");
    let dev = &caps[0];
    for d in [dev.input.as_ref(), dev.output.as_ref()]
        .into_iter()
        .flatten()
    {
        let names = d.channel_names.as_ref().expect("JACK names its ports");
        assert_eq!(names.len(), usize::from(d.max_channels));
        assert!(names.iter().all(|n| !n.is_empty()), "{names:?}");
        assert_eq!(d.default_buffer, d.buffer_frames.map(|f| f.min));
    }
    // The dummy driver's ports have no aliases: their short names are listed.
    if std::env::var_os("AC2_JACK_DUMMY").is_some()
        && let Some(i) = &dev.input
    {
        assert_eq!(
            i.channel_names
                .as_ref()
                .and_then(|n| n.first())
                .map(String::as_str),
            Some("capture_1")
        );
    }
}

#[test]
fn server_rate_and_size_are_fixed() {
    let b = backend("ac2-test-fixed", false);
    if server_or_skip(&b).is_none() {
        return;
    }
    let caps = b.enumerate().expect("caps");
    let rate = caps[0].input.as_ref().expect("input").rates[0].min;
    let mut req = DuplexRequest::new(vec![0], 1, level());
    req.sample_rate = Some(rate + 1);
    assert!(matches!(
        b.open(req),
        Err(AudioError::Unsupported(Unsupported::SampleRate { .. }))
    ));
}

#[test]
fn generator_loopback_through_graph_has_constant_offset() {
    if std::env::var_os("AC2_JACK_DUMMY").is_none() {
        eprintln!("skipping: emitting JACK test needs AC2_JACK_DUMMY=1 (dummy driver only)");
        return;
    }
    let b = backend("ac2-test-loop", false);
    if server_or_skip(&b).is_none() {
        return;
    }
    let (mut g, port) = generator([0]).expect("routes");
    g.set_source(Box::new(WhiteNoise::new(0.05, 7)))
        .expect("queue");
    let mut req = DuplexRequest::new(vec![0], 1, level());
    req.output = OutputSource::Generator(port);
    req.history = Some(HistoryRequest::channel(0));
    req.ring_seconds = 5.0;
    let mut s = b.open(req).expect("open");

    let (patch, _) = jack::Client::new("ac2-test-patch", jack::ClientOptions::NO_START_SERVER)
        .expect("patch client");
    patch
        .connect_ports_by_name("ac2-test-loop:out_1", "ac2-test-loop:in_1")
        .expect("graph loopback");
    g.start();

    let blocks = collect(&mut s, Duration::from_millis(1500));
    let history = s.history().expect("history").clone();
    let mut lags = Vec::new();
    // Correlate two windows, a second apart, skipping blocks after any break.
    for at in [blocks.len() / 3, blocks.len() * 2 / 3] {
        let (h, data) = &blocks[at];
        if h.flags.breaks_continuity() {
            continue;
        }
        let len = data.len();
        let mut best = (f64::MIN, 0i64);
        for lag in 0..4096i64 {
            let start = h.start_sample as i64 - lag;
            if start < 0 {
                break;
            }
            let mut out = vec![0.0f32; len];
            if history.read(start as u64, &mut out).is_err() {
                continue;
            }
            let score: f64 = data
                .iter()
                .zip(&out)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum();
            if score > best.0 {
                best = (score, lag);
            }
        }
        lags.push(best.1);
    }
    g.stop();
    assert!(!lags.is_empty());
    assert!(
        lags.windows(2).all(|w| w[0] == w[1]),
        "offset drifted: {lags:?}"
    );
    eprintln!("graph loopback offset: {lags:?} samples");
    assert_eq!(s.stop(Duration::from_millis(500)), StopOutcome::FadedOut);
}

/// The output patch connects exactly the chosen outputs to the physical playback ports of
/// the same number, never removes a connection it did not make, and a routing change needs
/// no reopen: a port connection made by someone else survives it.
#[test]
fn output_patch_connects_chosen_outputs_only() {
    if std::env::var_os("AC2_JACK_DUMMY").is_none() {
        eprintln!("skipping: patching playback ports needs AC2_JACK_DUMMY=1 (dummy driver only)");
        return;
    }
    let b = backend("ac2-test-patch-out", false);
    if server_or_skip(&b).is_none() {
        return;
    }
    let (other, _) = jack::Client::new("ac2-test-other", jack::ClientOptions::NO_START_SERVER)
        .expect("other client");
    let playback = other.ports(
        None,
        Some("32 bit float mono audio"),
        jack::PortFlags::IS_PHYSICAL | jack::PortFlags::IS_INPUT,
    );
    if playback.len() < 2 {
        eprintln!("skipping: the dummy server has fewer than 2 playback ports");
        return;
    }
    let (mut g, port) = generator(Vec::<u16>::new()).expect("routes");
    g.set_source(Box::new(WhiteNoise::new(0.05, 7)))
        .expect("queue");
    let mut req = DuplexRequest::new(vec![0, 1], 2, level());
    req.output = OutputSource::Generator(port);
    let mut s = b.open(req).expect("open");
    let patch = s.output_patch().expect("JACK streams have a patch");
    let ours = |k: usize| format!("ac2-test-patch-out:out_{}", k + 1);
    let linked = |a: &str, b: &str| {
        other
            .port_by_name(a)
            .is_some_and(|p| p.is_connected_to(b).unwrap_or(false))
    };
    // Nothing is connected at open.
    assert!(!linked(&ours(0), &playback[0]) && !linked(&ours(1), &playback[1]));

    let st = patch.connect_only(&[0, 7]).expect("patch");
    assert_eq!(st.unmatched, [7], "the stream has no output 8");
    assert_eq!(st.links.len(), 1);
    assert_eq!(
        (st.links[0].from.as_str(), st.links[0].to.as_str()),
        (ours(0).as_str(), playback[0].as_str())
    );
    assert!(linked(&ours(0), &playback[0]));
    assert!(
        !linked(&ours(1), &playback[1]),
        "never an output not chosen"
    );

    // The operator patches output 2 by hand and records output 1 and 2 (graph loopback
    // into our own inputs).
    other
        .connect_ports_by_name(&ours(1), &playback[1])
        .expect("manual patch");
    other
        .connect_ports_by_name(&ours(0), "ac2-test-patch-out:in_1")
        .expect("recorder 1");
    other
        .connect_ports_by_name(&ours(1), "ac2-test-patch-out:in_2")
        .expect("recorder 2");
    g.set_routes(&[0]).expect("routes");
    g.start();
    let level_of = |blocks: &[(ac2_audio::BlockHeader, Vec<f32>)], ch: usize| {
        let tail = &blocks[blocks.len() / 2..];
        tail.iter()
            .flat_map(|(_, d)| d.iter().skip(ch).step_by(2))
            .fold(0.0f32, |m, v| m.max(v.abs()))
    };
    let first = collect(&mut s, Duration::from_millis(400));
    assert!(level_of(&first, 0) > 0.05, "plays on output 1");
    assert_eq!(level_of(&first, 1), 0.0);

    // Re-routing to output 2: no reopen, so every connection is still there.
    g.set_routes(&[1]).expect("routes");
    let st = patch.connect_only(&[1]).expect("patch");
    assert_eq!(st.links.len(), 1);
    assert!(
        !linked(&ours(0), &playback[0]),
        "its own connection is removed"
    );
    assert!(linked(&ours(1), &playback[1]), "the manual one is kept");
    assert!(linked(&ours(0), "ac2-test-patch-out:in_1"));
    let second = collect(&mut s, Duration::from_millis(400));
    assert_eq!(level_of(&second, 0), 0.0);
    assert!(level_of(&second, 1) > 0.05, "plays on output 2");

    // The manual connection was not the patch's: dropping output 2 leaves it alone.
    patch.connect_only(&[]).expect("patch");
    assert!(linked(&ours(1), &playback[1]));
    g.stop();
    assert_eq!(s.stop(Duration::from_millis(500)), StopOutcome::FadedOut);
    assert!(
        patch.connect_only(&[0]).is_err(),
        "a closed stream is never patched"
    );
}

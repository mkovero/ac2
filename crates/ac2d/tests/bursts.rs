//! A host that delivers audio in bursts is named in the daemon log, once per minute, with
//! the backend, device and buffer; a steady stream is not. The fake rig is stepped by hand:
//! a burst is 400 ms of audio produced at once after 400 ms of nothing.
#![allow(clippy::unwrap_used)]

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ac2_proto::Command;
use ac2d::Daemon;

use common::*;

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Log {
    fn warnings(&self) -> usize {
        String::from_utf8_lossy(&self.0.lock().unwrap())
            .matches("audio arrives in bursts")
            .count()
    }
}

#[test]
fn bursty_delivery_is_named_once_per_minute() {
    let log = Log::default();
    let w = log.clone();
    tracing_subscriber::fmt()
        .with_writer(move || w.clone())
        .with_ansi(false)
        .with_env_filter("warn")
        .init();
    let fake = manual_rig();
    let h = Daemon::start(config(fake.clone(), inproc("bursts"))).unwrap();
    let (mut c, _s) = connect(&h, &[]);
    c.ok(Command::SessionOpen {
        config: session(true),
    });
    let mut d = driver(&fake);

    // Steady: 50 ms steps, never a pause.
    run(&mut d, 1.0);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(log.warnings(), 0);

    // Bursts: nothing for 400 ms, then 400 ms of audio at once.
    let lump = (0.4 * f64::from(FS) / f64::from(BLOCK)).ceil() as u64;
    for _ in 0..4 {
        std::thread::sleep(Duration::from_millis(400));
        d.run_blocks(lump);
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(log.warnings(), 1, "one warning per minute");
    let text = String::from_utf8_lossy(&log.0.lock().unwrap()).into_owned();
    let line = text
        .lines()
        .find(|l| l.contains("audio arrives in bursts"))
        .unwrap();
    for want in ["Fake device \"fake\"", "256-frame buffer", "smaller buffer"] {
        assert!(line.contains(want), "{line}");
    }
    h.shutdown();
}

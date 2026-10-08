//! The JACK client around [`Device`]. It registers four ports and never connects them: the
//! caller does all patching, so nothing is ever routed to a real output by this program.

use ac2_jack_dut::cli::Config;
use ac2_jack_dut::dsp::Device;
use jack::{AudioIn, AudioOut, Client, ClientOptions, ClientStatus, Control, Port, ProcessScope};
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

struct Process {
    ref_in: Port<AudioIn>,
    ref_out: Port<AudioOut>,
    dut_in: Port<AudioIn>,
    dut_out: Port<AudioOut>,
    device: Device,
}

impl jack::ProcessHandler for Process {
    fn process(&mut self, _: &Client, ps: &ProcessScope) -> Control {
        self.device.process(
            self.ref_in.as_slice(ps),
            self.ref_out.as_mut_slice(ps),
            self.dut_in.as_slice(ps),
            self.dut_out.as_mut_slice(ps),
        );
        Control::Continue
    }
}

struct Notifications {
    xruns: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
}

impl jack::NotificationHandler for Notifications {
    fn xrun(&mut self, _: &Client) -> Control {
        self.xruns.fetch_add(1, Ordering::Relaxed);
        Control::Continue
    }

    unsafe fn shutdown(&mut self, _: ClientStatus, _: &str) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Runs the client until a signal, end of stdin or server shutdown; returns the exit code.
pub fn run(config: Config) -> i32 {
    // Before connecting, so a signal that arrives right after `ready` still stops cleanly.
    let stop = Arc::new(AtomicBool::new(false));
    let s = Arc::clone(&stop);
    if let Err(e) = ctrlc::set_handler(move || s.store(true, Ordering::Relaxed)) {
        eprintln!("ac2-jack-dut: cannot install signal handler: {e}");
    }
    let client = match Client::new(&config.name, ClientOptions::NO_START_SERVER) {
        Ok((c, _)) => c,
        Err(e) => {
            eprintln!("ac2-jack-dut: cannot connect to a JACK server ({e}); is jackd running?");
            return 1;
        }
    };
    let ports = (|| {
        Ok::<_, jack::Error>((
            client.register_port("ref_in", AudioIn::default())?,
            client.register_port("ref_out", AudioOut::default())?,
            client.register_port("dut_in", AudioIn::default())?,
            client.register_port("dut_out", AudioOut::default())?,
        ))
    })();
    let (ref_in, ref_out, dut_in, dut_out) = match ports {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ac2-jack-dut: cannot register ports: {e}");
            return 1;
        }
    };

    let xruns = Arc::new(AtomicU64::new(0));
    let process = Process {
        ref_in,
        ref_out,
        dut_in,
        dut_out,
        device: Device::new(config.pre, config.poly, config.post, config.noise_rms),
    };
    let notifications = Notifications {
        xruns: Arc::clone(&xruns),
        stop: Arc::clone(&stop),
    };
    let active = match client.activate_async(notifications, process) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ac2-jack-dut: cannot activate: {e}");
            return 1;
        }
    };

    {
        let c = active.as_client();
        let mut out = std::io::stdout().lock();
        let _ = writeln!(
            out,
            "ready {} {} {}",
            c.name(),
            c.sample_rate(),
            c.buffer_size()
        );
        let _ = out.flush();
    }

    let s = Arc::clone(&stop);
    std::thread::spawn(move || {
        let mut sink = [0u8; 256];
        let mut stdin = std::io::stdin().lock();
        while matches!(stdin.read(&mut sink), Ok(n) if n > 0) {}
        s.store(true, Ordering::Relaxed);
    });

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(20));
    }

    let code = match active.deactivate() {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("ac2-jack-dut: deactivate failed: {e}");
            1
        }
    };
    println!("xruns {}", xruns.load(Ordering::Relaxed));
    code
}

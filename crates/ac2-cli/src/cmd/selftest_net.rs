//! `selftest net`: the network timing stream ([`crate::selftest::net`]) and its closing
//! summary.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ac2_zmq::PublicKey;

use crate::args::SelftestNet;
use crate::output::Out;
use crate::selftest::net::{self, NetError, NetMeta, NetSpec, Quantiles, Role, WINDOW};
use crate::selftest::stamps::Provenance;
use crate::watch::quit_signal;
use crate::{BUILD_ID, CliError};

/// Longest run: a day of arrivals is 1 GB at 96 kHz / 256 frames.
const MAX_MINUTES: f64 = 24.0 * 60.0;

pub(super) async fn net(a: &SelftestNet, out: &mut Out<'_>) -> Result<(), CliError> {
    if !(a.minutes.is_finite() && a.minutes > 0.0 && a.minutes <= MAX_MINUTES) {
        return Err(CliError::Usage(format!(
            "--minutes: more than 0 and at most {MAX_MINUTES:.0}"
        )));
    }
    if a.rate == 0 || a.frames == 0 || a.channels == 0 {
        return Err(CliError::Usage(
            "--rate, --frames and --channels: at least 1".into(),
        ));
    }
    let spec = NetSpec {
        role: a.role,
        rate: a.rate,
        frames: a.frames,
        channels: a.channels,
        duration: Duration::from_secs_f64(a.minutes * 60.0),
        window: WINDOW,
        credit_pause: None,
    };
    // Credit could never cover a larger block: the sender would stall for good.
    if spec.message_bytes() > u64::from(WINDOW) {
        return Err(CliError::Usage(format!(
            "a block of {} bytes does not fit the {} KiB credit window; fewer --frames or \
             --channels",
            spec.message_bytes(),
            WINDOW / 1024
        )));
    }
    let ep = match (&a.listen, &a.connect, &a.server_key) {
        (Some(addr), None, None) => {
            let ep = net::listen(addr).map_err(error)?;
            let file = net::write_server_key(&a.out, ep.server_key()).map_err(error)?;
            eprintln!(
                "ac2: listening on {} as --role {}; on the other host pass\n  --server-key '{}'\n\
                 (also in {})",
                ep.addr(),
                role_name(a.role),
                ep.server_key().to_z85(),
                file.display()
            );
            ep
        }
        (None, Some(addr), Some(key)) => {
            let key = PublicKey::from_z85(key.trim())
                .map_err(|_| CliError::Usage("--server-key: not a Z85 CURVE public key".into()))?;
            net::connect(addr, key).map_err(error)?
        }
        _ => {
            return Err(CliError::Usage(
                "either --listen, or --connect with --server-key".into(),
            ));
        }
    };
    let provenance = Provenance {
        host: ac2_discovery::local_host_label(),
        ac2_build: BUILD_ID.into(),
    };
    eprintln!(
        "ac2: {} for {} min into {}, no audio (Ctrl-C ends early)",
        match a.role {
            Role::Send => "sending blocks",
            Role::Receive => "receiving blocks",
        },
        a.minutes,
        a.out.display()
    );
    let stop = Arc::new(AtomicBool::new(false));
    let mut task = {
        let (stop, dir) = (Arc::clone(&stop), a.out.clone());
        tokio::task::spawn_blocking(move || net::run(ep, &spec, &dir, provenance, &stop))
    };
    let joined = tokio::select! {
        r = &mut task => r,
        () = quit_signal() => {
            stop.store(true, Ordering::Release);
            task.await
        }
    };
    let meta = joined
        .map_err(|e| CliError::Io(std::io::Error::other(e)))?
        .map_err(error)?;
    out.emit(&meta, || text(&meta))?;
    Ok(())
}

fn error(e: NetError) -> CliError {
    match e {
        NetError::Io(e) => CliError::Io(e),
        other => CliError::Usage(other.to_string()),
    }
}

fn role_name(r: Role) -> &'static str {
    match r {
        Role::Send => "send",
        Role::Receive => "receive",
    }
}

fn q(s: &mut String, label: &str, q: &Quantiles) {
    let _ = writeln!(
        s,
        "  {label:<17}p50 {:.0} µs, p99 {:.0} µs, p99.9 {:.0} µs, max {:.0} µs",
        q.p50_us, q.p99_us, q.p999_us, q.max_us
    );
}

/// The closing summary a tester can paste.
pub(crate) fn text(m: &NetMeta) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "ac2 selftest net: {} --role {} ({}, {}), peer {}",
        m.provenance.host,
        role_name(m.role),
        match m.side {
            net::Side::Listen => "listening",
            net::Side::Connect => "connected",
        },
        m.addr,
        m.peer_host
    );
    let _ = writeln!(
        s,
        "  stream           {} Hz, {} frames × {} ch: {} B per block, every {:.0} µs, \
         window {} KiB",
        m.sample_rate,
        m.frames,
        m.channels,
        m.message_bytes,
        m.block_period_us,
        m.window_bytes / 1024
    );
    if let Some(e) = &m.sent {
        let _ = writeln!(
            s,
            "  sent             {} blocks in {:.1} s{}{}",
            e.blocks,
            e.seconds,
            if e.interrupted { " (interrupted)" } else { "" },
            if e.peer_gone { " (receiver gone)" } else { "" }
        );
        let _ = writeln!(
            s,
            "  credit stalls    {} ({:.1} ms), {} blocks waited for credit",
            e.stalls, e.stall_ms, e.credit_waits
        );
        let _ = writeln!(
            s,
            "  late sends       {} more than one block late, latest {:.0} µs",
            e.late_sends, e.max_late_us
        );
    }
    if let Some(r) = &m.received {
        let _ = writeln!(
            s,
            "  received         {} blocks in {:.1} s (ended: {}), {} missing, {} reordered",
            r.received,
            r.seconds,
            match r.ended {
                net::Ended::Sender => "sender's end",
                net::Ended::Deadline => "no end from the sender",
                net::Ended::Interrupted => "interrupted",
            },
            r.missing,
            r.reordered
        );
        let (stalls, late) = r
            .sender
            .as_ref()
            .map_or((r.in_band.stalls, r.in_band.late_sends), |e| {
                (e.stalls, e.late_sends)
            });
        let _ = writeln!(
            s,
            "  sender reported  {stalls} credit stalls, {late} late sends"
        );
        if let Some(sum) = &r.summary {
            let _ = writeln!(
                s,
                "  one-way floor    {:.0} µs (arrived − sent; holds the hosts' clock offset)",
                sum.one_way_min_us
            );
            q(&mut s, "one-way jitter", &sum.one_way);
            q(&mut s, "vs schedule", &sum.vs_schedule);
            q(&mut s, "interval jitter", &sum.interval_jitter);
            let _ = writeln!(
                s,
                "  link offset      {:.2} ms (one-way p99.9 + one block)",
                sum.link_offset_ms
            );
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use clap::Parser;

    use crate::Cli;
    use crate::output::Out;

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("port")
            .port()
    }

    /// Both ends of the command on loopback, the receiver listening: the key goes through
    /// `server.key`, and the receiver's `--json` summary parses with every block counted.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_command_runs_both_ends_and_summarises() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (rx_dir, tx_dir) = (tmp.path().join("rx"), tmp.path().join("tx"));
        let listen = format!("127.0.0.1:{}", free_port());
        let common = ["--minutes", "0.02", "--rate", "96000", "--frames", "128"];
        let rx_cli = {
            let mut v = vec!["ac2", "--json", "selftest", "net", "--listen", &listen];
            v.extend_from_slice(&["--role", "receive"]);
            v.extend_from_slice(&["--out", rx_dir.to_str().expect("utf-8")]);
            v.extend_from_slice(&common);
            Cli::parse_from(v)
        };
        let (mut rx_out, mut tx_out) = (Vec::new(), Vec::new());
        let receiver = async {
            let mut out = Out::new(true, &mut rx_out);
            crate::run(&rx_cli, &mut out).await
        };
        let sender = async {
            let key_file = rx_dir.join("server.key");
            let key = loop {
                match std::fs::read_to_string(&key_file) {
                    Ok(k) if k.ends_with('\n') => break k.trim().to_owned(),
                    _ => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            };
            let mut v = vec!["ac2", "selftest", "net", "--connect", &listen];
            v.extend_from_slice(&["--server-key", &key, "--role", "send"]);
            v.extend_from_slice(&["--out", tx_dir.to_str().expect("utf-8")]);
            v.extend_from_slice(&common);
            let cli = Cli::parse_from(v);
            let mut out = Out::new(false, &mut tx_out);
            crate::run(&cli, &mut out).await
        };
        let (r, s) = tokio::join!(receiver, sender);
        r.expect("receiver");
        s.expect("sender");
        let rx: serde_json::Value = serde_json::from_slice(&rx_out).expect("json summary");
        // 1.2 s at 750 blocks a second.
        assert_eq!(rx["received"]["received"], 900);
        assert_eq!(rx["received"]["missing"], 0);
        let offset = rx["received"]["summary"]["link_offset_ms"].as_f64();
        assert!(offset.expect("offset") > 1.3, "{rx}");
        let text = String::from_utf8(tx_out).expect("utf-8");
        assert!(text.starts_with("ac2 selftest net:"), "{text}");
        assert!(text.contains("sent             900 blocks"), "{text}");
    }

    #[test]
    fn a_server_key_starting_with_a_hyphen_is_the_value() {
        let key = "-/abcdefghijklmnopqrstuvwxyz0123456789AB";
        let cli = Cli::try_parse_from([
            "ac2",
            "selftest",
            "net",
            "--connect",
            "127.0.0.1:1",
            "--server-key",
            key,
            "--role",
            "send",
            "--minutes",
            "1",
            "--out",
            "x",
        ])
        .expect("parses");
        let crate::args::Cmd::Selftest {
            cmd: crate::args::SelftestCmd::Net(a),
            ..
        } = cli.cmd
        else {
            panic!("not selftest net");
        };
        assert_eq!(a.server_key.as_deref(), Some(key));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_block_larger_than_the_window_is_refused() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("x");
        let cli = Cli::parse_from([
            "ac2",
            "selftest",
            "net",
            "--listen",
            "127.0.0.1:0",
            "--role",
            "send",
            "--minutes",
            "1",
            "--frames",
            "8192",
            "--channels",
            "4",
            "--out",
            dir.to_str().expect("utf-8"),
        ]);
        let mut so = Vec::new();
        let mut out = Out::new(false, &mut so);
        assert!(crate::run(&cli, &mut out).await.is_err());
        assert!(!dir.exists());
    }
}

//! `--json` output of commands run in-process against the fake daemon.

mod daemon;
mod gen_;
mod input;
mod kinds;
mod meas;
mod trace;

use ac2_cli::{Cli, Out, run_reporting};
use ac2_client::fake::{FakeDaemon, FakeOptions};
use clap::Parser;
use serde_json::Value;

type R<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Run {
    code: u8,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> R<Value> {
        serde_json::from_str(&self.stdout)
            .map_err(|e| format!("{e}: stdout {:?} stderr {:?}", self.stdout, self.stderr).into())
    }
}

async fn ac2(f: &FakeDaemon, args: &[&str]) -> R<Run> {
    let ep = f.endpoints();
    let argv: Vec<&str> = [
        "ac2",
        "--ctrl-endpoint",
        &ep.ctrl,
        "--data-endpoint",
        &ep.data,
    ]
    .into_iter()
    .chain(args.iter().copied())
    .collect();
    let cli = Cli::try_parse_from(argv)?;
    let (mut so, mut se) = (Vec::new(), Vec::new());
    let code = {
        let mut out = Out::new(cli.json, &mut so);
        run_reporting(&cli, &mut out, &mut se).await
    };
    Ok(Run {
        code,
        stdout: String::from_utf8(so)?,
        stderr: String::from_utf8(se)?,
    })
}

async fn ok_json(f: &FakeDaemon, args: &[&str]) -> R<Value> {
    let r = ac2(f, args).await?;
    if r.code != 0 {
        return Err(format!("{args:?} exited {}: {} {}", r.code, r.stdout, r.stderr).into());
    }
    r.json()
}

fn fake() -> R<FakeDaemon> {
    Ok(FakeDaemon::start(FakeOptions::default())?)
}

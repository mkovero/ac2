//! Command line: parsed with clap, then validated into a [`Config`].

use crate::dsp::{Biquad, Noise};
use clap::Parser;

const AFTER_HELP: &str = "\
Paths:
  ref_out = ref_in + noise
  dut_out = post(poly(pre(dut_in))) + noise

Biquad coefficients are normalised so a0 = 1 and given as b0,b1,b2,a1,a2:
  y[n] = b0 x[n] + b1 x[n-1] + b2 x[n-2] - a1 y[n-1] - a2 y[n-2]
  H(z) = (b0 + b1 z^-1 + b2 z^-2) / (1 + a1 z^-1 + a2 z^-2)
Realised as Direct Form II transposed, in f64. Sections run in the order given.

poly(u) = c0 + c1 u + c2 u^2 + ...  (Horner, f64).

Noise is Gaussian white noise, independent on each output, fixed seeds. Its level is in
dBFS where 0 dBFS is a full-scale sine of peak 1.0, so noise RMS = 10^(dB/20)/sqrt(2).

Outputs are hard-limited to +-1.0 as a defence only; keep signals well below it.

The client never connects its ports: the caller patches ref_in, ref_out, dut_in, dut_out.
After activation it prints one line `ready <client_name> <sample_rate> <buffer_size>` and
runs until SIGINT/SIGTERM or end of stdin, then prints `xruns <n>`.";

#[derive(Debug, Parser)]
#[command(
    name = "ac2-jack-dut",
    about = "Software device under test: a JACK client with exactly known filters and harmonic distortion.",
    after_help = AFTER_HELP
)]
struct Args {
    /// JACK client name.
    #[arg(long, default_value = "ac2-dut")]
    name: String,
    /// Polynomial coefficients c0,c1,c2,... (at least c0,c1).
    #[arg(long, required = true, allow_hyphen_values = true)]
    poly: String,
    /// A biquad b0,b1,b2,a1,a2 before the polynomial; repeatable.
    #[arg(long, allow_hyphen_values = true)]
    pre: Vec<String>,
    /// A biquad b0,b1,b2,a1,a2 after the polynomial; repeatable.
    #[arg(long, allow_hyphen_values = true)]
    post: Vec<String>,
    /// Noise level in dBFS (0 dBFS = sine of peak 1.0), or `off`.
    #[arg(long, allow_hyphen_values = true)]
    noise_dbfs: Option<String>,
}

/// A validated configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub name: String,
    pub poly: Vec<f64>,
    pub pre: Vec<Biquad>,
    pub post: Vec<Biquad>,
    /// Noise RMS on each output; 0 for no noise.
    pub noise_rms: f64,
}

fn numbers(flag: &str, s: &str) -> Result<Vec<f64>, String> {
    s.split(',')
        .map(|t| {
            let t = t.trim();
            let v: f64 = t
                .parse()
                .map_err(|_| format!("--{flag}: `{t}` is not a number"))?;
            if v.is_finite() {
                Ok(v)
            } else {
                Err(format!("--{flag}: `{t}` is not finite"))
            }
        })
        .collect()
}

fn biquad(flag: &str, s: &str) -> Result<Biquad, String> {
    let v = numbers(flag, s)?;
    let [b0, b1, b2, a1, a2] = v[..] else {
        return Err(format!(
            "--{flag} {s}: needs exactly five numbers b0,b1,b2,a1,a2 (a0 = 1), got {}",
            v.len()
        ));
    };
    let b = Biquad::new(b0, b1, b2, a1, a2);
    if !b.is_stable() {
        return Err(format!(
            "--{flag} {s}: unstable, poles on or outside the unit circle \
             (need |a2| < 1 and |a1| < 1 + a2)"
        ));
    }
    Ok(b)
}

impl Config {
    /// Parses and validates a full argument list (first item is the program name).
    pub fn parse_from<I, T>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let a = Args::try_parse_from(args).map_err(|e| e.to_string())?;
        Self::validate(a)
    }

    /// Parses the process arguments; prints help or the error and exits on failure.
    pub fn from_env() -> Self {
        let a = Args::parse();
        Self::validate(a).unwrap_or_else(|e| {
            eprintln!("error: {e}");
            std::process::exit(2)
        })
    }

    fn validate(a: Args) -> Result<Self, String> {
        let poly = numbers("poly", &a.poly)?;
        if poly.len() < 2 {
            return Err("--poly: needs at least c0,c1".into());
        }
        let pre = a
            .pre
            .iter()
            .map(|s| biquad("pre", s))
            .collect::<Result<_, _>>()?;
        let post = a
            .post
            .iter()
            .map(|s| biquad("post", s))
            .collect::<Result<_, _>>()?;
        let noise_rms = match a.noise_dbfs.as_deref().map(str::trim) {
            None | Some("off") => 0.0,
            Some(s) => {
                let v = numbers("noise-dbfs", s)?;
                let [db] = v[..] else {
                    return Err(format!("--noise-dbfs: `{s}` is not one number"));
                };
                if db > 0.0 {
                    return Err(format!("--noise-dbfs: {db} is above full scale"));
                }
                Noise::rms_for_dbfs(db)
            }
        };
        if a.name.is_empty() {
            return Err("--name: empty".into());
        }
        Ok(Self {
            name: a.name,
            poly,
            pre,
            post,
            noise_rms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Config, String> {
        Config::parse_from(std::iter::once("ac2-jack-dut").chain(args.iter().copied()))
    }

    #[test]
    fn good_inputs() {
        let c = parse(&["--poly", "0,1,0.02,0.04"]).expect("parses");
        assert_eq!(c.name, "ac2-dut");
        assert_eq!(c.poly, vec![0.0, 1.0, 0.02, 0.04]);
        assert!(c.pre.is_empty() && c.post.is_empty());
        assert_eq!(c.noise_rms, 0.0);

        let c = parse(&[
            "--name",
            "x",
            "--poly",
            "-0.001,1",
            "--pre",
            "1,0,0,-0.5,0.25",
            "--pre",
            "0.5,0.5,0,0,0",
            "--post",
            "1,-2,1,-1.9,0.905",
            "--noise-dbfs",
            "-120",
        ])
        .expect("parses");
        assert_eq!(c.name, "x");
        assert_eq!(c.poly, vec![-0.001, 1.0]);
        assert_eq!(c.pre.len(), 2);
        assert_eq!(c.pre[1].b0, 0.5);
        assert_eq!(c.post[0].a2, 0.905);
        assert!((c.noise_rms - 1e-6 / std::f64::consts::SQRT_2).abs() < 1e-18);

        let c = parse(&["--poly", "0,1", "--noise-dbfs", "off"]).expect("parses");
        assert_eq!(c.noise_rms, 0.0);
    }

    #[test]
    fn bad_inputs() {
        for args in [
            &["--pre", "1,0,0,0,0"][..],                     // missing --poly
            &["--poly", "0"],                                // too short
            &["--poly", "0,1,abc"],                          // not a number
            &["--poly", "0,inf"],                            // not finite
            &["--poly", "0,NaN"],                            // not finite
            &["--poly", "0,1", "--pre", "1,0,0,0,1"],        // poles on |z| = 1
            &["--poly", "0,1", "--post", "1,0,0,-2.1,1.05"], // poles outside
            &["--poly", "0,1", "--pre", "1,0,0,0"],          // four numbers
            &["--poly", "0,1", "--noise-dbfs", "loud"],
            &["--poly", "0,1", "--noise-dbfs", "3"],
            &["--poly", "0,1", "--bogus"],
        ] {
            assert!(parse(args).is_err(), "accepted {args:?}");
        }
    }
}

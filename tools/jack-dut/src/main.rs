//! `ac2-jack-dut`: a software device under test with exactly known filters and harmonic
//! distortion, for comparing harmonic measurements against an analytic truth. See
//! `ac2_jack_dut::dsp` for the model and the coefficient convention, `--help` for the CLI.

#[cfg(target_os = "linux")]
mod host;

use ac2_jack_dut::cli;

#[cfg(target_os = "linux")]
fn main() {
    let config = cli::Config::from_env();
    std::process::exit(host::run(config));
}

#[cfg(not(target_os = "linux"))]
fn main() {
    // Parse anyway so `--help` and argument errors behave the same on every OS.
    let _ = cli::Config::from_env();
    eprintln!("ac2-jack-dut needs JACK (Linux only)");
    std::process::exit(2);
}

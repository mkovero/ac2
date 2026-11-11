//! `ac2`: command-line client of the ac2 daemon.
#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    ac2_cli::main_entry()
}

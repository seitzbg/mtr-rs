//! Entry point: exit 1 with the message on any fatal error, as `error(EXIT_FAILURE, …)` does
//! in packet/packet.c (mtr 0.96, commit 7b01773). GPL-2.0-only.
use std::process::ExitCode;

fn main() -> ExitCode {
    match mtr_packet::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mtr-packet: {e}");
            ExitCode::FAILURE
        }
    }
}

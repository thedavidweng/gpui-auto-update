//! `gpui-auto-update` command-line tool.
//!
//! Release and integration tooling for applications that use
//! `gpui-auto-update`: project initialization, doctor checks, signing key
//! management, feed generation, and release verification. It is not needed
//! at application runtime.

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--version" | "-V") => {
            println!("gpui-auto-update {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") | None => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("error: unrecognized argument `{other}`\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

const USAGE: &str = "\
Release and integration tooling for gpui-auto-update.

Usage: gpui-auto-update [--help | --version]

No subcommands are available yet.";

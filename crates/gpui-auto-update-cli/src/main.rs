//! `gpui-auto-update` command-line tool.
//!
//! Release and integration tooling for applications that use
//! `gpui-auto-update`: project initialization, doctor checks, signing key
//! management, feed generation, and release verification. It is not needed
//! at application runtime.

#![forbid(unsafe_code)]

mod sparkle;

use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};

/// Release and integration tooling for gpui-auto-update.
#[derive(Debug, Parser)]
#[command(name = "gpui-auto-update", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Acquire, embed, sign, and validate the Sparkle framework (macOS).
    Sparkle(sparkle::SparkleArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        None => {
            println!("{}", Cli::command().render_help());
            return ExitCode::SUCCESS;
        }
        Some(Command::Sparkle(args)) => sparkle::run(args),
    };
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

//! `gpui-auto-update` command-line tool.
//!
//! Release and integration tooling for applications that use
//! `gpui-auto-update`: project initialization, doctor checks, signing key
//! management, feed generation, and release verification. It is not needed
//! at application runtime.

#![forbid(unsafe_code)]

mod keys;
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
    /// Manage Sparkle-compatible Ed25519 signing keys.
    Keys(KeysArgs),
    /// Acquire, embed, sign, and validate the Sparkle framework (macOS).
    Sparkle(sparkle::SparkleArgs),
}

/// Arguments after `keys`, handed unparsed to the keys parser.
///
/// Clap's error messages quote offending arguments, which could echo a
/// private key mistakenly passed on the command line. The keys parser
/// rejects such arguments without repeating them, so clap must not see them.
#[derive(Debug, clap::Args)]
#[command(disable_help_flag = true)]
struct KeysArgs {
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        num_args = 0..,
        hide = true
    )]
    args: Vec<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        None => {
            println!("{}", Cli::command().render_help());
            return ExitCode::SUCCESS;
        }
        Some(Command::Keys(args)) => return keys::run(args.args.into_iter()),
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

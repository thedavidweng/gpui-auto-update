//! `gpui-auto-update sparkle ...`: Sparkle 2 framework packaging for macOS.

mod bundle;
mod codesign;
mod embed;
mod fetch;
mod macho;
mod pins;
mod plist;
mod sign;
mod validate;

use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};

use pins::Pins;

#[derive(Debug, Args)]
pub struct SparkleArgs {
    #[command(subcommand)]
    command: SparkleCommand,
}

#[derive(Debug, Subcommand)]
enum SparkleCommand {
    /// List the pinned official Sparkle releases and their SHA-256 checksums.
    Versions,
    /// Download a Sparkle distribution and verify its SHA-256 before
    /// extracting it.
    Fetch(fetch::FetchArgs),
    /// Copy Sparkle.framework and its license notice into an app bundle.
    Embed(embed::EmbedArgs),
    /// Sign the embedded Sparkle code and the app, innermost first, with the
    /// hardened runtime.
    Sign(sign::SignArgs),
    /// Check Info.plist metadata, framework placement, run paths, sandbox
    /// requirements, license notice, and code signatures.
    Validate(validate::ValidateArgs),
}

/// How the application is sandboxed, which decides the XPC services Sparkle
/// needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SandboxMode {
    /// Not sandboxed: Sparkle's XPC services are removed.
    NonSandboxed,
    /// Sandboxed without the network-client entitlement: Installer.xpc and
    /// Downloader.xpc are both required.
    Sandboxed,
    /// Sandboxed with the `com.apple.security.network.client` entitlement:
    /// Installer.xpc is required, Downloader.xpc is not.
    SandboxedNetworkClient,
}

impl std::fmt::Display for SandboxMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = self.to_possible_value().expect("no variant is skipped");
        f.write_str(value.get_name())
    }
}

pub fn run(args: SparkleArgs) -> Result<ExitCode> {
    match args.command {
        SparkleCommand::Versions => versions(),
        SparkleCommand::Fetch(args) => fetch::run(args),
        SparkleCommand::Embed(args) => embed::run(args),
        SparkleCommand::Sign(args) => sign::run(args),
        SparkleCommand::Validate(args) => validate::run(args),
    }
}

fn versions() -> Result<ExitCode> {
    let pins = Pins::load()?;
    for pin in &pins.releases {
        let marker = if pin.version == pins.default {
            " (default)"
        } else {
            ""
        };
        println!("{} sha256:{} {}{marker}", pin.version, pin.sha256, pin.url);
    }
    Ok(ExitCode::SUCCESS)
}

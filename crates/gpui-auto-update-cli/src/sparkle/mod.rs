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

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand, ValueEnum};

pub use pins::Pins;
pub use validate::{check_unsigned_app, is_reverse_dns};

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

/// Path of `bin/<tool>` in a Sparkle distribution extracted by
/// `sparkle fetch`, after checking that the distribution's framework is a
/// pinned Sparkle release.
pub fn distribution_tool(dist: &Path, tool: &str) -> Result<PathBuf> {
    let framework = dist.join(bundle::FRAMEWORK_NAME);
    let version = bundle::framework_version(&framework).with_context(|| {
        format!(
            "{} is not a Sparkle distribution; extract one with `gpui-auto-update sparkle fetch`",
            dist.display()
        )
    })?;
    let pins = Pins::load()?;
    if pins.get(&version).is_none() {
        bail!(
            "{} contains Sparkle {version}, which is not a pinned release (pinned: {})",
            dist.display(),
            pins.releases
                .iter()
                .map(|p| p.version.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let path = dist.join("bin").join(tool);
    if !path.is_file() {
        bail!(
            "the Sparkle {version} distribution in {} has no bin/{tool}",
            dist.display()
        );
    }
    Ok(path)
}

/// The Sparkle version of a distribution extracted by `sparkle fetch`.
pub fn distribution_version(dist: &Path) -> Result<String> {
    bundle::framework_version(&dist.join(bundle::FRAMEWORK_NAME))
}

/// The string values of an application bundle's Info.plist.
pub fn app_info_strings(app: &Path) -> Result<std::collections::BTreeMap<String, String>> {
    let info = bundle::read_dict(&app.join("Contents/Info.plist"))?;
    Ok(info
        .iter()
        .filter_map(|(k, v)| v.as_string().map(|s| (k.clone(), s.to_owned())))
        .collect())
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

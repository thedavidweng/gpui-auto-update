//! `gpui-auto-update init`: inspect an application package and explain the
//! updater configuration it needs.
//!
//! Signing identities, application identifiers, installer formats, and
//! release hosts are the developer's decisions, so init never fills them in:
//! it reports what is configured, explains what is missing, and offers a
//! skeleton whose values are all placeholders that `doctor` rejects.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Args;
use gpui_auto_update_core::version::ReleaseVersion;

use crate::project::{self, Config, Project};

/// Inspect a package and explain the updater configuration it needs.
#[derive(Debug, Args)]
pub struct InitArgs {
    /// The application's Cargo.toml.
    #[arg(long, value_name = "PATH", default_value = "Cargo.toml")]
    manifest_path: PathBuf,
    /// Append the placeholder skeleton to the manifest when it has no
    /// configuration yet.
    #[arg(long)]
    write: bool,
}

const SKELETON: &str = r#"# Updater configuration, read by `gpui-auto-update doctor`. Replace every
# <...> placeholder, and delete the platform tables you do not ship.
[package.metadata.gpui-auto-update]
# Reverse-DNS identifier you own; on macOS it must equal CFBundleIdentifier.
app-id = "<reverse-DNS application identifier>"
# SUPublicEDKey printed by `gpui-auto-update keys generate` (or `keys import`).
public-key = "<base64 Ed25519 public key>"

[package.metadata.gpui-auto-update.macos]
# Where you publish the Sparkle appcast; must equal SUFeedURL.
feed-url = "<https URL of the Sparkle appcast>"
sandbox = "<non-sandboxed | sandboxed | sandboxed-network-client>"
# Optional: sparkle = "<directory from `gpui-auto-update sparkle fetch`>"
# Optional: app = "<path to the built .app bundle>"

[package.metadata.gpui-auto-update.windows]
# How updates are applied; see docs/windows-installers.md.
strategy = "<inno-setup | portable | custom>"
# One feed per architecture you ship (x86_64, aarch64).
feeds = { x86_64 = "<https URL of the windows x86_64 feed>" }
# Optional: artifacts = { x86_64 = "<path to the built installer or executable>" }

[package.metadata.gpui-auto-update.linux]
# Executable name of the managed install, <prefix>/bin/<app-name>.
app-name = "<executable name>"
feeds = { x86_64 = "<https URL of the linux x86_64 feed>" }
# Optional: artifacts = { x86_64 = "<path to the built .tar.gz release>" }
"#;

pub fn run(args: InitArgs) -> Result<ExitCode> {
    let project = Project::load(&args.manifest_path)?;
    let name = project.name.as_deref().unwrap_or("(unnamed)");
    match &project.version {
        Ok(version) => {
            println!("package: {name} {version}");
            if let Err(e) = ReleaseVersion::parse(version) {
                println!(
                    "  version {version:?} is not strict SemVer (MAJOR.MINOR.PATCH); the updater orders releases by it: {e}"
                );
            }
        }
        Err(message) => println!("package: {name} ({message})"),
    }
    if !project.depends_on_facade {
        println!(
            "  {name} does not depend on gpui-auto-update; add it with `cargo add gpui-auto-update`"
        );
    }
    let workflows = project.root.join(".github/workflows");
    if workflows.is_dir() {
        println!(
            "  CI workflows found: store the private key as a secret, expose it through `env:`, and pass --key-env <VAR> (docs/key-management.md)"
        );
    }
    println!();

    match &project.config {
        Some(Err(message)) => {
            println!("{message}");
            return Ok(ExitCode::FAILURE);
        }
        Some(Ok(config)) => {
            if args.write {
                eprintln!(
                    "error: {} already has a {} table; nothing was written",
                    project.manifest.display(),
                    project::SECTION
                );
                return Ok(ExitCode::FAILURE);
            }
            describe(config);
        }
        None if args.write => {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&project.manifest)
                .with_context(|| format!("cannot open {}", project.manifest.display()))?;
            write!(file, "\n{SKELETON}")
                .with_context(|| format!("cannot write {}", project.manifest.display()))?;
            println!(
                "Appended a placeholder {} table to {}.",
                project::SECTION,
                project.manifest.display()
            );
            explain();
        }
        None => {
            println!(
                "{} has no {} table yet. Add the following, then fill it in:\n",
                project.manifest.display(),
                project::SECTION
            );
            print!("{SKELETON}");
            println!();
            explain();
        }
    }
    println!("Then run `gpui-auto-update doctor` to validate the integration.");
    Ok(ExitCode::SUCCESS)
}

/// What each value is and who decides it.
fn explain() {
    println!(
        "Every value is your decision; init does not choose them:\n\
         - app-id: an identifier in a domain you control. On macOS it is the bundle's CFBundleIdentifier.\n\
         - public-key: run `gpui-auto-update keys generate`. Keep the private key out of the repository.\n\
         - feed-url / feeds: the https locations where you will publish feeds; the updater never guesses a host.\n\
         - strategy: inno-setup or portable for built-in Windows handoff, custom for your own installer.\n\
         - app-name: the Linux executable name; release archives ship bin/<app-name> and the ownership marker.\n"
    );
}

fn describe(config: &Config) {
    let show = |key: &str, value: Option<&str>| match value {
        Some(v) if project::is_placeholder(v) => println!("  {key} is still a placeholder"),
        Some(v) => println!("  {key}: {v}"),
        None => println!("  {key} is not set"),
    };
    println!("configured:");
    show("app-id", config.app_id.as_deref());
    match config.public_key.as_deref() {
        None => println!(
            "  public-key is not set; run `gpui-auto-update keys generate` and copy the printed SUPublicEDKey"
        ),
        Some(v) => show("public-key", Some(v)),
    }
    let platforms: Vec<&str> = [
        config.macos.as_ref().map(|_| "macos"),
        config.windows.as_ref().map(|_| "windows"),
        config.linux.as_ref().map(|_| "linux"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if platforms.is_empty() {
        println!("  no platform table (macos, windows, linux) is configured");
    } else {
        println!("  platforms: {}", platforms.join(", "));
    }
    println!();
}

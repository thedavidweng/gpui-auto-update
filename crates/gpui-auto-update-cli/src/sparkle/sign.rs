//! Signing the embedded Sparkle framework and the host application in the
//! order Sparkle documents: innermost code first, the app bundle last.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::Args;

use super::bundle;

#[derive(Debug, Args)]
pub struct SignArgs {
    /// The application bundle (`.app`) with Sparkle already embedded.
    #[arg(long)]
    app: PathBuf,
    /// Code-signing identity: `-` for ad hoc, or a Developer ID Application
    /// identity name or SHA-1 hash from the keychain. Never stored in project
    /// configuration.
    #[arg(long, env = "GPUI_AUTO_UPDATE_SIGNING_IDENTITY")]
    identity: String,
    /// Entitlements for the application itself. Without this flag the app's
    /// existing entitlements are preserved.
    #[arg(long)]
    entitlements: Option<PathBuf>,
    /// Keychain to search for the identity (for example a temporary CI
    /// keychain).
    #[arg(long, env = "GPUI_AUTO_UPDATE_KEYCHAIN")]
    keychain: Option<PathBuf>,
}

/// One item to sign, innermost first.
struct Step {
    path: PathBuf,
    preserve_entitlements: bool,
    entitlements: Option<PathBuf>,
    hardened_runtime: bool,
}

fn plan(app: &Path, entitlements: Option<PathBuf>, adhoc: bool) -> Result<Vec<Step>> {
    let framework = bundle::embedded_framework(app);
    if !framework.is_dir() {
        bail!(
            "{} is missing; run `gpui-auto-update sparkle embed` first",
            framework.display()
        );
    }
    let version = bundle::current_version_dir(&framework)?;
    let xpc = version.join("XPCServices");
    let step = |path: PathBuf, preserve_entitlements| Step {
        path,
        preserve_entitlements,
        entitlements: None,
        hardened_runtime: true,
    };
    let mut steps = Vec::new();
    let installer = xpc.join("Installer.xpc");
    if installer.exists() {
        steps.push(step(installer, false));
    }
    let downloader = xpc.join("Downloader.xpc");
    if downloader.exists() {
        // The downloader's own entitlements (network client, sandbox) must
        // survive re-signing.
        steps.push(step(downloader, true));
    }
    for helper in ["Autoupdate", "Updater.app"] {
        let path = version.join(helper);
        if !path.exists() {
            bail!("{} is missing from the embedded framework", path.display());
        }
        steps.push(step(path, false));
    }
    steps.push(step(framework, false));
    steps.push(Step {
        path: app.to_path_buf(),
        preserve_entitlements: entitlements.is_none(),
        entitlements,
        // Library validation, which the hardened runtime turns on, refuses
        // to map ad-hoc signed frameworks: they have no Team ID to match.
        hardened_runtime: !adhoc,
    });
    Ok(steps)
}

pub fn run(args: SignArgs) -> Result<ExitCode> {
    if !cfg!(target_os = "macos") {
        bail!("code signing requires macOS and Apple's codesign tool");
    }
    if args.identity.trim().is_empty() {
        bail!("--identity must not be empty; use `-` for ad-hoc signing");
    }
    let adhoc = args.identity == "-";
    let steps = plan(&args.app, args.entitlements.clone(), adhoc)?;
    for step in steps {
        super::codesign::sign(
            &step.path,
            &super::codesign::SignRequest {
                identity: &args.identity,
                keychain: args.keychain.as_deref(),
                entitlements: step.entitlements,
                preserve_entitlements: step.preserve_entitlements,
                hardened_runtime: step.hardened_runtime,
            },
        )?;
        println!("signed {}", step.path.display());
    }
    if adhoc {
        println!(
            "Signed ad hoc for local testing. The app itself is signed without the hardened runtime, because library validation cannot load an ad-hoc signed Sparkle.framework. Distribution needs a Developer ID Application identity and notarization."
        );
    }
    println!(
        "Next: `gpui-auto-update sparkle validate --app {}`.",
        args.app.display()
    );
    Ok(ExitCode::SUCCESS)
}

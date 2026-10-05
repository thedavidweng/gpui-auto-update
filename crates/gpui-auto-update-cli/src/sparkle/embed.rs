//! Copying Sparkle.framework into an application bundle.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Args;

use super::SandboxMode;
use super::bundle;
use super::pins::Pins;

#[derive(Debug, Args)]
pub struct EmbedArgs {
    /// The application bundle (`.app`) to embed Sparkle into.
    #[arg(long)]
    app: std::path::PathBuf,
    /// A Sparkle distribution directory produced by `sparkle fetch`.
    #[arg(long)]
    sparkle: std::path::PathBuf,
    /// Whether the application runs in the App Sandbox. This decides which
    /// of Sparkle's XPC services are kept.
    #[arg(long, value_enum)]
    sandbox: SandboxMode,
}

pub fn run(args: EmbedArgs) -> Result<ExitCode> {
    let info = args.app.join("Contents/Info.plist");
    if !info.is_file() {
        bail!(
            "{} is not an application bundle: {} is missing",
            args.app.display(),
            info.display()
        );
    }
    let source = args.sparkle.join(bundle::FRAMEWORK_NAME);
    let license = args.sparkle.join("LICENSE");
    if !license.is_file() {
        bail!(
            "{} has no LICENSE file; Sparkle's license notice must ship with the framework",
            args.sparkle.display()
        );
    }
    let version = bundle::framework_version(&source)?;
    if Pins::load()?.get(&version).is_none() {
        eprintln!(
            "warning: Sparkle {version} is not one of the versions pinned by this tool (see `sparkle versions`)"
        );
    }

    let frameworks = args.app.join("Contents/Frameworks");
    fs::create_dir_all(&frameworks)
        .with_context(|| format!("cannot create {}", frameworks.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".sparkle-embed-")
        .tempdir_in(&frameworks)?;
    let staged = staging.path().join(bundle::FRAMEWORK_NAME);
    copy_tree(&source, &staged)?;
    prune_xpc_services(&staged, args.sandbox)?;

    let dest = bundle::embedded_framework(&args.app);
    if fs::symlink_metadata(&dest).is_ok() {
        fs::remove_dir_all(&dest).with_context(|| format!("cannot replace {}", dest.display()))?;
    }
    fs::rename(&staged, &dest).with_context(|| format!("cannot create {}", dest.display()))?;
    drop(staging);

    let notice = args.app.join(bundle::LICENSE_NOTICE);
    fs::create_dir_all(notice.parent().expect("notice path has a parent"))?;
    fs::copy(&license, &notice).with_context(|| format!("cannot write {}", notice.display()))?;

    println!(
        "Embedded Sparkle {version} at {} ({} mode).",
        dest.display(),
        args.sandbox
    );
    println!("Sparkle's license notice is at {}.", notice.display());
    match args.sandbox {
        SandboxMode::NonSandboxed => {}
        SandboxMode::Sandboxed => println!(
            "Set SUEnableInstallerLauncherService and SUEnableDownloaderService to true in Info.plist."
        ),
        SandboxMode::SandboxedNetworkClient => println!(
            "Set SUEnableInstallerLauncherService to true in Info.plist; the app's com.apple.security.network.client entitlement replaces the downloader service."
        ),
    }
    println!(
        "The bundle's signature is now invalid. Next: `gpui-auto-update sparkle sign --app {} --identity <identity>`.",
        args.app.display()
    );
    Ok(ExitCode::SUCCESS)
}

fn prune_xpc_services(framework: &Path, mode: SandboxMode) -> Result<()> {
    let version_dir = bundle::current_version_dir(framework)?;
    let xpc = version_dir.join("XPCServices");
    match mode {
        SandboxMode::Sandboxed => {}
        SandboxMode::SandboxedNetworkClient => {
            remove_if_present(&xpc.join("Downloader.xpc"))?;
        }
        SandboxMode::NonSandboxed => {
            remove_if_present(&xpc)?;
            // The top-level alias would dangle and fail strict code-signing
            // validation.
            let alias = framework.join("XPCServices");
            if fs::symlink_metadata(&alias).is_ok() {
                fs::remove_file(&alias)?;
            }
        }
    }
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path).is_ok() {
        fs::remove_dir_all(path).with_context(|| format!("cannot remove {}", path.display()))?;
    }
    Ok(())
}

/// Recursive copy that recreates symlinks instead of following them, which
/// framework bundles depend on.
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    let meta =
        fs::symlink_metadata(src).with_context(|| format!("cannot read {}", src.display()))?;
    let kind = meta.file_type();
    if kind.is_symlink() {
        symlink(&fs::read_link(src)?, dst)
    } else if kind.is_dir() {
        fs::create_dir(dst).with_context(|| format!("cannot create {}", dst.display()))?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_tree(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(src, dst).with_context(|| format!("cannot copy {}", src.display()))?;
        Ok(())
    }
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("cannot create symlink {}", link.display()))
}

#[cfg(not(unix))]
fn symlink(_target: &Path, link: &Path) -> Result<()> {
    bail!(
        "cannot recreate framework symlink {}: embedding Sparkle requires macOS",
        link.display()
    )
}

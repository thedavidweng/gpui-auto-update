//! Windows backend for `gpui-auto-update`.
//!
//! Implements verified update handoff for per-user installer installs and
//! portable installs, followed by relaunch, on top of the shared feed and
//! verification contracts in
//! [`gpui-auto-update-core`](https://docs.rs/gpui-auto-update-core).
//!
//! The crate builds on every target so that workspace-wide commands work
//! everywhere; Win32 code is compiled only for `cfg(windows)`. Unsafe OS
//! interoperability is confined to this crate.
//!
//! # Overview
//!
//! [`WindowsBackend`] is created from a declared [`WindowsUpdateConfig`]:
//! the running version, the trusted Ed25519 key, one signed feed per
//! architecture, and an [`UpdateStrategy`]. Nothing is inferred from
//! artifact file names.
//!
//! - [`UpdateStrategy::inno_setup`] hands a verified Inno Setup installer
//!   a silent, per-user command line ([`InnoSetup`], configurable switches)
//!   that pins `/DIR=` to the running install directory, then quits; the
//!   installer replaces the files and relaunches the application.
//! - [`UpdateStrategy::portable`] swaps a verified single executable into
//!   place by renaming, then restarts into it.
//! - [`UpdateStrategy::installer`] accepts any [`InstallerStrategy`], the
//!   extension point for MSI and other installer families.
//!
//! An artifact never runs before its length and Ed25519 signature verify,
//! and the release version embedded in its PE version resource must match
//! the feed entry ([`confirm_embedded_version`]). Installers start as the
//! current user and never request elevation. Strategy selection, argument
//! construction, install-directory targeting, and handoff ordering are
//! portable Rust; only process creation is Windows-specific.
//!
//! The installer contract, required Inno Setup script settings, and the MSI
//! extension point are documented in `docs/windows-installers.md` in the
//! repository.

mod backend;
mod config;
mod launch;
pub mod pe;
mod portable;
mod strategy;
#[cfg(windows)]
mod sys;
mod version_check;

pub use backend::{WindowsBackend, WindowsHandoff};
pub use config::{DEFAULT_LAUNCH_GRACE, WindowsUpdateConfig};
pub use launch::{InstallerLauncher, LaunchedInstaller, SystemLauncher};
pub use strategy::{
    InnoSetup, InstallTarget, InstallerCommand, InstallerStrategy, InvalidSwitch,
    PortableExecutable, UpdateStrategy,
};
pub use version_check::{DEFAULT_VERSION_KEY, confirm_embedded_version};

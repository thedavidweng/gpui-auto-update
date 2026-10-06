//! Linux backend for `gpui-auto-update`.
//!
//! Implements updates for managed user-local installs: verified staging, a
//! helper that swaps the install after the application quits, relaunch health
//! confirmation, and rollback. Shared feed and verification contracts come
//! from [`gpui-auto-update-core`](https://docs.rs/gpui-auto-update-core).
//!
//! The crate builds on every target so that workspace-wide commands work
//! everywhere; Linux-only code is compiled only for `target_os = "linux"`.
//! Unsafe OS interoperability is confined to this crate.
//!
//! # Managed-install detection
//!
//! Only explicitly marked, user-owned tarball installs update themselves.
//! [`detect`] decides this from injected [`DetectionInputs`] (portable and
//! testable on any Unix host); on Linux, `detect_current` gathers those
//! inputs from the running process. Everything else reports an externally
//! managed, unsupported, or temporarily unavailable
//! [`Capability`](gpui_auto_update_core::Capability) together with a
//! [`DetectionReason`], so the application can explain why updates are
//! unavailable instead of hiding the updater. The marker contract is
//! documented in `docs/linux-managed-install.md` in the repository.
//!
//! # Staging a release
//!
//! [`ReleaseStager::stage`] takes a verified
//! [`StagedArtifact`](gpui_auto_update_core::download::StagedArtifact) (the
//! only way to obtain one is a download whose signature verified) and
//! extracts the release tarball into a private directory next to the managed
//! install, never over it. Archive entries are validated strictly (no
//! traversal, absolute paths, links, special files, duplicates, or other
//! top-level roots, and bounded sizes and entry counts), the top-level
//! directory must name the expected version and architecture, and the
//! extracted layout must pass [`validate_layout`] before the
//! [`StagedRelease`] is returned. The archive contract is documented in the
//! same document.
//!
//! # Installing: helper, health confirmation, rollback
//!
//! The staged release is installed by the update helper, a mode of the
//! application's own executable: call [`run_helper_if_requested`] first in
//! `main`. [`HelperCommand::hand_off`] starts it and waits until it has
//! accepted both the current and the staged layout; after
//! [`PendingHelper::commit`] the application quits normally and the helper
//! swaps the prefix, relaunches, and waits for [`confirm_startup`] from the
//! new version, rolling back if it exits first. Failures after the quit are
//! recorded and returned once by [`take_diagnostic`] on the next start.
//! [`LinuxUpdater`] combines staging and the handoff for one managed
//! install. The protocol is recorded in `docs/adr/0002-linux-update-helper.md`.

// The crate docs describe the Unix-only API; on other targets those items do
// not exist, and docs.rs documents this crate for Linux.
#![cfg_attr(not(unix), allow(rustdoc::broken_intra_doc_links))]

#[cfg(target_os = "linux")]
mod current;
#[cfg(unix)]
mod detect;
#[cfg(unix)]
mod diagnostic;
#[cfg(unix)]
mod extract;
#[cfg(unix)]
mod handoff;
#[cfg(unix)]
mod health;
#[cfg(unix)]
mod helper;
#[cfg(any(target_os = "linux", test))]
mod proc_status;
#[cfg(unix)]
mod siblings;
#[cfg(unix)]
mod updater;

#[cfg(target_os = "linux")]
pub use current::detect_current;

#[cfg(unix)]
pub use detect::{
    Detection, DetectionInputs, DetectionReason, MARKER_FILE_NAME, ManagedInstall, detect,
    marker_contents,
};
#[cfg(unix)]
pub use diagnostic::{HelperDiagnostic, HelperOutcome, take_diagnostic};
#[cfg(unix)]
pub use extract::{
    ArchiveLimits, LayoutError, ReleaseStager, StageError, StagedRelease, validate_layout,
};
#[cfg(unix)]
pub use handoff::{HandoffError, HandoffRequest, HelperCommand, PendingHelper};
#[cfg(unix)]
pub use health::{
    HEALTH_FILE_ENV, HealthError, StartupConfirmation, confirm_startup, confirm_startup_for,
};
#[cfg(unix)]
pub use helper::run_helper_if_requested;
#[cfg(unix)]
pub use updater::LinuxUpdater;

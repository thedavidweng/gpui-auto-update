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

#[cfg(target_os = "linux")]
mod current;
#[cfg(unix)]
mod detect;
#[cfg(any(target_os = "linux", test))]
mod proc_status;

#[cfg(target_os = "linux")]
pub use current::detect_current;

#[cfg(unix)]
pub use detect::{
    Detection, DetectionInputs, DetectionReason, MARKER_FILE_NAME, ManagedInstall, detect,
    marker_contents,
};

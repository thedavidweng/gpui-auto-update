//! macOS backend for `gpui-auto-update`.
//!
//! On macOS, update mechanics (appcast checks, EdDSA verification,
//! installation, and relaunch) are delegated to Sparkle 2. This crate adapts
//! Sparkle's lifecycle to the backend contract defined by
//! [`gpui-auto-update-core`](https://docs.rs/gpui-auto-update-core).
//!
//! The crate builds on every target so that workspace-wide commands work
//! everywhere; platform code is compiled only for `target_os = "macos"`.
//! Unsafe Objective-C interoperability is confined to this crate.
//!
//! The backend is not yet implemented.

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
//! The backend is not yet implemented.

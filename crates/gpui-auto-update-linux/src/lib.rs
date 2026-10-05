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
//! The backend is not yet implemented.

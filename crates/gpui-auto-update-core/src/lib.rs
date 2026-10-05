//! Framework-independent core of `gpui-auto-update`.
//!
//! This crate owns the contracts every platform shares: the public update
//! state model, update capability and ownership, the signed feed format,
//! artifact verification, automatic-update policy, structured errors, and the
//! orchestration used by the Windows and Linux backends.
//!
//! It deliberately has no GPUI dependency, so it can be reused by other UI
//! frameworks and tested without a window system. Most applications should
//! depend on the [`gpui-auto-update`](https://docs.rs/gpui-auto-update)
//! facade instead of using this crate directly.
//!
//! The public API is not yet implemented; this release only reserves the
//! crate in the workspace layout described in ADR 0001.

#![forbid(unsafe_code)]

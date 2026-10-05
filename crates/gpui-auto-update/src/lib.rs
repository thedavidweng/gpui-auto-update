//! Sparkle-quality automatic updates for GPUI.
//!
//! `gpui-auto-update` is the crate GPUI applications depend on. It exposes
//! the updater as an observable GPUI entity with standard actions, runs
//! network and filesystem work off the foreground executor, and selects the
//! platform backend for the current target: Sparkle 2 on macOS, and signed
//! native updates on Windows and Linux.
//!
//! The GPUI facade is not yet implemented. The framework-independent
//! contracts are re-exported as [`core`].

#![forbid(unsafe_code)]

pub use gpui_auto_update_core as core;

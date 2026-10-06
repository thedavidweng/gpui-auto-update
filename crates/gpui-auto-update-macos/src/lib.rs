//! macOS backend for `gpui-auto-update`.
//!
//! On macOS, update mechanics (appcast checks, EdDSA and code-signing
//! verification, deltas, installation, authorization, and relaunch) are
//! delegated to Sparkle 2 through its `SPUUpdater`-generation API. This
//! crate translates Sparkle's lifecycle into the contracts of
//! [`gpui-auto-update-core`](https://docs.rs/gpui-auto-update-core):
//!
//! - [`SparkleBackend`] is the [`CheckSource`](gpui_auto_update_core::CheckSource):
//!   a manual check uses Sparkle's standard update UI, a background check
//!   uses Sparkle's background check, and both report what Sparkle found.
//! - [`SparkleBackend::preferences`] is the
//!   [`PreferenceStore`](gpui_auto_update_core::PreferenceStore), owned by
//!   Sparkle, so Sparkle stays the single source of truth for the
//!   automatic-check preference and its schedule.
//! - [`SparkleBackend::attach`] mirrors downloads, installs, relaunches,
//!   and the user's choices in Sparkle's windows into the update state.
//!
//! The Sparkle calls themselves go through the [`SparkleEngine`] trait. The
//! real engine, which links `Sparkle.framework` through the
//! [`sparkle-updater`](https://crates.io/crates/sparkle-updater) crate, is
//! behind the opt-in `sparkle` feature and is started with
//! `SparkleBackend::start`; see `docs/adr/0002-sparkle-binding.md` in the
//! repository for why. Without the feature this crate is pure Rust, builds
//! on every target, and its lifecycle can be exercised with a scripted
//! engine.
//!
//! Most applications use this crate through the `gpui-auto-update` facade.

mod backend;
mod engine;
mod event;
mod hub;
mod mapping;
#[cfg(all(target_os = "macos", feature = "sparkle"))]
mod native;

pub use backend::{DEFAULT_CHECK_TIMEOUT, SparkleBackend, SparklePreferences};
pub use engine::SparkleEngine;
pub use event::{
    NoUpdateReason, SparkleError, SparkleEvent, SparkleUpdate, UpdateStage, UserChoice,
};
pub use hub::SparkleEvents;

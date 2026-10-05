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
#![forbid(unsafe_code)]

mod capability;
mod check_source;
mod coordinator;
mod error;
mod policy;
mod preferences;
mod state;

pub use capability::Capability;
pub use check_source::{CheckKind, CheckOutcome, CheckRequest, CheckSource};
pub use coordinator::{Subscription, UpdateCoordinator};
pub use error::{ErrorKind, UpdateError};
pub use policy::{AutomaticChecks, CheckPolicy, Clock, SystemClock};
pub use preferences::{
    FilePreferenceStore, MemoryPreferenceStore, PreferenceOwner, PreferenceStore, UpdatePreferences,
};
pub use state::{
    AvailableUpdate, Channel, DownloadProgress, ReleaseNotes, ReleaseNotesFormat, UpdateEvent,
    UpdateState,
};

pub mod check;
pub mod download;
pub mod feed;
pub mod fetch;
pub mod trust;
pub mod version;

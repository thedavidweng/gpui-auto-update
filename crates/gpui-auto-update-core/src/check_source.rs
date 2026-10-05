//! The contract between the check coordinator and whatever resolves updates.

use std::sync::Arc;

use crate::error::UpdateError;
use crate::state::AvailableUpdate;

/// How a check was initiated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CheckKind {
    /// Initiated by the user; always produces visible feedback.
    Manual,
    /// Initiated automatically; silent unless an update is found.
    Background,
}

/// Parameters of one check, passed to [`CheckSource::check`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CheckRequest {
    /// How the check that the source is asked to run was initiated.
    ///
    /// When a manual check attaches to a running background check, the
    /// source is not called again, so this stays [`CheckKind::Background`].
    pub kind: CheckKind,
}

impl CheckRequest {
    /// Creates a request for a check of `kind`.
    pub fn new(kind: CheckKind) -> Self {
        Self { kind }
    }
}

/// The result of a successful check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    /// No release newer than the running version applies.
    UpToDate,
    /// A newer release applies.
    UpdateAvailable(AvailableUpdate),
}

/// Resolves the newest applicable update; implemented by the signed feed
/// checker and by platform backends such as Sparkle.
///
/// [`CheckSource::check`] may block (network I/O, waiting on a native
/// updater). The coordinator calls it on the thread that called
/// [`crate::UpdateCoordinator::check`], never while holding internal locks,
/// and at most once at a time per coordinator.
pub trait CheckSource: Send + Sync + 'static {
    /// Runs one check.
    fn check(&self, request: &CheckRequest) -> Result<CheckOutcome, UpdateError>;
}

impl<T: CheckSource + ?Sized> CheckSource for Box<T> {
    fn check(&self, request: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        (**self).check(request)
    }
}

impl<T: CheckSource + ?Sized> CheckSource for Arc<T> {
    fn check(&self, request: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        (**self).check(request)
    }
}

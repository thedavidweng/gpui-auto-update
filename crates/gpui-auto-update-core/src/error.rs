//! Structured update errors.
//!
//! Every [`UpdateError`] carries an [`ErrorKind`] that code can match on and a
//! human-readable message that is safe to show to users. Diagnostic detail
//! (internal paths, URLs, underlying I/O errors) is kept separately and only
//! reaches logs through [`UpdateError::diagnostic`], [`fmt::Debug`] and
//! [`std::error::Error::source`]; it never appears in [`fmt::Display`].

use std::borrow::Cow;
use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;

/// The class of an update failure.
///
/// The set grows as backends gain features, so matches must include a
/// wildcard arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The updater was configured incorrectly (missing feed URL, invalid
    /// public key, inconsistent bundle metadata, and so on).
    Configuration,
    /// This installation cannot be updated by this library.
    UnsupportedInstallation,
    /// This installation is updated by something else, such as a package
    /// manager.
    ExternallyManaged,
    /// The installation could normally update itself, but cannot right now.
    TemporarilyUnavailable,
    /// The requested operation overlaps an update operation that is already
    /// running.
    OperationInProgress,
    /// The requested operation does not apply to the current update state.
    InvalidState,
    /// The feed could not be fetched.
    FeedRetrieval,
    /// The feed was fetched but could not be parsed.
    FeedParsing,
    /// No applicable version could be resolved from the feed, or a version
    /// string was malformed.
    VersionResolution,
    /// A signature was missing or did not verify against the trusted key.
    Signature,
    /// The artifact could not be downloaded.
    Download,
    /// The downloaded artifact's length did not match the feed.
    LengthMismatch,
    /// The downloaded archive was malformed or unsafe to extract.
    ArchiveValidation,
    /// The verified artifact could not be staged next to the install.
    Staging,
    /// The update helper or installer could not be launched.
    HelperLaunch,
    /// The host application did not complete its prepare-to-install hook or
    /// quit as required.
    QuitCoordination,
    /// The installed files could not be replaced with the staged release.
    Replacement,
    /// The application could not be relaunched after the update.
    Relaunch,
    /// The relaunched application did not confirm that it started correctly.
    HealthConfirmation,
    /// Rolling back to the previous version failed.
    Rollback,
    /// An unexpected internal failure, such as a check source that panicked.
    Internal,
}

impl ErrorKind {
    /// A generic, user-presentable description of this class of failure.
    ///
    /// It never contains paths, URLs, or other installation-specific data.
    pub fn default_message(self) -> &'static str {
        match self {
            Self::Configuration => "The updater is not configured correctly.",
            Self::UnsupportedInstallation => {
                "This installation does not support automatic updates."
            }
            Self::ExternallyManaged => {
                "This installation is updated by another tool, such as a package manager."
            }
            Self::TemporarilyUnavailable => "Updates are temporarily unavailable. Try again later.",
            Self::OperationInProgress => "Another update operation is already in progress.",
            Self::InvalidState => "That update action is not available right now.",
            Self::FeedRetrieval => "Could not reach the update server.",
            Self::FeedParsing => "The update information could not be read.",
            Self::VersionResolution => "The update information contains an invalid version.",
            Self::Signature => "The update could not be verified and was rejected.",
            Self::Download => "The update could not be downloaded.",
            Self::LengthMismatch => "The downloaded update is incomplete or has the wrong size.",
            Self::ArchiveValidation => "The downloaded update is damaged or invalid.",
            Self::Staging => "The update could not be prepared for installation.",
            Self::HelperLaunch => "The update installer could not be started.",
            Self::QuitCoordination => "The application could not be closed to install the update.",
            Self::Replacement => "The update could not be installed.",
            Self::Relaunch => "The application could not be restarted after the update.",
            Self::HealthConfirmation => "The updated application did not start correctly.",
            Self::Rollback => "The previous version could not be restored.",
            Self::Internal => "An unexpected error occurred while updating.",
        }
    }
}

/// A structured update failure.
///
/// `Display` renders only the user-facing message. Diagnostic detail and the
/// underlying cause are available to logging through [`Self::diagnostic`],
/// [`fmt::Debug`], and [`std::error::Error::source`].
///
/// Errors are cheap to clone so the same failure can be delivered to every
/// caller attached to a coalesced check and stored in [`crate::UpdateState`].
#[derive(Clone)]
pub struct UpdateError {
    kind: ErrorKind,
    message: Cow<'static, str>,
    diagnostic: Option<String>,
    source: Option<Arc<dyn StdError + Send + Sync + 'static>>,
}

impl UpdateError {
    /// Creates an error of `kind` with that kind's default user message.
    pub fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            message: Cow::Borrowed(kind.default_message()),
            diagnostic: None,
            source: None,
        }
    }

    /// Replaces the user-facing message.
    ///
    /// The message is shown to end users verbatim. It must not contain
    /// filesystem paths, URLs with credentials, tokens, or other secrets; put
    /// those in [`Self::with_diagnostic`] instead.
    pub fn with_message(mut self, message: impl Into<Cow<'static, str>>) -> Self {
        self.message = message.into();
        self
    }

    /// Attaches diagnostic detail intended for logs, never for end users.
    pub fn with_diagnostic(mut self, diagnostic: impl Into<String>) -> Self {
        self.diagnostic = Some(diagnostic.into());
        self
    }

    /// Attaches the underlying cause, exposed through
    /// [`std::error::Error::source`].
    pub fn with_source(mut self, source: impl StdError + Send + Sync + 'static) -> Self {
        self.source = Some(Arc::new(source));
        self
    }

    /// The class of this failure.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The message that is safe to present to end users.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Diagnostic detail for logs, if any. Not suitable for end users.
    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }
}

impl From<ErrorKind> for UpdateError {
    fn from(kind: ErrorKind) -> Self {
        Self::new(kind)
    }
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl fmt::Debug for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpdateError")
            .field("kind", &self.kind)
            .field("message", &self.message)
            .field("diagnostic", &self.diagnostic)
            .field("source", &self.source)
            .finish()
    }
}

impl StdError for UpdateError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn StdError + 'static))
    }
}

/// Two errors are equal when their kind and user message match; diagnostic
/// detail and sources are ignored.
impl PartialEq for UpdateError {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.message == other.message
    }
}

impl Eq for UpdateError {}

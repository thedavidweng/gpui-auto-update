//! Update capability (ownership) of the running installation.

use crate::error::{ErrorKind, UpdateError};

/// Whether this running installation may update itself.
///
/// The updater never modifies an installation it cannot prove it owns, so
/// anything other than [`Capability::SelfManaged`] disables checks and
/// installs and lets the UI explain why.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Capability {
    /// This installation is owned by this updater and may update itself.
    SelfManaged,
    /// Another tool, such as Homebrew or a system package manager, updates
    /// this installation.
    ExternallyManaged {
        /// A user-presentable name of the managing tool, when known
        /// (for example `"Homebrew"`).
        manager: Option<String>,
    },
    /// This installation cannot be updated by this library (for example an
    /// unmarked Linux install, or a platform without a backend).
    Unsupported,
    /// The installation is normally self-managed but cannot update right now
    /// (for example the install location is read-only or on a disk image).
    TemporarilyUnavailable,
}

impl Capability {
    /// Whether checks and installs may run for this installation.
    pub fn can_self_update(&self) -> bool {
        matches!(self, Self::SelfManaged)
    }

    /// The error a check or install reports when this capability forbids it,
    /// or `None` for [`Capability::SelfManaged`].
    pub fn denial(&self) -> Option<UpdateError> {
        match self {
            Self::SelfManaged => None,
            Self::ExternallyManaged {
                manager: Some(manager),
            } => Some(
                UpdateError::new(ErrorKind::ExternallyManaged)
                    .with_message(format!("This installation is updated by {manager}.")),
            ),
            Self::ExternallyManaged { manager: None } => {
                Some(UpdateError::new(ErrorKind::ExternallyManaged))
            }
            Self::Unsupported => Some(UpdateError::new(ErrorKind::UnsupportedInstallation)),
            Self::TemporarilyUnavailable => {
                Some(UpdateError::new(ErrorKind::TemporarilyUnavailable))
            }
        }
    }
}

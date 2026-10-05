//! Release versions used by native (Windows and Linux) feeds.
//!
//! A native feed's `sparkle:version` must be a strict
//! [Semantic Versioning 2.0.0](https://semver.org) string. That one value is
//! both the machine-comparable version and the only feed-derived text that may
//! ever name a file or directory, so it is validated once here.

use std::cmp::Ordering;
use std::fmt;

/// Longest accepted version string, in bytes.
pub const MAX_VERSION_LEN: usize = 64;

/// A validated release version.
///
/// Ordering for update decisions follows semver *precedence*: build metadata
/// (`+...`) is ignored and pre-releases sort before their release.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReleaseVersion {
    parsed: semver::Version,
    text: String,
}

/// Why a string is not a valid [`ReleaseVersion`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VersionError {
    /// The string is longer than [`MAX_VERSION_LEN`].
    #[error("version is {0} bytes long; at most {MAX_VERSION_LEN} are allowed")]
    TooLong(usize),
    /// The string is not strict semver (`MAJOR.MINOR.PATCH[-pre][+build]`).
    #[error("version {0:?} is not a strict semantic version")]
    NotSemver(String),
}

impl ReleaseVersion {
    /// Parses a strict semantic version with no surrounding whitespace and no
    /// `v` prefix.
    pub fn parse(text: &str) -> Result<Self, VersionError> {
        if text.len() > MAX_VERSION_LEN {
            return Err(VersionError::TooLong(text.len()));
        }
        let parsed =
            semver::Version::parse(text).map_err(|_| VersionError::NotSemver(text.to_owned()))?;
        // Semver's grammar only admits ASCII alphanumerics, '-', '.', and '+',
        // and forbids empty identifiers, so "." and ".." can never appear as a
        // whole value. Re-checking the alphabet keeps the path-safety claim of
        // `path_component` independent of the parser's leniency.
        let safe = text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'+'));
        if !safe || parsed.to_string() != text {
            return Err(VersionError::NotSemver(text.to_owned()));
        }
        Ok(Self {
            parsed,
            text: text.to_owned(),
        })
    }

    /// Returns `true` when `self` has higher semver precedence than `other`.
    pub fn is_newer_than(&self, other: &Self) -> bool {
        self.cmp_precedence(other) == Ordering::Greater
    }

    /// Compares by semver precedence (build metadata ignored).
    pub fn cmp_precedence(&self, other: &Self) -> Ordering {
        self.parsed.cmp_precedence(&other.parsed)
    }

    /// The version text, guaranteed to be a single normal path component
    /// (no separators, no `.`/`..`, no drive prefix), suitable for naming a
    /// staging directory or download file.
    pub fn path_component(&self) -> &str {
        &self.text
    }

    /// The version text exactly as published.
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for ReleaseVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::str::FromStr for ReleaseVersion {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

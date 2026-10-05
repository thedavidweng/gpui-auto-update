//! A single update check against a native feed: fetch it within bounds,
//! validate every entry, and select the newest applicable release.
//!
//! A check never verifies artifact bytes (it does not download them), but it
//! only ever returns releases whose entries carry a well-formed Ed25519
//! signature and declared length, which download code must then verify with
//! [`TrustedKey::verify_artifact`](crate::trust::TrustedKey::verify_artifact).

use std::sync::{Mutex, MutexGuard, PoisonError};

use url::Url;

use crate::check_source::{CheckOutcome, CheckRequest, CheckSource};
use crate::error::{ErrorKind, UpdateError};
use crate::feed::{
    Channel as FeedChannel, Feed, FeedError, FeedLimits, ItemError, SelectedUpdate, Selection,
    UpdateTarget,
};
use crate::fetch::{FetchError, HttpClient};
use crate::state::{AvailableUpdate, Channel, ReleaseNotes, ReleaseNotesFormat};
use crate::version::ReleaseVersion;

/// Why a check failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckError {
    /// The feed could not be downloaded.
    #[error("could not fetch the update feed: {0}")]
    Fetch(#[from] FetchError),
    /// The feed was downloaded but rejected.
    #[error("the update feed is invalid: {0}")]
    Feed(#[from] FeedError),
}

/// Checks one architecture-specific feed for a newer release.
#[derive(Debug, Clone)]
pub struct UpdateChecker {
    feed_url: Url,
    target: UpdateTarget,
    limits: FeedLimits,
    client: HttpClient,
}

impl UpdateChecker {
    /// A checker for the feed at `feed_url` with default [`FeedLimits`].
    pub fn new(feed_url: Url, target: UpdateTarget, client: HttpClient) -> Self {
        Self {
            feed_url,
            target,
            limits: FeedLimits::default(),
            client,
        }
    }

    /// Replaces the feed limits.
    pub fn with_limits(mut self, limits: FeedLimits) -> Self {
        self.limits = limits;
        self
    }

    /// The configured feed URL.
    pub fn feed_url(&self) -> &Url {
        &self.feed_url
    }

    /// The installation the feed is evaluated for.
    pub fn target(&self) -> &UpdateTarget {
        &self.target
    }

    /// Fetches and evaluates the feed for an installation at `current`.
    /// Blocks the calling thread.
    pub fn check(&self, current: &ReleaseVersion) -> Result<Selection, CheckError> {
        let bytes = self
            .client
            .get_bytes(&self.feed_url, self.limits.max_feed_bytes)?;
        let feed = Feed::parse(&bytes, &self.limits)?;
        Ok(feed.select(&self.target, current)?)
    }
}

impl From<CheckError> for UpdateError {
    fn from(error: CheckError) -> Self {
        let kind = match &error {
            CheckError::Fetch(_) => ErrorKind::FeedRetrieval,
            CheckError::Feed(FeedError::NoEntriesForTarget { .. }) => ErrorKind::Configuration,
            CheckError::Feed(FeedError::InvalidItem { reason, .. }) => match reason {
                ItemError::Version(_) => ErrorKind::VersionResolution,
                ItemError::Signature(_) | ItemError::Missing("sparkle:edSignature") => {
                    ErrorKind::Signature
                }
                _ => ErrorKind::FeedParsing,
            },
            CheckError::Feed(_) => ErrorKind::FeedParsing,
        };
        UpdateError::new(kind)
            .with_diagnostic(error.to_string())
            .with_source(error)
    }
}

impl From<&FeedChannel> for Channel {
    fn from(channel: &FeedChannel) -> Self {
        Channel::new(channel.as_str())
    }
}

impl From<&SelectedUpdate> for AvailableUpdate {
    /// The user-facing metadata of a selected release.
    ///
    /// [`AvailableUpdate::version`] is the display version when the feed
    /// has one, with `sparkle:version` as [`AvailableUpdate::build`];
    /// otherwise it is `sparkle:version` itself. Inline release notes
    /// (`<description>`, HTML) take precedence over a release notes link.
    fn from(selected: &SelectedUpdate) -> Self {
        let item = &selected.item;
        let mut update = match &item.display_version {
            Some(display) => {
                AvailableUpdate::new(display.clone()).with_build(item.version.as_str())
            }
            None => AvailableUpdate::new(item.version.as_str()),
        };
        if let Some(channel) = &item.channel {
            update = update.with_channel(channel.into());
        }
        if let Some(content) = &item.description {
            update = update.with_release_notes(ReleaseNotes::Inline {
                content: content.clone(),
                format: ReleaseNotesFormat::Html,
            });
        } else if let Some(link) = &item.release_notes_url {
            update = update.with_release_notes(ReleaseNotes::Link(link.to_string()));
        }
        if let Some(published) = &item.published {
            update = update.with_published(published.clone());
        }
        update.with_critical(selected.is_critical)
    }
}

/// Runs [`UpdateChecker`] checks for an [`crate::UpdateCoordinator`].
///
/// Besides reporting the [`AvailableUpdate`], it remembers the full
/// [`SelectedUpdate`] (artifact URL, length, and signature) so the artifact
/// can then be downloaded with
/// [`ArtifactDownloader`](crate::download::ArtifactDownloader). Share it
/// with the coordinator through an [`Arc`](std::sync::Arc) to read
/// [`Self::selected`] later.
#[derive(Debug)]
pub struct FeedCheckSource {
    checker: UpdateChecker,
    current: ReleaseVersion,
    selected: Mutex<Option<SelectedUpdate>>,
}

impl FeedCheckSource {
    /// A source that checks with `checker` for an installation at `current`.
    pub fn new(checker: UpdateChecker, current: ReleaseVersion) -> Self {
        Self {
            checker,
            current,
            selected: Mutex::new(None),
        }
    }

    /// The release found by the last successful check, or `None` if that
    /// check found no update. A failed check keeps the previous value, just
    /// as a failed background check keeps the previous update state.
    pub fn selected(&self) -> Option<SelectedUpdate> {
        self.lock().clone()
    }

    fn lock(&self) -> MutexGuard<'_, Option<SelectedUpdate>> {
        self.selected.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl CheckSource for FeedCheckSource {
    fn check(&self, _request: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        match self.checker.check(&self.current)? {
            Selection::UpToDate => {
                *self.lock() = None;
                Ok(CheckOutcome::UpToDate)
            }
            Selection::UpdateAvailable(selected) => {
                let update = AvailableUpdate::from(&*selected);
                *self.lock() = Some(*selected);
                Ok(CheckOutcome::UpdateAvailable(update))
            }
        }
    }
}

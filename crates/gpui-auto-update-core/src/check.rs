//! A single update check against a native feed: fetch it within bounds,
//! validate every entry, and select the newest applicable release.
//!
//! A check never verifies artifact bytes (it does not download them), but it
//! only ever returns releases whose entries carry a well-formed Ed25519
//! signature and declared length, which download code must then verify with
//! [`TrustedKey::verify_artifact`](crate::trust::TrustedKey::verify_artifact).

use url::Url;

use crate::feed::{Feed, FeedError, FeedLimits, Selection, UpdateTarget};
use crate::fetch::{FetchError, HttpClient};
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

//! Configuration for the project's signed native feeds, with the platform's
//! default backend.

use std::sync::Arc;

use gpui_auto_update_core::check::{FeedCheckSource, UpdateChecker};
use gpui_auto_update_core::download::ArtifactDownloader;
use gpui_auto_update_core::feed::{Arch, Os, UpdateTarget};
use gpui_auto_update_core::fetch::{FetchPolicy, HttpClient};
use gpui_auto_update_core::trust::TrustedKey;
use gpui_auto_update_core::version::ReleaseVersion;
use gpui_auto_update_core::{Capability, ErrorKind, UpdateError};

use crate::backend::UpdateBackend;
use crate::config::{NoUpdates, UpdaterConfig};

/// A signed, architecture-specific native feed (see `docs/feed-format.md`)
/// and the trust needed to install from it.
#[derive(Clone, Debug)]
pub struct NativeFeed {
    feed_url: url::Url,
    public_key: TrustedKey,
    current_version: ReleaseVersion,
    executable_name: Option<String>,
    fetch_policy: FetchPolicy,
}

impl NativeFeed {
    /// The feed at `feed_url` (for this platform and architecture), whose
    /// artifacts are signed with the key `public_key` verifies, for an
    /// application running `current_version`.
    ///
    /// Fails with [`ErrorKind::Configuration`] when `feed_url` is not a URL.
    pub fn new(
        feed_url: &str,
        public_key: TrustedKey,
        current_version: ReleaseVersion,
    ) -> Result<Self, UpdateError> {
        let feed_url = feed_url.parse().map_err(|error| {
            UpdateError::new(ErrorKind::Configuration)
                .with_diagnostic(format!("invalid feed URL {feed_url:?}: {error}"))
        })?;
        Ok(Self {
            feed_url,
            public_key,
            current_version,
            executable_name: None,
            fetch_policy: FetchPolicy::default(),
        })
    }

    /// Sets the application name used by the Linux managed-install layout
    /// (`<prefix>/bin/<app>`). Defaults to the running executable's file
    /// name.
    pub fn with_executable_name(mut self, name: impl Into<String>) -> Self {
        self.executable_name = Some(name.into());
        self
    }

    /// Replaces the HTTP rules (timeouts, redirects, and whether plain
    /// `http` is allowed, which only loopback test servers should use).
    pub fn with_fetch_policy(mut self, policy: FetchPolicy) -> Self {
        self.fetch_policy = policy;
        self
    }
}

impl UpdaterConfig {
    /// A configuration that checks `feed` and installs with this platform's
    /// default backend:
    ///
    /// | Platform | Backend |
    /// | --- | --- |
    /// | Linux | `linux::LinuxBackend`: managed user-local installs, helper swap, health confirmation, rollback |
    /// | Others | [`UnsupportedBackend`](crate::UnsupportedBackend) until their native backend is configured |
    ///
    /// On an operating system or architecture without native feeds the
    /// installation reports [`Capability::Unsupported`]. Creating the
    /// configuration does no I/O; the backend inspects the installation on
    /// the background executor when the updater starts.
    pub fn native_feed(app_id: impl Into<String>, feed: NativeFeed) -> Self {
        let (Some(os), Some(arch)) = (Os::current(), Arch::current()) else {
            return Self::new(app_id, NoUpdates).with_capability(Capability::Unsupported);
        };
        let client = HttpClient::new(feed.fetch_policy);
        let checker =
            UpdateChecker::new(feed.feed_url, UpdateTarget::new(os, arch), client.clone());
        let source = Arc::new(FeedCheckSource::new(checker, feed.current_version));
        let downloader = ArtifactDownloader::new(client, feed.public_key);
        let backend = default_backend(source.clone(), downloader, feed.executable_name);
        let mut config = Self::new(app_id, source);
        config.backend = backend;
        config
    }
}

#[cfg(target_os = "linux")]
fn default_backend(
    source: Arc<FeedCheckSource>,
    downloader: ArtifactDownloader,
    executable_name: Option<String>,
) -> Arc<dyn UpdateBackend> {
    let backend = crate::linux::LinuxBackend::new(source, downloader);
    Arc::new(match executable_name {
        Some(name) => backend.with_executable_name(name),
        None => backend,
    })
}

#[cfg(not(target_os = "linux"))]
fn default_backend(
    _source: Arc<FeedCheckSource>,
    _downloader: ArtifactDownloader,
    _executable_name: Option<String>,
) -> Arc<dyn UpdateBackend> {
    Arc::new(crate::backend::UnsupportedBackend)
}

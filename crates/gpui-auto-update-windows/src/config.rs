//! Declared configuration of the Windows backend.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_auto_update_core::feed::{Arch, Channel, FeedLimits, SystemVersion};
use gpui_auto_update_core::fetch::FetchPolicy;
use gpui_auto_update_core::trust::TrustedKey;
use gpui_auto_update_core::version::ReleaseVersion;
use url::Url;

use crate::launch::{InstallerLauncher, SystemLauncher};
use crate::strategy::{InstallTarget, UpdateStrategy};

/// How long a started installer is watched for an immediate failure before
/// the application quits.
pub const DEFAULT_LAUNCH_GRACE: Duration = Duration::from_millis(750);

/// Everything the Windows backend needs, declared by the application.
///
/// Required: the running version, the trusted release key, the update
/// strategy, and one feed URL per supported architecture. The architecture
/// and the strategy are never inferred from artifact names.
#[derive(Clone)]
pub struct WindowsUpdateConfig {
    pub(crate) current_version: ReleaseVersion,
    pub(crate) key: TrustedKey,
    pub(crate) strategy: UpdateStrategy,
    pub(crate) feeds: Vec<(Arch, Url)>,
    pub(crate) arch: Option<Arch>,
    pub(crate) channels: Vec<Channel>,
    pub(crate) system_version: Option<SystemVersion>,
    pub(crate) fetch_policy: FetchPolicy,
    pub(crate) limits: FeedLimits,
    pub(crate) target: Option<InstallTarget>,
    pub(crate) staging_root: Option<PathBuf>,
    pub(crate) launcher: Arc<dyn InstallerLauncher>,
    pub(crate) launch_grace: Duration,
}

impl WindowsUpdateConfig {
    /// A configuration for an installation at `current_version` that trusts
    /// artifacts signed by `key` and applies them with `strategy`.
    ///
    /// The defaults are: the architecture this binary was compiled for, the
    /// default channel only, default fetch timeouts and feed limits, the
    /// running executable's directory as the install target, and
    /// [`SystemLauncher`].
    pub fn new(current_version: ReleaseVersion, key: TrustedKey, strategy: UpdateStrategy) -> Self {
        Self {
            current_version,
            key,
            strategy,
            feeds: Vec::new(),
            arch: None,
            channels: Vec::new(),
            system_version: None,
            fetch_policy: FetchPolicy::default(),
            limits: FeedLimits::default(),
            target: None,
            staging_root: None,
            launcher: Arc::new(SystemLauncher),
            launch_grace: DEFAULT_LAUNCH_GRACE,
        }
    }

    /// Declares the signed feed for `arch`, replacing an earlier one.
    pub fn with_feed(mut self, arch: Arch, url: Url) -> Self {
        self.feeds.retain(|(a, _)| *a != arch);
        self.feeds.push((arch, url));
        self
    }

    /// Selects the feed of `arch` instead of the compile-time architecture,
    /// for example to move an emulated x86_64 build on ARM64 hardware to the
    /// native ARM64 release.
    pub fn with_arch(mut self, arch: Arch) -> Self {
        self.arch = Some(arch);
        self
    }

    /// Also accepts releases on `channel`.
    pub fn with_channel(mut self, channel: Channel) -> Self {
        self.channels.push(channel);
        self
    }

    /// Skips releases whose `sparkle:minimumSystemVersion` is above
    /// `version` (for example `10.0.17763`).
    pub fn with_system_version(mut self, version: SystemVersion) -> Self {
        self.system_version = Some(version);
        self
    }

    /// Replaces the timeouts and redirect policy of feed and artifact
    /// downloads.
    pub fn with_fetch_policy(mut self, policy: FetchPolicy) -> Self {
        self.fetch_policy = policy;
        self
    }

    /// Replaces the feed and artifact size limits.
    pub fn with_feed_limits(mut self, limits: FeedLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Updates `target` instead of the running executable's installation.
    pub fn with_install_target(mut self, target: InstallTarget) -> Self {
        self.target = Some(target);
        self
    }

    /// Stages downloads under `root`.
    ///
    /// Defaults: a `.gpui-auto-update` directory inside the install
    /// directory for portable installs (the final rename must stay on one
    /// volume), and `%TEMP%\gpui-auto-update\<executable name>` for
    /// installers.
    pub fn with_staging_root(mut self, root: PathBuf) -> Self {
        self.staging_root = Some(root);
        self
    }

    /// Replaces how installer processes are started.
    pub fn with_launcher(mut self, launcher: impl InstallerLauncher) -> Self {
        self.launcher = Arc::new(launcher);
        self
    }

    /// How long a started installer is watched for an immediate failure
    /// (default [`DEFAULT_LAUNCH_GRACE`]).
    pub fn with_launch_grace(mut self, grace: Duration) -> Self {
        self.launch_grace = grace;
        self
    }
}

impl fmt::Debug for WindowsUpdateConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowsUpdateConfig")
            .field("current_version", &self.current_version)
            .field("strategy", &self.strategy)
            .field("feeds", &self.feeds)
            .field("arch", &self.arch)
            .field("target", &self.target)
            .field("staging_root", &self.staging_root)
            .finish_non_exhaustive()
    }
}

//! Updater configuration.

use std::sync::Arc;

use gpui_auto_update_core::{
    Capability, CheckOutcome, CheckPolicy, CheckRequest, CheckSource, Clock, MemoryPreferenceStore,
    PreferenceStore, SystemClock, UpdateError,
};

use crate::backend::{UnsupportedBackend, UpdateBackend};
use crate::preview::PreviewState;

/// Whether the running binary is a development (debug) or release build.
///
/// Debug builds never install updates unless
/// [`UpdaterConfig::allow_debug_self_update`] is set, so that running the
/// application from a development checkout cannot replace it with a
/// production release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuildProfile {
    /// Built with debug assertions.
    Debug,
    /// Built without debug assertions.
    Release,
}

impl BuildProfile {
    /// The profile of this build, from `cfg!(debug_assertions)`.
    pub fn current() -> Self {
        if cfg!(debug_assertions) {
            Self::Debug
        } else {
            Self::Release
        }
    }
}

/// Everything [`crate::init`] needs to create the updater.
///
/// Only the application identifier and the check source are required; the
/// rest has conservative defaults:
///
/// - the backend is [`UnsupportedBackend`] until a platform backend is
///   supplied, so nothing is ever installed by accident;
/// - the capability is asked from the backend at startup;
/// - preferences are stored in a file at
///   [`default_preferences_path`](crate::default_preferences_path);
/// - the policy is [`CheckPolicy::recommended`];
/// - the build profile is [`BuildProfile::current`] and debug builds do not
///   install updates.
pub struct UpdaterConfig {
    pub(crate) app_id: String,
    pub(crate) source: Arc<dyn CheckSource>,
    pub(crate) backend: Arc<dyn UpdateBackend>,
    pub(crate) capability: Option<Capability>,
    pub(crate) preferences: Option<Box<dyn PreferenceStore>>,
    pub(crate) policy: CheckPolicy,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) build_profile: BuildProfile,
    pub(crate) allow_debug_self_update: bool,
    pub(crate) preview: Option<PreviewState>,
}

impl UpdaterConfig {
    /// A configuration for the application identified by `app_id` (a
    /// reverse-DNS identifier such as `"com.example.App"`, used to locate the
    /// preferences file) that resolves updates with `source`.
    pub fn new(app_id: impl Into<String>, source: impl CheckSource) -> Self {
        Self {
            app_id: app_id.into(),
            source: Arc::new(source),
            backend: Arc::new(UnsupportedBackend),
            capability: None,
            preferences: None,
            policy: CheckPolicy::recommended(),
            clock: Arc::new(SystemClock),
            build_profile: BuildProfile::current(),
            allow_debug_self_update: false,
            preview: None,
        }
    }

    /// A configuration that starts in `preview` and never touches the
    /// network, the filesystem, or the installation.
    ///
    /// Use it to develop and test update UI. The updater stays in preview
    /// until [`Updater::exit_preview`](crate::Updater::exit_preview), after
    /// which it reports an unsupported installation.
    pub fn preview(app_id: impl Into<String>, preview: PreviewState) -> Self {
        Self::new(app_id, NoUpdates)
            .with_capability(Capability::Unsupported)
            .with_preferences(MemoryPreferenceStore::new())
            .with_preview(preview)
    }

    /// The application identifier.
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    /// Sets the backend that stages and installs updates.
    pub fn with_backend(mut self, backend: impl UpdateBackend) -> Self {
        self.backend = Arc::new(backend);
        self
    }

    /// Sets the installation's capability instead of asking the backend,
    /// for example when the application knows it was installed by a package
    /// manager.
    pub fn with_capability(mut self, capability: Capability) -> Self {
        self.capability = Some(capability);
        self
    }

    /// Sets where the automatic-update preference is stored.
    pub fn with_preferences(mut self, store: impl PreferenceStore) -> Self {
        self.preferences = Some(Box::new(store));
        self
    }

    /// Sets the automatic-check policy.
    pub fn with_policy(mut self, policy: CheckPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Sets the wall clock used for check intervals.
    pub fn with_clock(mut self, clock: impl Clock) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// Overrides the detected build profile.
    pub fn with_build_profile(mut self, profile: BuildProfile) -> Self {
        self.build_profile = profile;
        self
    }

    /// Lets a debug build install updates. Only set this when the feed and
    /// backend point at development releases.
    pub fn allow_debug_self_update(mut self, allow: bool) -> Self {
        self.allow_debug_self_update = allow;
        self
    }

    /// Starts the updater in `preview`; see [`Self::preview`].
    pub fn with_preview(mut self, preview: PreviewState) -> Self {
        self.preview = Some(preview);
        self
    }
}

impl std::fmt::Debug for UpdaterConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdaterConfig")
            .field("app_id", &self.app_id)
            .field("capability", &self.capability)
            .field("policy", &self.policy)
            .field("build_profile", &self.build_profile)
            .field("allow_debug_self_update", &self.allow_debug_self_update)
            .field("preview", &self.preview)
            .finish_non_exhaustive()
    }
}

/// A check source that never finds an update; used by preview
/// configurations.
pub(crate) struct NoUpdates;

impl CheckSource for NoUpdates {
    fn check(&self, _: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        Ok(CheckOutcome::UpToDate)
    }
}

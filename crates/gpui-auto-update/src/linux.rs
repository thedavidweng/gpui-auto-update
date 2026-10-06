//! The Linux backend: managed-install staging, the update helper, startup
//! health confirmation, and rollback (see the `gpui-auto-update-linux`
//! crate).
//!
//! [`UpdaterConfig::native_feed`](crate::UpdaterConfig::native_feed) selects
//! it automatically on Linux. Construct it yourself only to customize the
//! check source, downloader, or helper.

use std::sync::{Arc, OnceLock};

use gpui_auto_update_core::check::FeedCheckSource;
use gpui_auto_update_core::download::ArtifactDownloader;
use gpui_auto_update_core::{AvailableUpdate, Capability, Channel, UpdateError};
use gpui_auto_update_linux::{Detection, HelperCommand, LinuxUpdater};

use crate::backend::{Handoff, ProgressSink, UpdateBackend};

pub use gpui_auto_update_linux::{HEALTH_FILE_ENV, HelperDiagnostic, HelperOutcome};

type Detector = Box<dyn Fn(&str) -> Detection + Send + Sync>;

/// Updates a managed user-local install on Linux.
///
/// Installation is finished by the update helper, a mode of the
/// application's own executable, so the application's `main` must call
/// [`run_update_helper_if_requested`](crate::run_update_helper_if_requested)
/// first. [`Self::install`](UpdateBackend::install) returns
/// [`Handoff::Quit`] once the helper has acknowledged the staged release;
/// the helper replaces the install after the application has quit,
/// relaunches it, and waits for
/// [`Updater::main_window_opened`](crate::Updater::main_window_opened).
///
/// Managed-install detection runs on first use, on the background
/// executor, not when the backend is created.
pub struct LinuxBackend {
    source: Arc<FeedCheckSource>,
    downloader: ArtifactDownloader,
    executable_name: Option<String>,
    helper: Option<HelperCommand>,
    detect: Detector,
    updater: OnceLock<LinuxUpdater>,
}

impl LinuxBackend {
    /// A backend that downloads the release `source` selected with
    /// `downloader`, for the running executable's managed install.
    #[cfg(target_os = "linux")]
    pub fn new(source: Arc<FeedCheckSource>, downloader: ArtifactDownloader) -> Self {
        Self::with_detector(
            source,
            downloader,
            Box::new(|name| gpui_auto_update_linux::detect_current(name)),
        )
    }

    fn with_detector(
        source: Arc<FeedCheckSource>,
        downloader: ArtifactDownloader,
        detect: Detector,
    ) -> Self {
        Self {
            source,
            downloader,
            executable_name: None,
            helper: None,
            detect,
            updater: OnceLock::new(),
        }
    }

    /// Sets the application name of the managed-install layout (the
    /// `<app>` in `<prefix>/bin/<app>`). Defaults to the running
    /// executable's file name.
    pub fn with_executable_name(mut self, name: impl Into<String>) -> Self {
        self.executable_name = Some(name.into());
        self
    }

    /// Replaces the helper command, for example to change its timeouts.
    pub fn with_helper(mut self, helper: HelperCommand) -> Self {
        self.helper = Some(helper);
        self
    }

    fn updater(&self) -> &LinuxUpdater {
        self.updater.get_or_init(|| {
            let name = self.executable_name.clone().unwrap_or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.file_name()?.to_str().map(str::to_owned))
                    .unwrap_or_default()
            });
            let updater = LinuxUpdater::new(
                (self.detect)(&name),
                self.source.clone(),
                self.downloader.clone(),
            );
            match &self.helper {
                Some(helper) => updater.with_helper(helper.clone()),
                None => updater,
            }
        })
    }
}

impl std::fmt::Debug for LinuxBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinuxBackend")
            .field("executable_name", &self.executable_name)
            .field("updater", &self.updater.get())
            .finish_non_exhaustive()
    }
}

impl UpdateBackend for LinuxBackend {
    fn capability(&self) -> Capability {
        let updater = self.updater();
        tracing::debug!(reason = %updater.detection().reason(), "Linux install detection");
        updater.capability()
    }

    fn stage(&self, _update: &AvailableUpdate, progress: &ProgressSink) -> Result<(), UpdateError> {
        self.updater().stage(progress.coordinator())
    }

    fn install(
        &self,
        _update: &AvailableUpdate,
        _progress: &ProgressSink,
    ) -> Result<Handoff, UpdateError> {
        self.updater().hand_off()?;
        Ok(Handoff::Quit)
    }

    fn relaunch(&self) -> Result<Handoff, UpdateError> {
        // The helper is already waiting for the application to exit.
        Ok(Handoff::Quit)
    }

    fn take_previous_failure(&self) -> Option<UpdateError> {
        self.updater().take_previous_failure()
    }

    fn confirm_startup(&self) -> Result<(), UpdateError> {
        self.updater().confirm_startup().map(drop)
    }

    fn channel(&self) -> Option<Channel> {
        self.source.channel()
    }

    /// Selects the channel of the shared feed source. The choice is not
    /// persisted; set it again on every launch, as on macOS.
    fn set_channel(&self, channel: Option<Channel>) -> Result<(), UpdateError> {
        self.source.set_channel(channel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_auto_update_core::check::UpdateChecker;
    use gpui_auto_update_core::feed::{Arch, Os, UpdateTarget};
    use gpui_auto_update_core::fetch::{FetchPolicy, HttpClient};
    use gpui_auto_update_core::trust::TrustedKey;
    use gpui_auto_update_core::version::ReleaseVersion;
    use gpui_auto_update_linux::{DetectionInputs, detect};
    use std::path::PathBuf;
    use std::sync::Mutex;

    fn backend(seen: Arc<Mutex<Vec<String>>>) -> LinuxBackend {
        let client = HttpClient::new(FetchPolicy::default());
        let checker = UpdateChecker::new(
            "https://updates.invalid/feed.xml".parse().unwrap(),
            UpdateTarget::new(Os::Linux, Arch::X86_64),
            client.clone(),
        );
        let source = Arc::new(FeedCheckSource::new(
            checker,
            ReleaseVersion::parse("1.0.0").unwrap(),
        ));
        let downloader = ArtifactDownloader::new(client, TrustedKey::insecure_test_key());
        LinuxBackend::with_detector(
            source,
            downloader,
            Box::new(move |name| {
                seen.lock().unwrap().push(name.to_owned());
                detect(&DetectionInputs {
                    app_name: name.to_owned(),
                    arch: "x86_64".into(),
                    euid: 1000,
                    executable: PathBuf::from("/usr/bin/demo"),
                    home: None,
                    root: PathBuf::from("/"),
                })
            }),
        )
        .with_executable_name("demo")
    }

    #[test]
    fn detects_once_and_reports_why_updates_are_unavailable() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let backend = backend(seen.clone());
        assert!(seen.lock().unwrap().is_empty(), "detection is deferred");

        assert!(!backend.capability().can_self_update());
        assert_eq!(backend.take_previous_failure(), None);
        assert_eq!(backend.confirm_startup(), Ok(()));
        assert_eq!(*seen.lock().unwrap(), vec!["demo".to_owned()]);
    }

    #[test]
    fn relaunching_after_a_handoff_quits_for_the_helper() {
        let backend = backend(Arc::default());
        assert_eq!(backend.relaunch(), Ok(Handoff::Quit));
        let error = backend
            .install(
                &AvailableUpdate::new("2.0.0"),
                &ProgressSink::new(gpui_auto_update_core::UpdateCoordinator::new(
                    NoSource,
                    Capability::SelfManaged,
                )),
            )
            .unwrap_err();
        assert_eq!(
            error.kind(),
            gpui_auto_update_core::ErrorKind::UnsupportedInstallation
        );
    }

    #[test]
    fn the_channel_selects_which_feed_entries_checks_consider() {
        let backend = backend(Arc::default());
        assert_eq!(backend.channel(), None);

        backend
            .set_channel(Some(gpui_auto_update_core::Channel::new("beta")))
            .unwrap();
        assert_eq!(
            backend.channel(),
            Some(gpui_auto_update_core::Channel::new("beta"))
        );
        assert_eq!(
            backend.source.channel(),
            Some(gpui_auto_update_core::Channel::new("beta")),
            "checks run through the shared feed source"
        );

        let error = backend
            .set_channel(Some(gpui_auto_update_core::Channel::new("../beta")))
            .unwrap_err();
        assert_eq!(
            error.kind(),
            gpui_auto_update_core::ErrorKind::Configuration
        );

        backend.set_channel(None).unwrap();
        assert_eq!(backend.channel(), None);
    }

    struct NoSource;
    impl gpui_auto_update_core::CheckSource for NoSource {
        fn check(
            &self,
            _: &gpui_auto_update_core::CheckRequest,
        ) -> Result<gpui_auto_update_core::CheckOutcome, UpdateError> {
            Ok(gpui_auto_update_core::CheckOutcome::UpToDate)
        }
    }
}

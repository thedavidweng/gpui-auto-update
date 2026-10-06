//! The Windows backend: check, stage, and hand off one installation.

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use gpui_auto_update_core::check::{FeedCheckSource, UpdateChecker};
use gpui_auto_update_core::download::{ArtifactDownloader, DownloadError, StagedArtifact};
use gpui_auto_update_core::feed::{Arch, FeedLimits, Os, UpdateTarget};
use gpui_auto_update_core::fetch::{FetchPolicy, HttpClient};
use gpui_auto_update_core::trust::{EdSignature, TrustedKey, VerifyError};
use gpui_auto_update_core::version::ReleaseVersion;
use gpui_auto_update_core::{Capability, ErrorKind, UpdateCoordinator, UpdateError, UpdateEvent};
use url::Url;

use crate::config::WindowsUpdateConfig;
use crate::launch::{self, InstallerLauncher};
use crate::portable;
use crate::strategy::{InstallTarget, UpdateStrategy};
use crate::version_check;

/// How the application must end after [`WindowsBackend::install`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WindowsHandoff {
    /// An installer is running and will replace the files and relaunch the
    /// application; quit through the normal quit path without restarting.
    QuitForInstaller,
    /// The new executable is in place; quit and start `executable` once this
    /// process has exited.
    Restart {
        /// The updated executable.
        executable: PathBuf,
    },
}

/// Updates one Windows installation from an architecture-specific signed
/// feed.
///
/// Use [`Self::check_source`] as the update coordinator's check source, so
/// the backend knows which release a check selected. All methods block;
/// call them off the UI thread. Cloning is cheap and shares state.
///
/// The flow is:
///
/// 1. [`Self::stage`] downloads the selected artifact into a fresh staging
///    directory, verifies its declared length and Ed25519 signature,
///    confirms the version embedded in the verified bytes, and only then
///    reports the update as staged. Nothing is executed.
/// 2. After the application has saved its state, [`Self::install`]
///    verifies the staged file again, then either starts the installer as
///    the current user (installer strategies) or swaps the executable in
///    place (portable strategy), and returns how the application must end.
#[derive(Clone, Debug)]
pub struct WindowsBackend {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    arch: Arch,
    feed_url: Url,
    source: Arc<FeedCheckSource>,
    key: TrustedKey,
    strategy: UpdateStrategy,
    fetch_policy: FetchPolicy,
    limits: FeedLimits,
    target: Result<InstallTarget, UpdateError>,
    staging_root: Option<PathBuf>,
    launcher: Arc<dyn InstallerLauncher>,
    launch_grace: Duration,
    staged: Mutex<Option<Staged>>,
}

#[derive(Clone, Debug)]
struct Staged {
    artifact: StagedArtifact,
    signature: EdSignature,
}

impl WindowsBackend {
    /// A backend for `config`.
    ///
    /// Fails with [`ErrorKind::Configuration`] when no feed is declared for
    /// the selected architecture, or the architecture of this build is not
    /// one feeds describe. An install target that cannot be determined is
    /// not an error here; it makes [`Self::capability`] report
    /// [`Capability::Unsupported`].
    pub fn new(config: WindowsUpdateConfig) -> Result<Self, UpdateError> {
        let arch = config.arch.or_else(Arch::current).ok_or_else(|| {
            UpdateError::new(ErrorKind::Configuration)
                .with_diagnostic("this build's architecture has no Windows update feeds")
        })?;
        let feed_url = config
            .feeds
            .iter()
            .find(|(a, _)| *a == arch)
            .map(|(_, url)| url.clone())
            .ok_or_else(|| {
                UpdateError::new(ErrorKind::Configuration).with_diagnostic(format!(
                    "no update feed is configured for {arch}; declare one with WindowsUpdateConfig::with_feed"
                ))
            })?;
        let mut target = UpdateTarget::new(Os::Windows, arch);
        for channel in config.channels {
            target = target.with_channel(channel);
        }
        if let Some(version) = config.system_version {
            target = target.with_system_version(version);
        }
        let checker = UpdateChecker::new(
            feed_url.clone(),
            target,
            HttpClient::new(config.fetch_policy.clone()),
        )
        .with_limits(config.limits.clone());
        let source = Arc::new(FeedCheckSource::new(checker, config.current_version));
        let install_target = match config.target {
            Some(target) => Ok(target),
            None => InstallTarget::current(),
        };
        Ok(Self {
            inner: Arc::new(Inner {
                arch,
                feed_url,
                source,
                key: config.key,
                strategy: config.strategy,
                fetch_policy: config.fetch_policy,
                limits: config.limits,
                target: install_target,
                staging_root: config.staging_root,
                launcher: config.launcher,
                launch_grace: config.launch_grace,
                staged: Mutex::new(None),
            }),
        })
    }

    /// The check source to give the update coordinator.
    pub fn check_source(&self) -> Arc<FeedCheckSource> {
        self.inner.source.clone()
    }

    /// The architecture whose feed is used.
    pub fn arch(&self) -> Arch {
        self.inner.arch
    }

    /// The feed checked for updates.
    pub fn feed_url(&self) -> &Url {
        &self.inner.feed_url
    }

    /// The declared update strategy.
    pub fn strategy(&self) -> &UpdateStrategy {
        &self.inner.strategy
    }

    /// The install target, if it could be determined.
    pub fn install_target(&self) -> Option<&InstallTarget> {
        self.inner.target.as_ref().ok()
    }

    /// The verified artifact waiting to be installed, if any.
    pub fn staged_path(&self) -> Option<PathBuf> {
        self.staged()
            .as_ref()
            .map(|staged| staged.artifact.path().to_path_buf())
    }

    /// Whether this installation can update itself.
    ///
    /// It can when the install target is known, installers can be started
    /// here, and the install directory is writable by the current user.
    /// A directory that is not writable (for example a machine-wide install
    /// under Program Files) is [`Capability::Unsupported`], because updates
    /// never request elevation. For portable installs, this also removes the
    /// executable left by a previous update.
    pub fn capability(&self) -> Capability {
        let target = match &self.inner.target {
            Ok(target) => target,
            Err(error) => {
                tracing::info!(diagnostic = error.diagnostic(), "update target unknown");
                return Capability::Unsupported;
            }
        };
        if !self.inner.launcher.is_supported() {
            return Capability::Unsupported;
        }
        if let Err(error) = probe_writable(target.install_dir()) {
            tracing::info!(
                %error,
                dir = %target.install_dir().display(),
                "install directory is not writable by this user; self-update disabled"
            );
            return Capability::Unsupported;
        }
        if matches!(self.inner.strategy, UpdateStrategy::Portable(_)) {
            portable::remove_backups(target.executable());
        }
        Capability::SelfManaged
    }

    /// Downloads, verifies, and stages the release selected by the last
    /// check, reporting each step to `coordinator`.
    ///
    /// The coordinator must show the available update. On success it shows
    /// [`gpui_auto_update_core::UpdateState::Staged`]; on failure it shows
    /// the returned error, and nothing is left staged. Earlier staging
    /// directories under the staging root are removed first.
    pub fn stage(&self, coordinator: &UpdateCoordinator) -> Result<(), UpdateError> {
        match self.stage_inner(coordinator) {
            Ok(()) => Ok(()),
            Err(Failure::Report(error)) => {
                if let Err(rejected) = coordinator.apply(UpdateEvent::Failed(error.clone())) {
                    tracing::debug!(kind_of_error = ?rejected.kind(), "could not record the staging failure");
                }
                tracing::warn!(
                    kind_of_error = ?error.kind(),
                    diagnostic = error.diagnostic(),
                    "staging the Windows update failed: {error}"
                );
                Err(error)
            }
            Err(Failure::Superseded(error)) => Err(error),
        }
    }

    fn stage_inner(&self, coordinator: &UpdateCoordinator) -> Result<(), Failure> {
        let inner = &*self.inner;
        let target = inner.target.clone().map_err(Failure::Report)?;
        let selected = inner.source.selected().ok_or_else(|| {
            Failure::Report(
                UpdateError::new(ErrorKind::InvalidState)
                    .with_diagnostic("no release is selected; run a check first"),
            )
        })?;
        let root = self.staging_root(&target);
        if let Some(previous) = self.staged().take() {
            let _ = previous.artifact.discard();
        }
        remove_stale_staging(&root);

        let downloader = ArtifactDownloader::new(
            HttpClient::new(inner.fetch_policy.clone()),
            inner.key.clone(),
        )
        .with_max_artifact_bytes(inner.limits.max_artifact_bytes)
        .with_file_name(inner.strategy.staged_file_name())
        .map_err(|error| {
            Failure::Report(
                UpdateError::new(ErrorKind::Configuration).with_diagnostic(error.to_string()),
            )
        })?;
        let artifact = downloader
            .download(&selected.item, &root, |event| coordinator.apply(event))
            .map_err(|error| match error {
                DownloadError::Interrupted(error) => Failure::Superseded(error),
                other => Failure::Report(other.into()),
            })?;
        if let Err(error) = self.confirm(artifact.path(), artifact.expected_version()) {
            let _ = artifact.discard();
            return Err(Failure::Report(error));
        }
        if let Err(error) = coordinator.apply(UpdateEvent::Staged) {
            let _ = artifact.discard();
            return Err(Failure::Superseded(error));
        }
        tracing::info!(
            version = %artifact.expected_version(),
            path = %artifact.path().display(),
            "staged a verified Windows update"
        );
        *self.staged() = Some(Staged {
            artifact,
            signature: selected.item.artifact.signature,
        });
        Ok(())
    }

    /// Installs the staged update and says how the application must end.
    ///
    /// Call this only after the application has saved its state; quit
    /// promptly afterwards. The staged file is verified again (length,
    /// signature, and embedded version) immediately before use. Then:
    ///
    /// - installer strategies start the installer as the current user,
    ///   watch it briefly for an immediate failure, and return
    ///   [`WindowsHandoff::QuitForInstaller`];
    /// - the portable strategy moves the running executable aside, moves the
    ///   verified one into its place (restoring the old one on failure), and
    ///   returns [`WindowsHandoff::Restart`].
    ///
    /// On failure the update stays staged so it can be retried, and the
    /// error is [`ErrorKind::HelperLaunch`] when the installer could not be
    /// started.
    pub fn install(&self) -> Result<WindowsHandoff, UpdateError> {
        let inner = &*self.inner;
        let staged = self.staged().clone().ok_or_else(|| {
            UpdateError::new(ErrorKind::InvalidState).with_diagnostic("no update is staged")
        })?;
        let target = inner.target.clone()?;
        let path = staged.artifact.path();
        self.reverify(&staged)?;
        self.confirm(path, staged.artifact.expected_version())?;

        let handoff = match &inner.strategy {
            UpdateStrategy::Installer(installer) => {
                let command = installer.command(path, &target)?;
                tracing::info!(
                    installer = installer.name(),
                    command = %command.command_line(),
                    "starting the update installer"
                );
                launch::launch_and_watch(&*inner.launcher, &command, inner.launch_grace)?;
                WindowsHandoff::QuitForInstaller
            }
            UpdateStrategy::Portable(_) => {
                portable::replace_executable(path, target.executable())?;
                if let Err(error) = fs::remove_dir_all(staged.artifact.directory()) {
                    tracing::debug!(%error, "could not remove the portable staging directory");
                }
                tracing::info!(
                    executable = %target.executable().display(),
                    "replaced the portable executable"
                );
                WindowsHandoff::Restart {
                    executable: target.executable().to_path_buf(),
                }
            }
        };
        *self.staged() = None;
        Ok(handoff)
    }

    /// How to end the application after an install that already handed off:
    /// installers relaunch the application themselves, while portable
    /// installs restart the updated executable.
    pub fn relaunch_handoff(&self) -> Result<WindowsHandoff, UpdateError> {
        match &self.inner.strategy {
            UpdateStrategy::Installer(_) => Ok(WindowsHandoff::QuitForInstaller),
            UpdateStrategy::Portable(_) => Ok(WindowsHandoff::Restart {
                executable: self.inner.target.clone()?.executable().to_path_buf(),
            }),
        }
    }

    fn confirm(&self, path: &Path, expected: &ReleaseVersion) -> Result<(), UpdateError> {
        match &self.inner.strategy {
            UpdateStrategy::Installer(installer) => installer.confirm_version(path, expected),
            UpdateStrategy::Portable(portable) => {
                let info = version_check::read(path)?;
                version_check::confirm_info(&info, &portable.version_key, expected)?;
                let machine = info.machine();
                if machine.arch() != Some(self.inner.arch) {
                    let built_for = machine
                        .arch()
                        .map_or_else(|| format!("{machine:?}"), |arch| arch.to_string());
                    return Err(UpdateError::new(ErrorKind::ArchiveValidation)
                        .with_message("The downloaded update is for a different kind of PC.")
                        .with_diagnostic(format!(
                            "the executable is built for {built_for} but this installation uses {}",
                            self.inner.arch
                        )));
                }
                Ok(())
            }
        }
    }

    /// Verifies the staged bytes again, so a file changed after staging is
    /// never run.
    fn reverify(&self, staged: &Staged) -> Result<(), UpdateError> {
        let path = staged.artifact.path();
        let staging = |detail: String| {
            UpdateError::new(ErrorKind::Staging).with_diagnostic(format!(
                "the staged update at {} is unusable: {detail}",
                path.display()
            ))
        };
        let metadata = fs::symlink_metadata(path).map_err(|e| staging(e.to_string()))?;
        if !metadata.is_file() {
            return Err(staging("not a regular file".to_owned()));
        }
        let file = File::open(path).map_err(|e| staging(e.to_string()))?;
        self.inner
            .key
            .verify_artifact(
                &staged.signature,
                staged.artifact.length(),
                BufReader::new(file),
            )
            .map_err(|error| match error {
                VerifyError::BadSignature => UpdateError::new(ErrorKind::Signature)
                    .with_diagnostic("the staged update no longer matches its signature"),
                VerifyError::LengthMismatch { expected, actual } => {
                    UpdateError::new(ErrorKind::LengthMismatch).with_diagnostic(format!(
                        "the staged update is {actual} bytes but {expected} were verified"
                    ))
                }
                VerifyError::Io(e) => staging(e.to_string()),
            })
    }

    fn staging_root(&self, target: &InstallTarget) -> PathBuf {
        if let Some(root) = &self.inner.staging_root {
            return root.clone();
        }
        match &self.inner.strategy {
            UpdateStrategy::Portable(_) => target.install_dir().join(portable::STAGING_DIR_NAME),
            UpdateStrategy::Installer(_) => {
                let name = target
                    .executable()
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(sanitize)
                    .unwrap_or_else(|| "app".to_owned());
                std::env::temp_dir().join("gpui-auto-update").join(name)
            }
        }
    }

    fn staged(&self) -> MutexGuard<'_, Option<Staged>> {
        self.inner
            .staged
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

enum Failure {
    /// The failure belongs in the update state.
    Report(UpdateError),
    /// The state moved on without this operation; leave it alone.
    Superseded(UpdateError),
}

/// Proves the current user can create files in `dir` by creating and
/// removing a uniquely named one.
fn probe_writable(dir: &Path) -> std::io::Result<()> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let probe = dir.join(format!(
        ".gpui-auto-update-probe-{}-{nanos}",
        std::process::id()
    ));
    File::options().write(true).create_new(true).open(&probe)?;
    fs::remove_file(&probe)
}

/// Removes staging directories left by earlier runs. Directories in use (an
/// installer still running from one) cannot be removed and are skipped.
fn remove_stale_staging(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("update-"));
        if is_staging && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            if let Err(error) = fs::remove_dir_all(entry.path()) {
                tracing::debug!(%error, path = %entry.path().display(), "could not remove an old staging directory");
            }
        }
    }
}

fn sanitize(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() || clean.starts_with('.') {
        format!("app{clean}")
    } else {
        clean
    }
}

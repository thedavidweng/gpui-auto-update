//! The application side of the helper handoff: start the helper, wait for
//! its acknowledgement, and keep it waiting until the application exits.

use std::ffi::OsString;
use std::io::{self, BufRead as _, BufReader};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use gpui_auto_update_core::{ErrorKind, UpdateError};

use crate::detect::ManagedInstall;
use crate::extract::StagedRelease;
use crate::helper::{self, HELPER_ARG};

const DEFAULT_ACK_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_secs(60);

/// What the helper is asked to install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandoffRequest {
    app_name: String,
    install: PathBuf,
    staged: PathBuf,
    version: Option<String>,
}

impl HandoffRequest {
    /// A request to replace the managed install at `install_prefix` with the
    /// staged release at `staged_prefix` (see [`StagedRelease::prefix`]).
    ///
    /// The helper checks both layouts itself, so a request built from
    /// arbitrary paths is refused rather than trusted.
    pub fn new(
        app_name: impl Into<String>,
        install_prefix: impl AsRef<Path>,
        staged_prefix: impl AsRef<Path>,
    ) -> Self {
        Self {
            app_name: app_name.into(),
            install: install_prefix.as_ref().to_path_buf(),
            staged: staged_prefix.as_ref().to_path_buf(),
            version: None,
        }
    }

    /// A request to replace `install` with `staged`.
    pub fn for_release(install: &ManagedInstall, staged: &StagedRelease) -> Self {
        Self::new(install.app_name(), install.prefix(), staged.prefix())
            .with_version(staged.version().as_str())
    }

    /// The version being installed, recorded in diagnostics.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }
}

/// Why the helper did not accept a handoff.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HandoffError {
    /// The helper process could not be started.
    #[error("could not start the update helper: {0}")]
    Spawn(#[source] io::Error),
    /// The helper checked the request and refused it.
    #[error("the update helper refused the update: {0}")]
    Refused(String),
    /// The helper exited, or did not answer in time, without acknowledging.
    #[error("the update helper did not acknowledge the update: {0}")]
    NoAcknowledgement(String),
}

impl From<HandoffError> for UpdateError {
    fn from(error: HandoffError) -> Self {
        UpdateError::new(ErrorKind::HelperLaunch)
            .with_diagnostic(error.to_string())
            .with_source(error)
    }
}

/// Starts the update helper.
///
/// The helper is a mode of the application's own executable (see
/// [`run_helper_if_requested`](crate::run_helper_if_requested)), so by
/// default it is the running executable.
#[derive(Clone, Debug)]
pub struct HelperCommand {
    executable: PathBuf,
    ack_timeout: Duration,
    health_timeout: Duration,
}

impl HelperCommand {
    /// The helper in the running executable.
    pub fn current() -> io::Result<Self> {
        Ok(Self::new(std::env::current_exe()?))
    }

    /// The helper in `executable`, whose `main` must call
    /// [`run_helper_if_requested`](crate::run_helper_if_requested) first.
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            ack_timeout: DEFAULT_ACK_TIMEOUT,
            health_timeout: DEFAULT_HEALTH_TIMEOUT,
        }
    }

    /// How long to wait for the helper's acknowledgement (default 30 s).
    pub fn with_ack_timeout(mut self, timeout: Duration) -> Self {
        self.ack_timeout = timeout;
        self
    }

    /// How long the helper waits for the new version to confirm its start
    /// (default 60 s) before reporting it as unconfirmed.
    pub fn with_health_timeout(mut self, timeout: Duration) -> Self {
        self.health_timeout = timeout;
        self
    }

    /// Starts the helper for `request` and waits until it has checked both
    /// the current and the staged installation.
    ///
    /// On success the helper is waiting for this process to exit, and
    /// nothing has been changed yet; [`PendingHelper::commit`] it and then
    /// quit normally. On failure the helper has exited and nothing was
    /// changed. Blocks for up to the acknowledgement timeout.
    pub fn hand_off(&self, request: &HandoffRequest) -> Result<PendingHelper, HandoffError> {
        let mut command = Command::new(&self.executable);
        command
            .args(self.arguments(request))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            // Keep terminal signals aimed at the application (Ctrl-C) from
            // reaching the helper while it swaps the install.
            .process_group(0);
        let mut child = command.spawn().map_err(HandoffError::Spawn)?;
        let stdin = child.stdin.take();
        let mut pending = PendingHelper {
            child: Some(child),
            stdin,
        };
        let stdout = pending
            .child
            .as_mut()
            .and_then(|child| child.stdout.take())
            .ok_or_else(|| HandoffError::NoAcknowledgement("no output pipe".into()))?;

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
            let _ = tx.send(result);
        });
        let line = match rx.recv_timeout(self.ack_timeout) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => return Err(HandoffError::NoAcknowledgement(error.to_string())),
            Err(_) => {
                return Err(HandoffError::NoAcknowledgement(format!(
                    "no answer within {:?}",
                    self.ack_timeout
                )));
            }
        };
        match helper::parse_ack(&line) {
            Ok(()) => Ok(pending),
            Err(Some(reason)) => Err(HandoffError::Refused(reason)),
            Err(None) => Err(HandoffError::NoAcknowledgement(if line.is_empty() {
                "the helper exited".into()
            } else {
                format!("unexpected answer {line:?}")
            })),
        }
    }

    fn arguments(&self, request: &HandoffRequest) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            HELPER_ARG.into(),
            "--app".into(),
            request.app_name.clone().into(),
            "--install".into(),
            request.install.clone().into(),
            "--staged".into(),
            request.staged.clone().into(),
            "--parent-pid".into(),
            std::process::id().to_string().into(),
            "--health-timeout-ms".into(),
            self.health_timeout.as_millis().to_string().into(),
        ];
        if let Some(version) = &request.version {
            args.push("--version".into());
            args.push(version.clone().into());
        }
        args
    }
}

/// A helper that acknowledged a handoff and waits for the application to
/// exit.
///
/// The helper starts replacing the install only once this process has
/// exited. Dropping this value without [`Self::commit`] stops the helper.
#[derive(Debug)]
pub struct PendingHelper {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
}

impl PendingHelper {
    /// The helper's process id.
    pub fn id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Lets the helper proceed once this process exits.
    ///
    /// The helper watches a pipe that only closes when this process ends,
    /// so the application can finish its normal quit and save path at its
    /// own pace, and the install is never replaced while it still runs.
    pub fn commit(mut self) {
        // The pipe's write end must stay open until the process exits, so it
        // is deliberately leaked; the kernel closes it on exit.
        std::mem::forget(self.stdin.take());
        self.child = None;
    }

    /// Stops the helper without installing anything.
    pub fn cancel(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.stdin = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for PendingHelper {
    fn drop(&mut self) {
        self.stop();
    }
}

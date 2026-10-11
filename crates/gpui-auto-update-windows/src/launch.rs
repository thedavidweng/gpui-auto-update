//! Starting installer processes.

use std::fmt;
use std::io;
use std::thread;
use std::time::{Duration, Instant};

use gpui_auto_update_core::{ErrorKind, UpdateError};

use crate::strategy::InstallerCommand;

/// `ERROR_ELEVATION_REQUIRED`: the program's manifest asks for
/// administrator rights, which `CreateProcess` never grants.
const ERROR_ELEVATION_REQUIRED: i32 = 740;
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Starts installer processes for the backend.
///
/// [`SystemLauncher`] is the real implementation. Replace it to observe
/// handoffs in tests or to customize process creation.
pub trait InstallerLauncher: Send + Sync + fmt::Debug + 'static {
    /// Whether this launcher can start Windows installers here. When it
    /// cannot, the backend reports the installation as unsupported.
    fn is_supported(&self) -> bool {
        true
    }

    /// Starts `command` as the current user, without elevation and without
    /// waiting for it to finish. The process must outlive the application.
    fn launch(&self, command: &InstallerCommand) -> io::Result<Box<dyn LaunchedInstaller>>;
}

/// An installer process that has been started.
pub trait LaunchedInstaller: Send {
    /// The exit code if the process has exited, without blocking.
    fn try_wait(&mut self) -> io::Result<Option<i32>>;
}

/// Starts installers with `CreateProcessW` (through `std::process`), never
/// with the `runas` verb, so an installer that requires administrator rights
/// fails to start instead of prompting for elevation.
///
/// The installer runs detached from the application's console and job
/// object where Windows allows it, with its staging directory as its working
/// directory so the install directory is not held open. On other platforms
/// it is unsupported.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemLauncher;

impl InstallerLauncher for SystemLauncher {
    fn is_supported(&self) -> bool {
        cfg!(windows)
    }

    fn launch(&self, command: &InstallerCommand) -> io::Result<Box<dyn LaunchedInstaller>> {
        #[cfg(windows)]
        {
            crate::sys::launch(command)
        }
        #[cfg(not(windows))]
        {
            let _ = command;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Windows installers can only be started on Windows",
            ))
        }
    }
}

/// Starts `command` and watches it for `grace`: an installer that cannot
/// start, or exits with an error before the application quits, is reported
/// while the application can still show it.
pub(crate) fn launch_and_watch(
    launcher: &dyn InstallerLauncher,
    command: &InstallerCommand,
    grace: Duration,
) -> Result<(), UpdateError> {
    let mut process = launcher
        .launch(command)
        .map_err(|error| launch_error(error, command))?;
    let deadline = Instant::now() + grace;
    loop {
        match process.try_wait() {
            Ok(Some(0)) => return Ok(()),
            Ok(Some(code)) => {
                return Err(
                    UpdateError::new(ErrorKind::HelperLaunch).with_diagnostic(format!(
                        "installer exited with code {code} right after starting: {}",
                        command.command_line()
                    )),
                );
            }
            Ok(None) => {}
            Err(error) => {
                tracing::debug!(%error, "could not poll the installer process");
                return Ok(());
            }
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(());
        }
        thread::sleep(POLL_INTERVAL.min(deadline - now));
    }
}

fn launch_error(error: io::Error, command: &InstallerCommand) -> UpdateError {
    let base = UpdateError::new(ErrorKind::HelperLaunch);
    let base = if error.raw_os_error() == Some(ERROR_ELEVATION_REQUIRED) {
        base.with_message(
            "The update installer asked for administrator rights. Updates install for the \
             current user only and never request them.",
        )
    } else {
        base
    };
    base.with_diagnostic(format!(
        "could not start the installer ({error}): {}",
        command.command_line()
    ))
    .with_source(error)
}

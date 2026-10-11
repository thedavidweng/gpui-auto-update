//! The helper process: acknowledge a handoff, wait for the application to
//! quit, swap the managed install, relaunch, wait for the health signal,
//! and roll back on failure.
//!
//! The helper is a mode of the application's own executable, entered
//! through [`run_helper_if_requested`]. The decision and the protocol are
//! recorded in `docs/adr/0003-linux-update-helper.md`.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use gpui_auto_update_core::ErrorKind;

use crate::diagnostic::{self, HelperDiagnostic, HelperOutcome};
use crate::extract::validate_layout;
use crate::health::{self, HEALTH_FILE_ENV};
use crate::siblings;

/// The first argument that selects helper mode.
pub(crate) const HELPER_ARG: &str = "--gpui-auto-update-helper";

const READY: &str = "READY";
const ERROR: &str = "ERROR\t";
const POLL: Duration = Duration::from_millis(25);
/// How long the application may take to disappear after closing its end of
/// the handoff pipe, which happens while it exits.
const EXIT_GRACE: Duration = Duration::from_secs(30);

/// Runs the update helper and exits the process if this process was started
/// as one; otherwise returns immediately without side effects.
///
/// Call it first thing in `main`, before creating the GPUI application, in
/// every application that uses the Linux backend: the backend starts the
/// application's own executable in helper mode to finish an update after
/// the application has quit. It is a no-op unless the first argument is
/// the private helper flag.
pub fn run_helper_if_requested() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(HELPER_ARG)) {
        return;
    }
    let code = run(args.collect());
    std::process::exit(code);
}

/// Interprets the helper's first line of output: `Ok` for an
/// acknowledgement, `Err(Some(reason))` for a refusal, and `Err(None)` for
/// anything else.
pub(crate) fn parse_ack(line: &str) -> Result<(), Option<String>> {
    let line = line.strip_suffix('\n').unwrap_or(line);
    if line == READY {
        Ok(())
    } else if let Some(reason) = line.strip_prefix(ERROR) {
        Err(Some(reason.to_owned()))
    } else {
        Err(None)
    }
}

struct Args {
    app: String,
    install: PathBuf,
    staged: PathBuf,
    parent_pid: u32,
    health_timeout: Duration,
    version: Option<String>,
}

impl Args {
    fn parse(args: Vec<OsString>) -> Result<Self, String> {
        let mut app = None;
        let mut install = None;
        let mut staged = None;
        let mut parent_pid = None;
        let mut health_timeout = None;
        let mut version = None;
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| format!("{} needs a value", flag.to_string_lossy()))?;
            let text = || {
                value
                    .to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{} is not UTF-8", flag.to_string_lossy()))
            };
            match flag.as_bytes() {
                b"--app" => app = Some(text()?),
                b"--install" => install = Some(PathBuf::from(&value)),
                b"--staged" => staged = Some(PathBuf::from(&value)),
                b"--parent-pid" => {
                    parent_pid = Some(text()?.parse().map_err(|_| "invalid --parent-pid")?);
                }
                b"--health-timeout-ms" => {
                    let ms: u64 = text()?.parse().map_err(|_| "invalid --health-timeout-ms")?;
                    health_timeout = Some(Duration::from_millis(ms));
                }
                b"--version" => version = Some(text()?),
                _ => return Err(format!("unknown argument {}", flag.to_string_lossy())),
            }
        }
        Ok(Self {
            app: app.ok_or("missing --app")?,
            install: install.ok_or("missing --install")?,
            staged: staged.ok_or("missing --staged")?,
            parent_pid: parent_pid.ok_or("missing --parent-pid")?,
            health_timeout: health_timeout.ok_or("missing --health-timeout-ms")?,
            version,
        })
    }
}

/// A checked handoff.
struct Plan {
    app: String,
    install: PathBuf,
    parent: PathBuf,
    staged: PathBuf,
    staging_dir: PathBuf,
    version: Option<String>,
    health_timeout: Duration,
}

fn run(args: Vec<OsString>) -> i32 {
    let plan = match Args::parse(args).and_then(|args| check(&args).map(|plan| (args, plan))) {
        Ok((args, plan)) => {
            if answer(READY).is_err() {
                // The application is already gone; it never saw the
                // acknowledgement, so it did not quit for this update.
                return 2;
            }
            if let Err(reason) = wait_for_exit(args.parent_pid) {
                log(&reason);
                return 2;
            }
            plan
        }
        Err(reason) => {
            let _ = answer(&format!("{ERROR}{}", single_line(&reason)));
            return 2;
        }
    };
    plan.finish()
}

/// Checks that the handoff describes a managed install and a staged
/// release next to it, both with valid layouts, from the process that
/// started the helper.
fn check(args: &Args) -> Result<Plan, String> {
    #[cfg(target_os = "linux")]
    let euid = {
        let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
        match crate::proc_status::effective_uid(&status) {
            Some(0) | None => return Err("the update helper does not run as root".into()),
            Some(uid) => uid,
        }
    };
    if std::os::unix::process::parent_id() != args.parent_pid {
        return Err("the helper was not started by the application it updates".into());
    }
    let install = args
        .install
        .canonicalize()
        .map_err(|error| format!("the current installation cannot be resolved: {error}"))?;
    if install != args.install {
        return Err("the current installation path is not canonical".into());
    }
    let staged = args
        .staged
        .canonicalize()
        .map_err(|error| format!("the staged release cannot be resolved: {error}"))?;
    let (parent, install_name) = siblings::split(&install).map_err(|error| error.to_string())?;
    let staging_dir = staged
        .parent()
        .filter(|dir| dir.parent() == Some(parent))
        .filter(|dir| {
            let expected = siblings::role_prefix(install_name, siblings::STAGED);
            dir.file_name()
                .is_some_and(|name| name.as_bytes().starts_with(expected.as_bytes()))
        })
        .ok_or("the staged release is not in a staging directory next to the installation")?
        .to_path_buf();
    validate_layout(&install, &args.app)
        .map_err(|error| format!("the current installation is not a managed install: {error}"))?;
    validate_layout(&staged, &args.app)
        .map_err(|error| format!("the staged release is not a managed install: {error}"))?;
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt as _;
        let owned = [install.as_path(), parent, &staging_dir, &staged]
            .iter()
            .all(|path| fs::symlink_metadata(path).is_ok_and(|meta| meta.uid() == euid));
        if !owned {
            return Err("the installation is not owned by the current user".into());
        }
    }
    Ok(Plan {
        app: args.app.clone(),
        parent: parent.to_path_buf(),
        install,
        staged,
        staging_dir,
        version: args.version.clone(),
        health_timeout: args.health_timeout,
    })
}

/// Waits until the application has exited: first for its end of the stdin
/// pipe to close, which happens only as the process ends (or when it gives
/// up on the handoff), then for this process to be reparented.
fn wait_for_exit(parent_pid: u32) -> Result<(), String> {
    let mut stdin = io::stdin().lock();
    let mut buf = [0u8; 256];
    loop {
        match stdin.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let deadline = Instant::now() + EXIT_GRACE;
    while std::os::unix::process::parent_id() == parent_pid {
        if Instant::now() >= deadline {
            return Err("the application cancelled the update without exiting".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

impl Plan {
    /// Replaces the install and supervises the new version's start.
    fn finish(self) -> i32 {
        // The files may have changed while the application was quitting.
        if let Err(error) = validate_layout(&self.staged, &self.app) {
            return self.not_installed(format!(
                "the staged release changed after the handoff: {error}"
            ));
        }
        if let Err(error) = validate_layout(&self.install, &self.app) {
            return self.not_installed(format!(
                "the current installation changed after the handoff: {error}"
            ));
        }

        let backup = match siblings::reserve_dir(&self.install, siblings::BACKUP) {
            Ok(backup) => backup,
            Err(error) => {
                return self.not_installed(format!("could not reserve a backup name: {error}"));
            }
        };
        if let Err(error) = fs::rename(&self.install, &backup) {
            let _ = fs::remove_dir(&backup);
            return self
                .not_installed(format!("could not move the current version aside: {error}"));
        }
        if let Err(error) = fs::rename(&self.staged, &self.install) {
            return match fs::rename(&backup, &self.install) {
                Ok(()) => {
                    self.not_installed(format!("could not move the new version in place: {error}"))
                }
                Err(restore) => self.rollback_failed(format!(
                    "could not move the new version in place ({error}) or restore the previous \
                     version ({restore}); it is kept at {}",
                    backup.display()
                )),
            };
        }
        let _ = siblings::sync_dir(&self.parent);
        let _ = fs::remove_dir(&self.staging_dir);

        let health = match siblings::unique(&self.install, siblings::HEALTH) {
            Ok(path) => path,
            Err(error) => {
                return self.roll_back(
                    &backup,
                    ErrorKind::Relaunch,
                    format!("could not reserve a health file: {error}"),
                );
            }
        };
        let mut child = match self.launch(Some(&health)) {
            Ok(child) => child,
            Err(error) => {
                return self.roll_back(
                    &backup,
                    ErrorKind::Relaunch,
                    format!("could not launch the new version: {error}"),
                );
            }
        };

        let deadline = Instant::now() + self.health_timeout;
        loop {
            if health::is_signaled(&health) {
                let _ = fs::remove_file(&health);
                let _ = fs::remove_dir_all(&backup);
                let _ = siblings::sync_dir(&self.parent);
                return 0;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    // The signal may have been sent just before exiting.
                    if health::is_signaled(&health) {
                        continue;
                    }
                    return self.roll_back(
                        &backup,
                        ErrorKind::HealthConfirmation,
                        format!("the new version exited ({status}) before confirming its start"),
                    );
                }
                Ok(None) => {}
                Err(error) => log(&format!("could not watch the new version: {error}")),
            }
            if Instant::now() >= deadline {
                // Never pull an install out from under a running process.
                let _ = self.record(HelperDiagnostic::new(
                    HelperOutcome::Unconfirmed,
                    ErrorKind::HealthConfirmation,
                    self.version.clone(),
                    format!(
                        "the new version did not confirm its start within {} ms; the previous \
                         version is kept at {}",
                        self.health_timeout.as_millis(),
                        backup.display()
                    ),
                ));
                return 1;
            }
            std::thread::sleep(POLL);
        }
    }

    /// Restores the previous version from `backup` and relaunches it.
    fn roll_back(&self, backup: &Path, kind: ErrorKind, cause: String) -> i32 {
        log(&cause);
        let failed = match siblings::reserve_dir(&self.install, siblings::FAILED) {
            Ok(failed) => failed,
            Err(error) => {
                return self.rollback_failed(format!(
                    "{cause}; could not reserve a name for the failed version ({error}); the \
                     previous version is kept at {}",
                    backup.display()
                ));
            }
        };
        if let Err(error) = fs::rename(&self.install, &failed) {
            let _ = fs::remove_dir(&failed);
            return self.rollback_failed(format!(
                "{cause}; could not move the failed version aside ({error}); the previous \
                 version is kept at {}",
                backup.display()
            ));
        }
        if let Err(error) = fs::rename(backup, &self.install) {
            let restored = fs::rename(&failed, &self.install).is_ok();
            return self.rollback_failed(format!(
                "{cause}; could not restore the previous version ({error}); it is kept at {}{}",
                backup.display(),
                if restored {
                    ""
                } else {
                    "; the installation directory is missing"
                }
            ));
        }
        let _ = siblings::sync_dir(&self.parent);
        let _ = fs::remove_dir_all(&failed);

        // Recorded before relaunching so the restored version can show it.
        let diagnostic = HelperDiagnostic::new(
            HelperOutcome::RolledBack,
            kind,
            self.version.clone(),
            cause.clone(),
        );
        let _ = self.record(diagnostic);
        if let Err(error) = self.launch(None) {
            let _ = self.record(HelperDiagnostic::new(
                HelperOutcome::RolledBack,
                kind,
                self.version.clone(),
                format!("{cause}; relaunching the previous version failed: {error}"),
            ));
        }
        1
    }

    /// Keeps the current version, discards the staged one, and relaunches.
    fn not_installed(&self, cause: String) -> i32 {
        log(&cause);
        let _ = fs::remove_dir_all(&self.staging_dir);
        let _ = self.record(HelperDiagnostic::new(
            HelperOutcome::NotInstalled,
            ErrorKind::Replacement,
            self.version.clone(),
            cause.clone(),
        ));
        if let Err(error) = self.launch(None) {
            let _ = self.record(HelperDiagnostic::new(
                HelperOutcome::NotInstalled,
                ErrorKind::Replacement,
                self.version.clone(),
                format!("{cause}; relaunching the current version failed: {error}"),
            ));
        }
        1
    }

    fn rollback_failed(&self, cause: String) -> i32 {
        log(&cause);
        let _ = self.record(HelperDiagnostic::new(
            HelperOutcome::RollbackFailed,
            ErrorKind::Rollback,
            self.version.clone(),
            cause,
        ));
        1
    }

    fn record(&self, diagnostic: HelperDiagnostic) -> io::Result<()> {
        diagnostic::record(&self.install, &diagnostic)
            .map(drop)
            .inspect_err(|error| log(&format!("could not record a diagnostic: {error}")))
    }

    /// Starts the installed application, passing `health` when it must
    /// confirm its start.
    fn launch(&self, health: Option<&Path>) -> io::Result<Child> {
        let mut command = Command::new(self.install.join("bin").join(&self.app));
        // The helper's stdout is the pipe to the application that has quit,
        // so the relaunched application must not inherit it.
        command.stdin(Stdio::null()).stdout(Stdio::null());
        if let Some(health) = health {
            command.env(HEALTH_FILE_ENV, health);
        } else {
            command.env_remove(HEALTH_FILE_ENV);
        }
        command.spawn()
    }
}

fn answer(line: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(line.as_bytes())?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}

fn log(message: &str) {
    let _ = writeln!(io::stderr(), "gpui-auto-update helper: {message}");
}

fn single_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledgements_are_recognized() {
        assert_eq!(parse_ack("READY\n"), Ok(()));
        assert_eq!(
            parse_ack("ERROR\tthe staged release is broken\n"),
            Err(Some("the staged release is broken".to_owned()))
        );
        assert_eq!(parse_ack(""), Err(None));
        assert_eq!(parse_ack("READYish\n"), Err(None));
    }

    #[test]
    fn incomplete_arguments_are_refused() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(Args::parse(args(&["--app", "demo"])).is_err());
        assert!(Args::parse(args(&["--app"])).is_err());
        assert!(Args::parse(args(&["--bogus", "x"])).is_err());
        let parsed = Args::parse(args(&[
            "--app",
            "demo",
            "--install",
            "/a",
            "--staged",
            "/b",
            "--parent-pid",
            "7",
            "--health-timeout-ms",
            "50",
        ]))
        .unwrap();
        assert_eq!(parsed.parent_pid, 7);
        assert_eq!(parsed.health_timeout, Duration::from_millis(50));
        assert_eq!(parsed.version, None);
    }
}

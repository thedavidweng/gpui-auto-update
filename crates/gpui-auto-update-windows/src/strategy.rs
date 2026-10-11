//! Update strategies: how a verified Windows artifact becomes the running
//! installation.
//!
//! The strategy is always declared by the application. Nothing here looks at
//! artifact file names, URLs, or MIME types to decide what an artifact is.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_auto_update_core::version::ReleaseVersion;
use gpui_auto_update_core::{ErrorKind, UpdateError};

use crate::version_check::{self, DEFAULT_VERSION_KEY};

/// Where the running application is installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallTarget {
    install_dir: PathBuf,
    executable: PathBuf,
}

impl InstallTarget {
    /// A target whose executable is `executable_name` inside `install_dir`.
    ///
    /// `executable_name` must be a single file name.
    pub fn new(install_dir: PathBuf, executable_name: &str) -> Result<Self, UpdateError> {
        let plain = !executable_name.is_empty()
            && executable_name != "."
            && executable_name != ".."
            && !executable_name
                .chars()
                .any(|c| matches!(c, '/' | '\\' | ':' | '\0') || c.is_control());
        if !plain {
            return Err(
                UpdateError::new(ErrorKind::Configuration).with_diagnostic(format!(
                    "executable name {executable_name:?} is not a single file name"
                )),
            );
        }
        let executable = install_dir.join(executable_name);
        Ok(Self {
            install_dir,
            executable,
        })
    }

    /// The target of the executable at `executable`, an absolute path; its
    /// directory is the install directory.
    pub fn for_executable(executable: &Path) -> Result<Self, UpdateError> {
        let unsupported = || {
            UpdateError::new(ErrorKind::UnsupportedInstallation).with_diagnostic(format!(
                "cannot locate the installation of {}",
                executable.display()
            ))
        };
        if !executable.is_absolute() {
            return Err(unsupported());
        }
        let dir = executable.parent().ok_or_else(unsupported)?;
        let name = executable
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(unsupported)?;
        Self::new(dir.to_path_buf(), name)
    }

    /// The target of the running executable, with symbolic links and
    /// `..` components resolved.
    pub fn current() -> Result<Self, UpdateError> {
        let exe = std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .map_err(|error| {
                UpdateError::new(ErrorKind::UnsupportedInstallation)
                    .with_diagnostic(format!("cannot locate the running executable: {error}"))
                    .with_source(error)
            })?;
        Self::for_executable(&exe)
    }

    /// The directory the application is installed in, which updates must
    /// keep using.
    pub fn install_dir(&self) -> &Path {
        &self.install_dir
    }

    /// The application executable.
    pub fn executable(&self) -> &Path {
        &self.executable
    }
}

/// A process to start for an installer handoff: a program and its
/// arguments, already quoted for the Windows command line.
///
/// Arguments are passed to the process exactly as given (`raw_arg`), because
/// installers such as Inno Setup and `msiexec` parse their own command lines
/// and expect switches like `/DIR="C:\Path"` rather than the C runtime's
/// quoting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallerCommand {
    program: PathBuf,
    args: Vec<String>,
}

impl InstallerCommand {
    /// A command that runs `program` with no arguments.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    /// Appends one argument verbatim.
    pub fn raw_arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// The program to start.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// The arguments, in order, exactly as they are passed.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// The full command line, for logs and diagnostics.
    pub fn command_line(&self) -> String {
        let mut line = format!("\"{}\"", self.program.display());
        for arg in &self.args {
            line.push(' ');
            line.push_str(arg);
        }
        line
    }
}

/// An installer technology that installs a verified artifact over the
/// running installation and relaunches the application.
///
/// [`InnoSetup`] is built in. Other technologies, such as MSI, implement
/// this trait; see `docs/windows-installers.md` in the repository.
/// Implementations only describe the handoff; the backend verifies the
/// artifact's signature first, confirms its version through
/// [`Self::confirm_version`], starts the [`InstallerCommand`] as the current
/// user without elevation, and then quits the application.
///
/// The installer owns relaunching: after it replaces the files it must start
/// the new version itself (Inno Setup through a `[Run]` entry), because the
/// application has quit by then.
pub trait InstallerStrategy: Send + Sync + fmt::Debug + 'static {
    /// A short name for logs, such as `"Inno Setup"`.
    fn name(&self) -> &str;

    /// The file name the verified artifact is given before it runs. It must
    /// be a plain ASCII file name; installers usually need their extension.
    fn staged_file_name(&self) -> &str;

    /// Confirms that the artifact at `artifact` is release `expected`, from
    /// metadata inside the signed bytes. Returning `Ok` without checking
    /// would let a relabeled older release be installed.
    fn confirm_version(
        &self,
        artifact: &Path,
        expected: &ReleaseVersion,
    ) -> Result<(), UpdateError>;

    /// The process that installs `artifact` into `target`'s install
    /// directory, per user and without prompts.
    fn command(
        &self,
        artifact: &Path,
        target: &InstallTarget,
    ) -> Result<InstallerCommand, UpdateError>;
}

/// The default switch set: no wizard, no message boxes, no reboot, per-user
/// install mode, and Restart Manager closing any instance that still holds
/// files, without restarting it (the installer's `[Run]` entry relaunches).
const DEFAULT_SWITCHES: &[&str] = &[
    "/VERYSILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/SP-",
    "/CURRENTUSER",
    "/CLOSEAPPLICATIONS",
    "/NORESTARTAPPLICATIONS",
];

/// Silent handoff to an [Inno Setup](https://jrsoftware.org/isinfo.php)
/// installer.
///
/// The command is the configured switch set, then `/LOG="<file>"` when a log
/// file is set, then `/DIR="<install dir>"`, so a portable or relocated copy
/// is updated where it runs rather than at the script's default location.
/// The default switches are
/// `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /CURRENTUSER /CLOSEAPPLICATIONS /NORESTARTAPPLICATIONS`;
/// [`Self::passive`] uses `/SILENT` to show progress instead.
///
/// The installer's version is confirmed from its `ProductVersion` version
/// resource string, which the script must set to the feed version (see
/// `docs/windows-installers.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InnoSetup {
    switches: Vec<String>,
    log_file: Option<PathBuf>,
    version_key: String,
}

impl Default for InnoSetup {
    fn default() -> Self {
        Self::new()
    }
}

impl InnoSetup {
    /// Very silent handoff with the default switches.
    pub fn new() -> Self {
        Self {
            switches: DEFAULT_SWITCHES.iter().map(|s| (*s).to_owned()).collect(),
            log_file: None,
            version_key: DEFAULT_VERSION_KEY.to_owned(),
        }
    }

    /// Like [`Self::new`], but with `/SILENT`: the installer shows a progress
    /// window and nothing else.
    pub fn passive() -> Self {
        let mut inno = Self::new();
        inno.switches[0] = "/SILENT".to_owned();
        inno
    }

    /// Replaces the switch set.
    ///
    /// Each switch must be one token starting with `/`. `/DIR=` is always
    /// added by the backend, `/LOG` through [`Self::with_log_file`], and
    /// `/ALLUSERS` is refused because per-user updates never elevate.
    pub fn with_switches<I, S>(mut self, switches: I) -> Result<Self, InvalidSwitch>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let switches = switches
            .into_iter()
            .map(|s| validate_switch(s.into()))
            .collect::<Result<_, _>>()?;
        self.switches = switches;
        Ok(self)
    }

    /// Appends one switch to the set; see [`Self::with_switches`].
    pub fn with_extra_switch(mut self, switch: impl Into<String>) -> Result<Self, InvalidSwitch> {
        self.switches.push(validate_switch(switch.into())?);
        Ok(self)
    }

    /// Asks the installer to write its log to `path` (`/LOG="<path>"`).
    pub fn with_log_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.log_file = Some(path.into());
        self
    }

    /// Uses version resource string `key` instead of `ProductVersion` to
    /// confirm the installer's version.
    pub fn with_version_key(mut self, key: impl Into<String>) -> Self {
        self.version_key = key.into();
        self
    }

    /// The configured switch set, without `/LOG` and `/DIR`.
    pub fn switches(&self) -> &[String] {
        &self.switches
    }
}

impl InstallerStrategy for InnoSetup {
    fn name(&self) -> &str {
        "Inno Setup"
    }

    fn staged_file_name(&self) -> &str {
        "setup.exe"
    }

    fn confirm_version(
        &self,
        artifact: &Path,
        expected: &ReleaseVersion,
    ) -> Result<(), UpdateError> {
        version_check::confirm_embedded_version(artifact, &self.version_key, expected)
    }

    fn command(
        &self,
        artifact: &Path,
        target: &InstallTarget,
    ) -> Result<InstallerCommand, UpdateError> {
        let mut command = InstallerCommand::new(artifact);
        for switch in &self.switches {
            command = command.raw_arg(switch.clone());
        }
        if let Some(log) = &self.log_file {
            command = command.raw_arg(format!("/LOG={}", quoted_path(log)?));
        }
        Ok(command.raw_arg(format!("/DIR={}", quoted_path(target.install_dir())?)))
    }
}

/// A switch that is not a single safe Inno Setup switch token.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{switch:?} is not an allowed installer switch: {reason}")]
pub struct InvalidSwitch {
    /// The rejected switch.
    pub switch: String,
    /// Why it was rejected.
    pub reason: &'static str,
}

fn validate_switch(switch: String) -> Result<String, InvalidSwitch> {
    let reject = |reason| {
        Err(InvalidSwitch {
            switch: switch.clone(),
            reason,
        })
    };
    if switch.len() < 2 || !switch.starts_with('/') {
        return reject("switches start with '/' and have a name");
    }
    if switch
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '"')
    {
        return reject("switches are a single token without quotes");
    }
    let upper = switch.to_ascii_uppercase();
    if upper.starts_with("/DIR=") {
        return reject("the install directory is always the running installation's");
    }
    if upper == "/ALLUSERS" {
        return reject("per-user updates never request administrative install mode");
    }
    if upper == "/LOG" || upper.starts_with("/LOG=") {
        return reject("set the log file with InnoSetup::with_log_file");
    }
    Ok(switch)
}

/// Quotes `path` for an installer switch value: `"C:\Path"`.
///
/// Verbatim prefixes (`\\?\`, `\\?\UNC\`) produced by canonicalization are
/// removed, because installers expect ordinary paths, and a trailing
/// separator is dropped (except after a drive root) so it cannot be read as
/// escaping the closing quote.
pub(crate) fn quoted_path(path: &Path) -> Result<String, UpdateError> {
    let invalid = |reason: &str| {
        UpdateError::new(ErrorKind::Configuration).with_diagnostic(format!(
            "installer path {} cannot be passed on a command line: {reason}",
            path.display()
        ))
    };
    let text = path.to_str().ok_or_else(|| invalid("not valid Unicode"))?;
    let text = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(text).to_owned()
    };
    let mut text = text.as_str();
    while text.len() > 1
        && (text.ends_with('\\') || text.ends_with('/'))
        && !text[..text.len() - 1].ends_with(':')
    {
        text = &text[..text.len() - 1];
    }
    if text.is_empty() {
        return Err(invalid("empty path"));
    }
    if text.chars().any(|c| c == '"' || c.is_control()) {
        return Err(invalid("contains a quote or control character"));
    }
    Ok(format!("\"{text}\""))
}

/// How updates are applied to this installation; declared by the
/// application, never inferred from an artifact.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum UpdateStrategy {
    /// Run a verified installer over the running installation.
    Installer(Arc<dyn InstallerStrategy>),
    /// Replace the application's single executable in place.
    Portable(PortableExecutable),
}

impl UpdateStrategy {
    /// Hand off to an Inno Setup installer.
    pub fn inno_setup(inno: InnoSetup) -> Self {
        Self::Installer(Arc::new(inno))
    }

    /// Hand off to a custom installer technology.
    pub fn installer(strategy: impl InstallerStrategy) -> Self {
        Self::Installer(Arc::new(strategy))
    }

    /// Replace a portable single-executable application in place.
    pub fn portable(portable: PortableExecutable) -> Self {
        Self::Portable(portable)
    }

    pub(crate) fn staged_file_name(&self) -> &str {
        match self {
            Self::Installer(installer) => installer.staged_file_name(),
            Self::Portable(_) => crate::portable::STAGED_FILE_NAME,
        }
    }
}

/// A portable application that is a single executable (plus files it does
/// not update), whose artifact is the new executable itself.
///
/// The verified executable must declare the feed version in its
/// `ProductVersion` version resource string and be built for the configured
/// architecture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortableExecutable {
    pub(crate) version_key: String,
}

impl Default for PortableExecutable {
    fn default() -> Self {
        Self::new()
    }
}

impl PortableExecutable {
    /// A portable executable whose version is confirmed from
    /// `ProductVersion`.
    pub fn new() -> Self {
        Self {
            version_key: DEFAULT_VERSION_KEY.to_owned(),
        }
    }

    /// Uses version resource string `key` instead of `ProductVersion`.
    pub fn with_version_key(mut self, key: impl Into<String>) -> Self {
        self.version_key = key.into();
        self
    }
}

//! Managed-install detection.
//!
//! Detection is a pure function of [`DetectionInputs`] plus the filesystem it
//! names, so it can be exercised against temporary roots on any Unix host.
//! The marker contract is documented in `docs/linux-managed-install.md`.

use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::{fmt, fs, io};

use gpui_auto_update_core::Capability;

/// Everything detection needs to know about the running process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectionInputs {
    /// The application's executable name, which is also the marker's `app`.
    pub app_name: String,
    /// The architecture the running binary was built for, as reported by
    /// [`std::env::consts::ARCH`].
    pub arch: String,
    /// The effective user id of the running process.
    pub euid: u32,
    /// The path of the running executable. It is canonicalized by detection.
    pub executable: PathBuf,
    /// The user's home directory, if known.
    pub home: Option<PathBuf>,
    /// The filesystem root that system paths such as `/usr` are resolved
    /// against. This is `/` outside tests.
    pub root: PathBuf,
}

/// A self-managed installation that the updater owns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedInstall {
    app_name: String,
    prefix: PathBuf,
    executable: PathBuf,
}

impl ManagedInstall {
    /// The application's executable name.
    pub fn app_name(&self) -> &str {
        &self.app_name
    }

    /// The canonical installation prefix, the directory that is replaced as
    /// a whole during an update.
    pub fn prefix(&self) -> &Path {
        &self.prefix
    }

    /// The canonical path of `<prefix>/bin/<app>`.
    pub fn executable(&self) -> &Path {
        &self.executable
    }
}

/// Why detection reached its capability.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DetectionReason {
    /// The installation is a valid managed install.
    ManagedInstall,
    /// The binary was built for an architecture without Linux release feeds.
    UnsupportedArchitecture(String),
    /// The configured application name cannot name a managed install.
    InvalidAppName,
    /// The process runs as root; root sessions never self-update.
    RootSession,
    /// A package manager (named by the capability) owns the installation.
    PackageManager,
    /// The installation lives in a system-wide location such as `/usr`.
    SystemInstall,
    /// The user's home directory is unknown or cannot be resolved.
    HomeUnresolvable,
    /// The prefix is not strictly inside the user's home directory.
    OutsideHome,
    /// The running executable could not be resolved to a canonical path.
    ExecutableUnresolvable,
    /// The executable is not `<prefix>/bin/<app>`, as with a source build.
    UnknownLayout,
    /// The prefix carries no managed-install marker.
    MissingMarker,
    /// The marker exists but is not a small regular file with the exact
    /// expected contents.
    InvalidMarker,
    /// The prefix, its `bin` directory, the executable, or the marker is not
    /// owned by the effective user.
    NotUserOwned,
    /// The prefix's parent directory cannot be written, so the prefix cannot
    /// be swapped (for example a read-only mount).
    ParentNotWritable,
}

impl fmt::Display for DetectionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ManagedInstall => "managed user-local installation",
            Self::UnsupportedArchitecture(arch) => {
                return write!(f, "the {arch:?} architecture is not supported");
            }
            Self::InvalidAppName => "the application name is not a valid install name",
            Self::RootSession => "root sessions do not update themselves",
            Self::PackageManager => "a package manager owns this installation",
            Self::SystemInstall => "system-wide installations are not updated by the application",
            Self::HomeUnresolvable => "the home directory could not be resolved",
            Self::OutsideHome => "the installation is not inside the user's home directory",
            Self::ExecutableUnresolvable => "the running executable could not be resolved",
            Self::UnknownLayout => "the executable is not part of a managed installation layout",
            Self::MissingMarker => "the installation has no managed-install marker",
            Self::InvalidMarker => "the managed-install marker is invalid",
            Self::NotUserOwned => "the installation is not owned by the current user",
            Self::ParentNotWritable => "the installation's parent directory is not writable",
        })
    }
}

/// The result of managed-install detection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detection {
    capability: Capability,
    reason: DetectionReason,
    install: Option<ManagedInstall>,
}

impl Detection {
    /// The capability to report for this installation.
    pub fn capability(&self) -> &Capability {
        &self.capability
    }

    /// Why this capability was chosen, for diagnostics.
    pub fn reason(&self) -> &DetectionReason {
        &self.reason
    }

    /// The managed install, present only when the capability is
    /// [`Capability::SelfManaged`].
    pub fn install(&self) -> Option<&ManagedInstall> {
        self.install.as_ref()
    }
}

impl Detection {
    fn denied(capability: Capability, reason: DetectionReason) -> Self {
        Self {
            capability,
            reason,
            install: None,
        }
    }

    fn unsupported(reason: DetectionReason) -> Self {
        Self::denied(Capability::Unsupported, reason)
    }
}

/// The marker's path relative to the prefix is `share/<app>/MARKER_FILE_NAME`.
pub const MARKER_FILE_NAME: &str = "gpui-auto-update.managed";

/// The first line of a version 1 marker.
const MARKER_HEADER: &str = "gpui-auto-update managed-install 1";

/// Markers are tiny; anything larger is rejected without reading it whole.
const MARKER_MAX_LEN: u64 = 4096;

/// The exact marker contents a managed install of `app_name` must carry.
pub fn marker_contents(app_name: &str) -> String {
    format!("{MARKER_HEADER}\napp={app_name}\n")
}

/// Decides whether the described installation may update itself.
pub fn detect(inputs: &DetectionInputs) -> Detection {
    if !SUPPORTED_ARCHES.contains(&inputs.arch.as_str()) {
        return Detection::unsupported(DetectionReason::UnsupportedArchitecture(
            inputs.arch.clone(),
        ));
    }
    if !is_valid_app_name(&inputs.app_name) {
        return Detection::unsupported(DetectionReason::InvalidAppName);
    }
    if inputs.euid == 0 {
        return Detection::unsupported(DetectionReason::RootSession);
    }
    let root = inputs
        .root
        .canonicalize()
        .unwrap_or_else(|_| inputs.root.clone());
    if root.join(".flatpak-info").exists() {
        return Detection::denied(managed_by("Flatpak"), DetectionReason::PackageManager);
    }
    let Ok(executable) = inputs.executable.canonicalize() else {
        return Detection::unsupported(DetectionReason::ExecutableUnresolvable);
    };
    if let Ok(relative) = executable.strip_prefix(&root) {
        if let Some(manager) = package_manager(relative) {
            return Detection::denied(managed_by(manager), DetectionReason::PackageManager);
        }
        if is_system_path(relative) {
            return Detection::denied(
                Capability::ExternallyManaged { manager: None },
                DetectionReason::SystemInstall,
            );
        }
    }
    let Some(prefix) = layout_prefix(&executable, &inputs.app_name) else {
        return Detection::unsupported(DetectionReason::UnknownLayout);
    };
    let Some(home) = inputs.home.as_deref().and_then(|h| h.canonicalize().ok()) else {
        return Detection::unsupported(DetectionReason::HomeUnresolvable);
    };
    if prefix == home || !prefix.starts_with(&home) {
        return Detection::unsupported(DetectionReason::OutsideHome);
    }
    let marker = prefix
        .join("share")
        .join(&inputs.app_name)
        .join(MARKER_FILE_NAME);
    match read_marker(&marker) {
        MarkerRead::Missing => return Detection::unsupported(DetectionReason::MissingMarker),
        MarkerRead::Invalid => return Detection::unsupported(DetectionReason::InvalidMarker),
        MarkerRead::Contents(contents) => {
            if contents != marker_contents(&inputs.app_name).as_bytes() {
                return Detection::unsupported(DetectionReason::InvalidMarker);
            }
        }
    }
    let Ok(exe_meta) = fs::metadata(&executable) else {
        return Detection::unsupported(DetectionReason::ExecutableUnresolvable);
    };
    if !exe_meta.is_file() || exe_meta.mode() & 0o111 == 0 {
        return Detection::unsupported(DetectionReason::UnknownLayout);
    }
    let owned = [prefix.as_path(), &prefix.join("bin"), &executable, &marker]
        .iter()
        .all(|path| fs::symlink_metadata(path).is_ok_and(|meta| meta.uid() == inputs.euid));
    if !owned {
        return Detection::unsupported(DetectionReason::NotUserOwned);
    }
    if !parent_is_writable(&prefix) {
        return Detection::denied(
            Capability::TemporarilyUnavailable,
            DetectionReason::ParentNotWritable,
        );
    }
    Detection {
        capability: Capability::SelfManaged,
        reason: DetectionReason::ManagedInstall,
        install: Some(ManagedInstall {
            app_name: inputs.app_name.clone(),
            prefix,
            executable,
        }),
    }
}

/// Architectures with Linux release feeds.
const SUPPORTED_ARCHES: &[&str] = &["x86_64", "aarch64"];

fn is_valid_app_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.starts_with(['.', '-'])
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn managed_by(manager: &str) -> Capability {
    Capability::ExternallyManaged {
        manager: Some(manager.to_owned()),
    }
}

/// Recognizes package-manager stores from a root-relative executable path.
fn package_manager(relative: &Path) -> Option<&'static str> {
    let parts: Vec<&std::ffi::OsStr> = relative.iter().collect();
    match parts.as_slice() {
        [first, second, ..] if *first == "nix" && *second == "store" => return Some("Nix"),
        [first, second, ..] if *first == "gnu" && *second == "store" => return Some("Guix"),
        [first, ..] if *first == "snap" => return Some("Snap"),
        [first, second, ..] if *first == "home" && *second == "linuxbrew" => {
            return Some("Homebrew");
        }
        _ => {}
    }
    if parts.iter().any(|part| *part == ".linuxbrew") {
        return Some("Homebrew");
    }
    if parts.windows(2).any(|w| w[0] == "flatpak" && w[1] == "app") {
        return Some("Flatpak");
    }
    None
}

/// System-wide locations that this updater never treats as user-owned.
fn is_system_path(relative: &Path) -> bool {
    const SYSTEM_TOP_LEVEL: &[&str] = &["usr", "opt", "app", "bin", "sbin", "lib", "lib64"];
    relative
        .iter()
        .next()
        .is_some_and(|first| SYSTEM_TOP_LEVEL.iter().any(|dir| first == *dir))
}

/// Returns `<prefix>` when `executable` is exactly `<prefix>/bin/<app_name>`.
fn layout_prefix(executable: &Path, app_name: &str) -> Option<PathBuf> {
    if executable.file_name()? != app_name {
        return None;
    }
    let bin = executable.parent()?;
    if bin.file_name()? != "bin" {
        return None;
    }
    Some(bin.parent()?.to_path_buf())
}

/// Proves the prefix can be replaced by creating and removing a uniquely
/// named sibling directory, which exercises the same permission (and
/// read-only filesystem) checks as the rename-based swap.
fn parent_is_writable(prefix: &Path) -> bool {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let (Some(parent), Some(name)) = (prefix.parent(), prefix.file_name()) else {
        return false;
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    for _ in 0..8 {
        let probe = parent.join(format!(
            ".{}.gpui-auto-update-probe-{}-{nanos}-{}",
            name.to_string_lossy(),
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        match fs::create_dir(&probe) {
            Ok(()) => return fs::remove_dir(&probe).is_ok(),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return false,
        }
    }
    false
}

enum MarkerRead {
    Missing,
    Invalid,
    Contents(Vec<u8>),
}

fn read_marker(path: &Path) -> MarkerRead {
    use std::io::Read as _;

    let link_meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return MarkerRead::Missing,
        Err(_) => return MarkerRead::Invalid,
    };
    if !link_meta.file_type().is_file() || link_meta.len() > MARKER_MAX_LEN {
        return MarkerRead::Invalid;
    }
    let Ok(file) = fs::File::open(path) else {
        return MarkerRead::Invalid;
    };
    // Reject a marker that was swapped (for example for a symlink) between
    // the `lstat` above and the open.
    match file.metadata() {
        Ok(meta) if meta.dev() == link_meta.dev() && meta.ino() == link_meta.ino() => {}
        _ => return MarkerRead::Invalid,
    }
    let mut contents = Vec::new();
    match file.take(MARKER_MAX_LEN + 1).read_to_end(&mut contents) {
        Ok(_) if contents.len() as u64 <= MARKER_MAX_LEN => MarkerRead::Contents(contents),
        _ => MarkerRead::Invalid,
    }
}

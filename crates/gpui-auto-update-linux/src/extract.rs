//! Extracting a verified release tarball into a sibling staging install.
//!
//! The input is always a [`StagedArtifact`], which only the core's
//! downloader creates after the Ed25519 signature has verified, so nothing
//! unverified is ever decompressed. The archive contract is documented in
//! `docs/linux-managed-install.md`.

use std::cell::Cell;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Write as _};
use std::os::unix::fs::{
    DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt,
};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use gpui_auto_update_core::download::StagedArtifact;
use gpui_auto_update_core::feed::{Arch, FeedLimits};
use gpui_auto_update_core::version::ReleaseVersion;
use gpui_auto_update_core::{ErrorKind, UpdateError};

use crate::detect::{
    MARKER_FILE_NAME, ManagedInstall, MarkerRead, is_valid_app_name, marker_contents, read_marker,
};

const DIR_MODE: u32 = 0o755;
const EXECUTABLE_MODE: u32 = 0o755;
const FILE_MODE: u32 = 0o644;
const COPY_CHUNK: usize = 64 * 1024;

/// Upper bounds applied while extracting a release archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveLimits {
    /// Largest accepted compressed archive, in bytes.
    pub max_compressed_bytes: u64,
    /// Largest accepted decompressed tar stream, in bytes. This bounds the
    /// sum of all entry sizes and also tar headers and metadata records.
    pub max_expanded_bytes: u64,
    /// Largest accepted number of archive entries (files and directories).
    pub max_entries: u64,
}

impl Default for ArchiveLimits {
    /// The core's artifact size limit (512 MiB compressed), 1 GiB expanded,
    /// and 100 000 entries.
    fn default() -> Self {
        Self {
            max_compressed_bytes: FeedLimits::default().max_artifact_bytes,
            max_expanded_bytes: 1024 * 1024 * 1024,
            max_entries: 100_000,
        }
    }
}

/// Why a verified artifact could not be staged as a new installation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StageError {
    /// The compressed archive is larger than
    /// [`ArchiveLimits::max_compressed_bytes`]; it was not read.
    #[error("archive is {length} bytes; at most {limit} are allowed")]
    ArchiveTooLarge {
        /// Size of the verified artifact.
        length: u64,
        /// The configured limit.
        limit: u64,
    },
    /// The decompressed archive, or the sizes its entries declare, exceed
    /// [`ArchiveLimits::max_expanded_bytes`].
    #[error("archive expands to more than {limit} bytes")]
    ExpandedTooLarge {
        /// The configured limit.
        limit: u64,
    },
    /// The archive has more than [`ArchiveLimits::max_entries`] entries.
    #[error("archive has more than {limit} entries")]
    TooManyEntries {
        /// The configured limit.
        limit: u64,
    },
    /// An entry path contains a `..` component.
    #[error("archive entry {path:?} traverses outside the release")]
    PathTraversal {
        /// The entry path as stored in the archive.
        path: String,
    },
    /// An entry path is absolute.
    #[error("archive entry {path:?} is an absolute path")]
    AbsolutePath {
        /// The entry path as stored in the archive.
        path: String,
    },
    /// An entry path is empty, not UTF-8, contains control characters, or
    /// starts with a `.` component.
    #[error("archive entry {path:?} is not a plain relative path")]
    UnsafePath {
        /// The entry path as stored in the archive (lossily decoded).
        path: String,
    },
    /// An entry is outside the single expected top-level directory, or the
    /// top-level entry is not a directory.
    #[error(
        "archive entry is under {found:?}; everything must be inside the {expected:?} directory"
    )]
    UnexpectedRoot {
        /// The required top-level directory name.
        expected: String,
        /// The top-level name found in the archive.
        found: String,
    },
    /// The top-level directory is named for a release, but its version part
    /// is not a valid release version.
    #[error("archive top-level directory {found:?} does not carry a valid release version")]
    InvalidVersionPath {
        /// The top-level name found in the archive.
        found: String,
    },
    /// The archive contains a different release than the feed entry it was
    /// downloaded for (feed metadata is not signed; the archive is).
    #[error("archive contains version {found}, but the feed offered {expected}")]
    VersionMismatch {
        /// The feed entry's `sparkle:version`.
        expected: String,
        /// The (valid) version named by the archive's top-level directory.
        found: String,
    },
    /// The archive is built for a different architecture.
    #[error("archive is built for {found}, but this installation is {expected}")]
    ArchMismatch {
        /// The installation's architecture.
        expected: Arch,
        /// The architecture named by the archive's top-level directory.
        found: Arch,
    },
    /// An entry is a symbolic or hard link.
    #[error("archive entry {path:?} is a link")]
    Link {
        /// The entry path as stored in the archive.
        path: String,
    },
    /// An entry is neither a regular file nor a directory (for example a
    /// device, FIFO, sparse, or unknown entry type).
    #[error("archive entry {path:?} is not a regular file or directory")]
    SpecialFile {
        /// The entry path as stored in the archive.
        path: String,
    },
    /// Two entries resolve to the same output path.
    #[error("archive contains {path:?} more than once")]
    DuplicatePath {
        /// The entry path as stored in the archive.
        path: String,
    },
    /// The gzip or tar data is corrupt or truncated.
    #[error("archive is malformed: {0}")]
    Malformed(#[source] io::Error),
    /// The extracted release is not a valid managed-install layout.
    #[error("extracted release is not a valid managed install: {0}")]
    InvalidLayout(#[from] LayoutError),
    /// The staged artifact file no longer matches the verified artifact.
    #[error("the staged artifact changed after verification")]
    ArtifactChanged,
    /// The staging installation could not be created or written.
    #[error("could not write the staging installation: {0}")]
    Io(#[source] io::Error),
}

impl From<StageError> for UpdateError {
    fn from(error: StageError) -> Self {
        let kind = match &error {
            StageError::ArtifactChanged | StageError::Io(_) => ErrorKind::Staging,
            _ => ErrorKind::ArchiveValidation,
        };
        UpdateError::new(kind)
            .with_diagnostic(error.to_string())
            .with_source(error)
    }
}

/// Why a directory is not a valid managed-install layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LayoutError {
    /// The application name cannot name a managed install.
    #[error("the application name is not a valid install name")]
    InvalidAppName,
    /// The prefix is missing or not a directory.
    #[error("the installation directory is missing")]
    MissingPrefix,
    /// `bin/<app>` is missing or not a regular file.
    #[error("bin/<app> is missing or not a regular file")]
    MissingExecutable,
    /// `bin/<app>` has no execute permission.
    #[error("bin/<app> is not executable")]
    NotExecutable,
    /// The managed-install marker is missing.
    #[error("the managed-install marker is missing")]
    MissingMarker,
    /// The marker is not a small regular file with the exact contents.
    #[error("the managed-install marker is invalid")]
    InvalidMarker,
}

/// Checks that `prefix` is a complete managed-install layout for
/// `app_name`: a regular, executable `bin/<app>` and the exact marker at
/// `share/<app>/gpui-auto-update.managed`, with no symbolic links on those
/// paths.
pub fn validate_layout(prefix: &Path, app_name: &str) -> Result<(), LayoutError> {
    if !is_valid_app_name(app_name) {
        return Err(LayoutError::InvalidAppName);
    }
    let is_dir = |path: &Path| fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir());
    if !is_dir(prefix) {
        return Err(LayoutError::MissingPrefix);
    }
    let bin = prefix.join("bin");
    let executable = match fs::symlink_metadata(bin.join(app_name)) {
        Ok(meta) if is_dir(&bin) && meta.is_file() => meta,
        _ => return Err(LayoutError::MissingExecutable),
    };
    if executable.mode() & 0o111 == 0 {
        return Err(LayoutError::NotExecutable);
    }
    let share = prefix.join("share");
    let app_share = share.join(app_name);
    if !is_dir(&share) || !is_dir(&app_share) {
        return Err(LayoutError::MissingMarker);
    }
    match read_marker(&app_share.join(MARKER_FILE_NAME)) {
        MarkerRead::Missing => Err(LayoutError::MissingMarker),
        MarkerRead::Invalid => Err(LayoutError::InvalidMarker),
        MarkerRead::Contents(contents) if contents == marker_contents(app_name).as_bytes() => {
            Ok(())
        }
        MarkerRead::Contents(_) => Err(LayoutError::InvalidMarker),
    }
}

/// A validated new installation, extracted next to the running one.
///
/// Dropping it leaves the files in place for the helper; call
/// [`Self::discard`] to remove them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedRelease {
    directory: PathBuf,
    prefix: PathBuf,
    version: ReleaseVersion,
}

impl StagedRelease {
    /// The staged installation prefix, `<directory>/<app>-<version>-linux-<arch>`,
    /// which has passed [`validate_layout`] and replaces the running prefix
    /// as a whole.
    pub fn prefix(&self) -> &Path {
        &self.prefix
    }

    /// The private staging directory that contains [`Self::prefix`]. It is a
    /// sibling of the running prefix, so both are on the same filesystem.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The release version, confirmed against the archive contents.
    pub fn version(&self) -> &ReleaseVersion {
        &self.version
    }

    /// Removes the staging directory and everything in it.
    pub fn discard(self) -> io::Result<()> {
        fs::remove_dir_all(&self.directory)
    }
}

/// Extracts verified release archives into sibling staging installations.
#[derive(Debug, Clone)]
pub struct ReleaseStager {
    arch: Arch,
    limits: ArchiveLimits,
}

impl ReleaseStager {
    /// A stager for installations built for `arch` (normally
    /// [`Arch::current`]) with the default [`ArchiveLimits`].
    pub fn new(arch: Arch) -> Self {
        Self {
            arch,
            limits: ArchiveLimits::default(),
        }
    }

    /// Replaces the archive limits.
    pub fn with_limits(mut self, limits: ArchiveLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Extracts `artifact` into a new private directory next to
    /// `install`'s prefix and validates the result.
    ///
    /// The running installation is never written. The archive must contain
    /// exactly one top-level directory named
    /// `<app>-<version>-linux-<arch>`, where `<version>` is the artifact's
    /// [`StagedArtifact::expected_version`] text and `<arch>` this stager's
    /// architecture, so the signed archive itself confirms the release the
    /// unsigned feed entry claimed. On success the staged prefix has passed
    /// [`validate_layout`], so it is safe to ask the application to quit. On
    /// any failure the staging directory is removed.
    ///
    /// The call blocks; run it on a background executor.
    pub fn stage(
        &self,
        install: &ManagedInstall,
        artifact: &StagedArtifact,
    ) -> Result<StagedRelease, StageError> {
        let length = artifact.length();
        if length > self.limits.max_compressed_bytes {
            return Err(StageError::ArchiveTooLarge {
                length,
                limit: self.limits.max_compressed_bytes,
            });
        }
        let file = open_verified(artifact.path(), length)?;

        let version = artifact.expected_version();
        let root = RootSpec {
            app: install.app_name(),
            version,
            arch: self.arch,
            name: format!(
                "{}-{}-linux-{}",
                install.app_name(),
                version.path_component(),
                self.arch.as_str()
            ),
        };
        let prefix = install.prefix();
        let (Some(parent), Some(prefix_name)) = (prefix.parent(), prefix.file_name()) else {
            return Err(StageError::Io(io::Error::other(
                "the installation prefix has no parent directory",
            )));
        };
        let directory = tempfile::Builder::new()
            .prefix(&format!(
                ".{}.gpui-auto-update-staged-{}-",
                prefix_name.to_string_lossy(),
                version.path_component()
            ))
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)
            .map_err(StageError::Io)?;

        let exceeded = Rc::new(Cell::new(false));
        let stream = LimitedReader {
            inner: flate2::read::GzDecoder::new(BufReader::new(file.take(length))),
            remaining: self.limits.max_expanded_bytes,
            exceeded: Rc::clone(&exceeded),
        };
        let mut archive = tar::Archive::new(stream);
        if let Err(error) = extract(&mut archive, directory.path(), &root, &self.limits) {
            // Whatever the tar reader made of the cut-off stream, the cause
            // is the expanded size limit.
            return Err(if exceeded.get() {
                StageError::ExpandedTooLarge {
                    limit: self.limits.max_expanded_bytes,
                }
            } else {
                error
            });
        }

        let staged_prefix = directory.path().join(&root.name);
        validate_layout(&staged_prefix, install.app_name())?;
        Ok(StagedRelease {
            directory: directory.keep(),
            prefix: staged_prefix,
            version: version.clone(),
        })
    }
}

/// Opens the staged artifact, refusing anything that is not the regular
/// file of the verified length.
fn open_verified(path: &Path, length: u64) -> Result<File, StageError> {
    let link_meta = fs::symlink_metadata(path).map_err(StageError::Io)?;
    if !link_meta.is_file() {
        return Err(StageError::ArtifactChanged);
    }
    let file = File::open(path).map_err(StageError::Io)?;
    let meta = file.metadata().map_err(StageError::Io)?;
    if meta.dev() != link_meta.dev() || meta.ino() != link_meta.ino() || meta.len() != length {
        return Err(StageError::ArtifactChanged);
    }
    Ok(file)
}

/// The single top-level directory an archive must use.
struct RootSpec<'a> {
    app: &'a str,
    version: &'a ReleaseVersion,
    arch: Arch,
    name: String,
}

impl RootSpec<'_> {
    /// Explains why `found` is not the expected top-level name.
    fn mismatch(&self, found: &str) -> StageError {
        let unexpected = || StageError::UnexpectedRoot {
            expected: self.name.clone(),
            found: found.to_owned(),
        };
        let Some(rest) = found
            .strip_prefix(self.app)
            .and_then(|rest| rest.strip_prefix('-'))
        else {
            return unexpected();
        };
        let Some((version, arch)) = rest.rsplit_once("-linux-") else {
            return unexpected();
        };
        let Some(arch) = Arch::parse(arch) else {
            return unexpected();
        };
        let Ok(version) = ReleaseVersion::parse(version) else {
            return StageError::InvalidVersionPath {
                found: found.to_owned(),
            };
        };
        if arch != self.arch {
            StageError::ArchMismatch {
                expected: self.arch,
                found: arch,
            }
        } else if version.as_str() != self.version.as_str() {
            StageError::VersionMismatch {
                expected: self.version.as_str().to_owned(),
                found: version.as_str().to_owned(),
            }
        } else {
            unexpected()
        }
    }
}

fn extract<R: Read>(
    archive: &mut tar::Archive<R>,
    directory: &Path,
    root: &RootSpec<'_>,
    limits: &ArchiveLimits,
) -> Result<(), StageError> {
    let mut seen = HashSet::new();
    let mut count: u64 = 0;
    let mut declared: u64 = 0;
    for entry in archive.entries().map_err(StageError::Malformed)? {
        let mut entry = entry.map_err(StageError::Malformed)?;
        count += 1;
        if count > limits.max_entries {
            return Err(StageError::TooManyEntries {
                limit: limits.max_entries,
            });
        }
        let raw = entry.path_bytes().into_owned();
        let relative = checked_path(&raw, root)?;
        let display = || String::from_utf8_lossy(&raw).into_owned();

        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() {
            return Err(StageError::Link { path: display() });
        }
        if !kind.is_file() && !kind.is_dir() {
            return Err(StageError::SpecialFile { path: display() });
        }
        if relative.components().count() == 1 && !kind.is_dir() {
            return Err(StageError::UnexpectedRoot {
                expected: root.name.clone(),
                found: display(),
            });
        }
        if !seen.insert(relative.clone()) {
            return Err(StageError::DuplicatePath { path: display() });
        }
        declared = declared.saturating_add(entry.size());
        if declared > limits.max_expanded_bytes {
            return Err(StageError::ExpandedTooLarge {
                limit: limits.max_expanded_bytes,
            });
        }

        let output = directory.join(&relative);
        if kind.is_dir() {
            create_dirs(&output)?;
        } else {
            let mode = if entry.header().mode().map_err(StageError::Malformed)? & 0o111 != 0 {
                EXECUTABLE_MODE
            } else {
                FILE_MODE
            };
            write_file(&mut entry, &output, mode)?;
        }
    }
    Ok(())
}

/// Validates an entry path and returns it normalized (empty and `.`
/// components between separators removed), starting with the root.
fn checked_path(raw: &[u8], root: &RootSpec<'_>) -> Result<PathBuf, StageError> {
    let lossy = || String::from_utf8_lossy(raw).into_owned();
    let Ok(text) = std::str::from_utf8(raw) else {
        return Err(StageError::UnsafePath { path: lossy() });
    };
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err(StageError::UnsafePath { path: lossy() });
    }
    let mut normalized = PathBuf::new();
    for component in Path::new(text).components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => return Err(StageError::PathTraversal { path: lossy() }),
            Component::RootDir | Component::Prefix(_) => {
                return Err(StageError::AbsolutePath { path: lossy() });
            }
            Component::CurDir => return Err(StageError::UnsafePath { path: lossy() }),
        }
    }
    match normalized.components().next() {
        Some(Component::Normal(first)) if first == root.name.as_str() => Ok(normalized),
        Some(Component::Normal(first)) => Err(root.mismatch(&first.to_string_lossy())),
        _ => Err(StageError::UnsafePath { path: lossy() }),
    }
}

fn create_dirs(path: &Path) -> Result<(), StageError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(path)
        .and_then(|()| fs::set_permissions(path, fs::Permissions::from_mode(DIR_MODE)))
        .map_err(StageError::Io)
}

fn write_file(entry: &mut impl Read, output: &Path, mode: u32) -> Result<(), StageError> {
    if let Some(parent) = output.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(DIR_MODE)
            .create(parent)
            .map_err(StageError::Io)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(output)
        .map_err(StageError::Io)?;
    let mut buf = vec![0u8; COPY_CHUNK];
    loop {
        let n = match entry.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(StageError::Malformed(e)),
        };
        file.write_all(&buf[..n]).map_err(StageError::Io)?;
    }
    // The umask may have narrowed the creation mode.
    file.set_permissions(fs::Permissions::from_mode(mode))
        .and_then(|()| file.sync_all())
        .map_err(StageError::Io)
}

/// Fails once more than `remaining` bytes would be read, recording that the
/// limit (rather than corrupt data) caused the failure.
struct LimitedReader<R> {
    inner: R,
    remaining: u64,
    exceeded: Rc<Cell<bool>>,
}

impl<R: Read> Read for LimitedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut probe = [0u8; 1];
            return match self.inner.read(&mut probe)? {
                0 => Ok(0),
                _ => {
                    self.exceeded.set(true);
                    Err(io::Error::other("archive exceeds the expanded size limit"))
                }
            };
        }
        let max = buf
            .len()
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..max])?;
        self.remaining -= n as u64;
        Ok(n)
    }
}

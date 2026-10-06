//! Extracting verified release tarballs into a sibling staging install.
//!
//! Every archive is built in the test, signed with a disposable key, served
//! from loopback, and downloaded through the core's verifying downloader, so
//! the only way to reach extraction is with a verified `StagedArtifact`.
#![cfg(unix)]

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signer, SigningKey};
use gpui_auto_update_core::ErrorKind;
use gpui_auto_update_core::UpdateError;
use gpui_auto_update_core::download::{ArtifactDownloader, StagedArtifact};
use gpui_auto_update_core::feed::{Arch, Feed, FeedLimits, Os, Selection, UpdateTarget};
use gpui_auto_update_core::fetch::{FetchPolicy, HttpClient};
use gpui_auto_update_core::trust::TrustedKey;
use gpui_auto_update_core::version::ReleaseVersion;
use gpui_auto_update_linux::{
    ArchiveLimits, DetectionInputs, LayoutError, ManagedInstall, ReleaseStager, StageError, detect,
    marker_contents, validate_layout,
};

const APP: &str = "demo";
const ROOT: &str = "demo-1.5.0-linux-x86_64";

// ---------------------------------------------------------------------------
// Archive construction

/// Builds gzip-compressed tarballs, including malformed and hostile ones.
struct Tarball {
    builder: tar::Builder<Vec<u8>>,
    /// The header being assembled by `raw_header`.
    last: Option<tar::Header>,
}

impl Tarball {
    fn new() -> Self {
        Self {
            builder: tar::Builder::new(Vec::new()),
            last: None,
        }
    }

    /// A valid release for `root`: executable, marker, and a data file.
    fn release(root: &str) -> Self {
        Self::new()
            .dir(root)
            .dir(&format!("{root}/bin"))
            .file(
                &format!("{root}/bin/{APP}"),
                b"#!/bin/sh\necho new\n",
                0o755,
            )
            .dir(&format!("{root}/share"))
            .dir(&format!("{root}/share/{APP}"))
            .file(
                &format!("{root}/share/{APP}/gpui-auto-update.managed"),
                marker_contents(APP).as_bytes(),
                0o644,
            )
            .file(&format!("{root}/share/{APP}/data.txt"), b"payload", 0o644)
    }

    /// Appends an entry whose name is written verbatim into the header, so
    /// names the `tar` crate would refuse to create can be tested.
    fn raw(mut self, name: &str, kind: tar::EntryType, data: &[u8], mode: u32) -> Self {
        self.raw_header(name, kind, data.len() as u64, mode, None);
        self.builder
            .append(&self.last.take().unwrap(), data)
            .unwrap();
        self
    }

    fn raw_header(
        &mut self,
        name: &str,
        kind: tar::EntryType,
        size: u64,
        mode: u32,
        link: Option<&str>,
    ) {
        let mut header = tar::Header::new_gnu();
        let bytes = name.as_bytes();
        let field = &mut header.as_old_mut().name;
        assert!(bytes.len() <= field.len(), "raw names must fit the header");
        field[..bytes.len()].copy_from_slice(bytes);
        if let Some(link) = link {
            let field = &mut header.as_old_mut().linkname;
            field[..link.len()].copy_from_slice(link.as_bytes());
        }
        header.set_entry_type(kind);
        if kind == tar::EntryType::GNUSparse {
            // An empty, well-formed sparse map, so the tar reader yields the
            // entry instead of failing on the header.
            header.as_gnu_mut().unwrap().set_real_size(size);
        }
        header.set_size(size);
        header.set_mode(mode);
        header.set_cksum();
        self.last = Some(header);
    }

    fn dir(self, name: &str) -> Self {
        self.raw(name, tar::EntryType::Directory, b"", 0o755)
    }

    fn file(self, name: &str, data: &[u8], mode: u32) -> Self {
        self.raw(name, tar::EntryType::Regular, data, mode)
    }

    /// A regular file appended through the `tar` crate's own path handling,
    /// which emits GNU long-name records for long names.
    fn long_file(mut self, name: &str, data: &[u8]) -> Self {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        self.builder.append_data(&mut header, name, data).unwrap();
        self
    }

    fn link(mut self, name: &str, kind: tar::EntryType, target: &str) -> Self {
        self.raw_header(name, kind, 0, 0o777, Some(target));
        self.builder
            .append(&self.last.take().unwrap(), &b""[..])
            .unwrap();
        self
    }

    /// A header that claims `size` bytes but is followed by none.
    fn declared(mut self, name: &str, size: u64) -> Self {
        self.raw_header(name, tar::EntryType::Regular, size, 0o644, None);
        let header = self.last.take().unwrap();
        self.builder.get_mut().extend_from_slice(header.as_bytes());
        self
    }

    fn gz(self) -> Vec<u8> {
        let tar = self.builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }
}

// ---------------------------------------------------------------------------
// Installation and download fixtures

/// A managed install of `demo` 1.0.0 inside a temporary home directory.
struct Install {
    _dir: tempfile::TempDir,
    home: PathBuf,
    install: ManagedInstall,
}

impl Install {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home/alice");
        let prefix = home.join(".local/opt/demo");
        fs::create_dir_all(prefix.join("bin")).unwrap();
        let exe = prefix.join("bin").join(APP);
        fs::write(&exe, b"#!/bin/sh\necho old\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        fs::create_dir_all(prefix.join("share").join(APP)).unwrap();
        fs::write(
            prefix
                .join("share")
                .join(APP)
                .join("gpui-auto-update.managed"),
            marker_contents(APP),
        )
        .unwrap();
        let detection = detect(&DetectionInputs {
            app_name: APP.to_owned(),
            arch: "x86_64".to_owned(),
            euid: fs::metadata(&exe).unwrap().uid(),
            executable: exe,
            home: Some(home.clone()),
            root,
        });
        let install = detection
            .install()
            .expect("fixture is a managed install")
            .clone();
        Self {
            _dir: dir,
            home,
            install,
        }
    }

    fn prefix(&self) -> &Path {
        self.install.prefix()
    }

    fn parent(&self) -> &Path {
        self.prefix().parent().unwrap()
    }

    /// Downloads `archive` as the verified artifact of release `version`.
    fn download(&self, version: &str, archive: Vec<u8>) -> StagedArtifact {
        let key = SigningKey::from_bytes(&[7; 32]);
        let signature = B64.encode(key.sign(&archive).to_bytes());
        let length = archive.len();
        let base = serve(archive);
        let xml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0"
     xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"
     xmlns:gpui-auto-update="https://github.com/thedavidweng/gpui-auto-update/xml-namespaces/feed">
  <channel>
    <item>
      <sparkle:version>{version}</sparkle:version>
      <enclosure url="{base}/artifact" length="{length}" type="application/gzip"
                 sparkle:os="linux" gpui-auto-update:arch="x86_64"
                 sparkle:edSignature="{signature}"/>
    </item>
  </channel>
</rss>
"#
        );
        let feed = Feed::parse(xml.as_bytes(), &FeedLimits::default()).unwrap();
        let current = ReleaseVersion::parse("1.0.0").unwrap();
        let item = match feed
            .select(&UpdateTarget::new(Os::Linux, Arch::X86_64), &current)
            .unwrap()
        {
            Selection::UpdateAvailable(update) => update.item,
            Selection::UpToDate => panic!("fixture release must be newer"),
        };
        let trusted = TrustedKey::from_base64(&B64.encode(key.verifying_key().to_bytes())).unwrap();
        let client = HttpClient::new(FetchPolicy {
            allow_insecure_http: true,
            timeout: Duration::from_secs(5),
            ..FetchPolicy::default()
        });
        ArtifactDownloader::new(client, trusted)
            .download(&item, &self.home.join(".cache/demo/updates"), |_| Ok(()))
            .unwrap()
    }

    /// Downloads `archive` as release 1.5.0 and stages it.
    fn stage(&self, archive: Vec<u8>) -> Result<gpui_auto_update_linux::StagedRelease, StageError> {
        self.stage_with(archive, ArchiveLimits::default())
    }

    fn stage_with(
        &self,
        archive: Vec<u8>,
        limits: ArchiveLimits,
    ) -> Result<gpui_auto_update_linux::StagedRelease, StageError> {
        let artifact = self.download("1.5.0", archive);
        ReleaseStager::new(Arch::X86_64)
            .with_limits(limits)
            .stage(&self.install, &artifact)
    }

    /// Stages `archive`, expects a rejection, and checks that the running
    /// install is untouched and no staging directory was left behind.
    fn reject(&self, archive: Vec<u8>) -> StageError {
        self.reject_with(archive, ArchiveLimits::default())
    }

    fn reject_with(&self, archive: Vec<u8>, limits: ArchiveLimits) -> StageError {
        let error = match self.stage_with(archive, limits) {
            Ok(staged) => panic!("archive was staged at {}", staged.prefix().display()),
            Err(error) => error,
        };
        self.assert_untouched();
        error
    }

    fn assert_untouched(&self) {
        assert_eq!(entries(self.parent()), vec!["demo".to_owned()]);
        assert_eq!(
            fs::read(self.prefix().join("bin").join(APP)).unwrap(),
            b"#!/bin/sh\necho old\n"
        );
        assert_eq!(validate_layout(self.prefix(), APP), Ok(()));
    }
}

/// Serves `body` at every path on a loopback port.
fn serve(body: Vec<u8>) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = server.server_addr().to_ip().unwrap();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let response =
                tiny_http::Response::from_data(body.clone()).with_chunked_threshold(usize::MAX);
            let _ = request.respond(response);
        }
    });
    format!("http://{addr}")
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o7777
}

// ---------------------------------------------------------------------------
// Valid releases

#[test]
fn verified_release_is_extracted_into_a_sibling_staging_install() {
    let fx = Install::new();

    let staged = fx.stage(Tarball::release(ROOT).gz()).unwrap();

    let prefix = staged.prefix();
    assert_eq!(prefix.file_name().unwrap(), ROOT);
    let container = prefix.parent().unwrap();
    assert_eq!(container.parent().unwrap(), fx.parent());
    assert_ne!(container, fx.prefix());
    assert!(!prefix.starts_with(fx.prefix()));
    assert_eq!(staged.version().as_str(), "1.5.0");
    assert_eq!(
        fs::read(prefix.join("bin").join(APP)).unwrap(),
        b"#!/bin/sh\necho new\n"
    );
    assert_eq!(
        fs::read(prefix.join("share/demo/data.txt")).unwrap(),
        b"payload"
    );
    assert_eq!(validate_layout(prefix, APP), Ok(()));
    // The running install is left exactly as it was.
    assert_eq!(
        fs::read(fx.prefix().join("bin").join(APP)).unwrap(),
        b"#!/bin/sh\necho old\n"
    );
}

#[test]
fn discarding_a_staged_release_removes_its_staging_directory() {
    let fx = Install::new();
    let staged = fx.stage(Tarball::release(ROOT).gz()).unwrap();

    staged.discard().unwrap();

    fx.assert_untouched();
}

#[test]
fn permissions_are_normalized_and_privilege_bits_dropped() {
    let fx = Install::new();
    let archive = Tarball::release(ROOT)
        .file(&format!("{ROOT}/bin/suid"), b"x", 0o4777)
        .file(&format!("{ROOT}/share/demo/world"), b"x", 0o666)
        .raw(
            &format!("{ROOT}/lib"),
            tar::EntryType::Directory,
            b"",
            0o2777,
        )
        .gz();

    let staged = fx.stage(archive).unwrap();

    let prefix = staged.prefix();
    assert_eq!(mode(&prefix.join("bin").join(APP)), 0o755);
    assert_eq!(mode(&prefix.join("bin/suid")), 0o755);
    assert_eq!(mode(&prefix.join("share/demo/world")), 0o644);
    assert_eq!(mode(&prefix.join("lib")), 0o755);
}

#[test]
fn parent_directories_need_not_be_listed() {
    let fx = Install::new();
    let archive = Tarball::new()
        .file(&format!("{ROOT}/bin/{APP}"), b"bin", 0o755)
        .file(
            &format!("{ROOT}/share/{APP}/gpui-auto-update.managed"),
            marker_contents(APP).as_bytes(),
            0o644,
        )
        .gz();

    let staged = fx.stage(archive).unwrap();

    assert_eq!(validate_layout(staged.prefix(), APP), Ok(()));
}

#[test]
fn long_names_within_the_root_are_supported() {
    let fx = Install::new();
    let long = format!("{ROOT}/share/demo/{}/file.txt", "d".repeat(150));
    let staged = fx
        .stage(Tarball::release(ROOT).long_file(&long, b"deep").gz())
        .unwrap();

    let relative = Path::new(&long).strip_prefix(ROOT).unwrap();
    assert_eq!(fs::read(staged.prefix().join(relative)).unwrap(), b"deep");
}

// ---------------------------------------------------------------------------
// Unsafe paths

#[test]
fn path_traversal_is_rejected() {
    let fx = Install::new();
    for name in [
        format!("{ROOT}/../escaped"),
        format!("{ROOT}/bin/../../escaped"),
        "../escaped".to_owned(),
    ] {
        let archive = Tarball::release(ROOT).file(&name, b"x", 0o644).gz();
        assert!(
            matches!(fx.reject(archive), StageError::PathTraversal { .. }),
            "{name}"
        );
    }
    assert!(!fx.parent().join("escaped").exists());
}

#[test]
fn absolute_paths_are_rejected() {
    let fx = Install::new();
    let target = Path::new("/tmp/gpui-auto-update-extraction-test-absolute");
    let archive = Tarball::release(ROOT)
        .file(target.to_str().unwrap(), b"x", 0o644)
        .gz();

    assert!(matches!(
        fx.reject(archive),
        StageError::AbsolutePath { .. }
    ));
    assert!(!target.exists());
}

#[test]
fn current_directory_and_empty_names_are_rejected() {
    let fx = Install::new();
    for name in [format!("./{ROOT}/bin/x"), String::new()] {
        let archive = Tarball::release(ROOT).file(&name, b"x", 0o644).gz();
        assert!(
            matches!(fx.reject(archive), StageError::UnsafePath { .. }),
            "{name:?}"
        );
    }
}

#[test]
fn unexpected_top_level_roots_are_rejected() {
    let fx = Install::new();
    for archive in [
        Tarball::release(ROOT).file("README", b"x", 0o644).gz(),
        Tarball::release(ROOT)
            .file("other/bin/demo", b"x", 0o755)
            .gz(),
        Tarball::release("demo").gz(),
        Tarball::release("other-1.5.0-linux-x86_64").gz(),
        Tarball::release(ROOT).file(ROOT, b"x", 0o644).gz(),
    ] {
        assert!(matches!(
            fx.reject(archive),
            StageError::UnexpectedRoot { .. } | StageError::DuplicatePath { .. }
        ));
    }
    let archive = Tarball::release("demo").gz();
    match fx.reject(archive) {
        StageError::UnexpectedRoot { expected, found } => {
            assert_eq!(expected, ROOT);
            assert_eq!(found, "demo");
        }
        other => panic!("unexpected error {other:?}"),
    }
}

#[test]
fn a_top_level_file_named_like_the_root_is_rejected() {
    let fx = Install::new();
    let archive = Tarball::new().file(ROOT, b"x", 0o644).gz();

    assert!(matches!(
        fx.reject(archive),
        StageError::UnexpectedRoot { .. }
    ));
}

// ---------------------------------------------------------------------------
// Version-derived paths

#[test]
fn a_root_for_another_version_is_rejected() {
    let fx = Install::new();
    // Correctly signed, but an older release relabeled as 1.5.0 in the feed.
    match fx.reject(Tarball::release("demo-1.4.0-linux-x86_64").gz()) {
        StageError::VersionMismatch { expected, found } => {
            assert_eq!(expected.as_str(), "1.5.0");
            assert_eq!(found.as_str(), "1.4.0");
        }
        other => panic!("unexpected error {other:?}"),
    }
    // Build metadata is part of the published version text.
    assert!(matches!(
        fx.reject(Tarball::release("demo-1.5.0+rebuild-linux-x86_64").gz()),
        StageError::VersionMismatch { .. }
    ));
}

#[test]
fn a_root_with_an_invalid_version_is_rejected() {
    let fx = Install::new();
    for root in [
        "demo-1.5-linux-x86_64",
        "demo-v1.5.0-linux-x86_64",
        "demo-01.5.0-linux-x86_64",
        "demo--linux-x86_64",
    ] {
        assert!(
            matches!(
                fx.reject(Tarball::release(root).gz()),
                StageError::InvalidVersionPath { .. }
            ),
            "{root}"
        );
    }
}

#[test]
fn a_root_for_another_architecture_is_rejected() {
    let fx = Install::new();
    match fx.reject(Tarball::release("demo-1.5.0-linux-aarch64").gz()) {
        StageError::ArchMismatch { expected, found } => {
            assert_eq!(expected, Arch::X86_64);
            assert_eq!(found, Arch::Aarch64);
        }
        other => panic!("unexpected error {other:?}"),
    }
}

#[test]
fn prerelease_versions_name_the_root() {
    let fx = Install::new();
    let root = "demo-2.0.0-linux.1-linux-x86_64";
    let artifact = fx.download("2.0.0-linux.1", Tarball::release(root).gz());

    let staged = ReleaseStager::new(Arch::X86_64)
        .stage(&fx.install, &artifact)
        .unwrap();

    assert_eq!(staged.prefix().file_name().unwrap(), root);
    assert_eq!(staged.version().as_str(), "2.0.0-linux.1");
}

// ---------------------------------------------------------------------------
// Entry types

#[test]
fn symlinks_are_rejected() {
    let fx = Install::new();
    for target in ["/etc/passwd", "../../outside", "data.txt"] {
        let archive = Tarball::release(ROOT)
            .link(
                &format!("{ROOT}/share/demo/link"),
                tar::EntryType::Symlink,
                target,
            )
            .gz();
        assert!(matches!(fx.reject(archive), StageError::Link { .. }));
    }
}

#[test]
fn hard_links_are_rejected() {
    let fx = Install::new();
    let archive = Tarball::release(ROOT)
        .link(
            &format!("{ROOT}/share/demo/hard"),
            tar::EntryType::Link,
            &format!("{ROOT}/share/demo/data.txt"),
        )
        .gz();

    assert!(matches!(fx.reject(archive), StageError::Link { .. }));
}

#[test]
fn special_files_are_rejected() {
    let fx = Install::new();
    for kind in [
        tar::EntryType::Char,
        tar::EntryType::Block,
        tar::EntryType::Fifo,
        tar::EntryType::Continuous,
        tar::EntryType::GNUSparse,
        tar::EntryType::new(b'Z'),
    ] {
        let archive = Tarball::release(ROOT)
            .raw(&format!("{ROOT}/share/demo/special"), kind, b"", 0o644)
            .gz();
        let error = fx.reject(archive);
        assert!(
            matches!(error, StageError::SpecialFile { .. }),
            "{kind:?}: {error:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Duplicates and limits

#[test]
fn duplicate_paths_are_rejected() {
    let fx = Install::new();
    for duplicate in [
        format!("{ROOT}/share/demo/data.txt"),
        format!("{ROOT}//share/demo/data.txt"),
        format!("{ROOT}/share/./demo/data.txt"),
        format!("{ROOT}/bin/"),
    ] {
        let archive = Tarball::release(ROOT).file(&duplicate, b"evil", 0o644).gz();
        assert!(
            matches!(fx.reject(archive), StageError::DuplicatePath { .. }),
            "{duplicate}"
        );
    }
}

#[test]
fn excessive_entry_counts_are_rejected() {
    let fx = Install::new();
    let limits = ArchiveLimits {
        max_entries: 7,
        ..ArchiveLimits::default()
    };
    // The release has exactly seven entries.
    assert!(fx.stage_with(Tarball::release(ROOT).gz(), limits).is_ok());
    let fx = Install::new();
    let archive = Tarball::release(ROOT)
        .file(&format!("{ROOT}/extra"), b"x", 0o644)
        .gz();

    assert!(matches!(
        fx.reject_with(archive, limits),
        StageError::TooManyEntries { limit: 7 }
    ));
}

#[test]
fn excessive_compressed_size_is_rejected_before_reading() {
    let fx = Install::new();
    let archive = Tarball::release(ROOT).gz();
    let length = archive.len() as u64;
    let limits = ArchiveLimits {
        max_compressed_bytes: length - 1,
        ..ArchiveLimits::default()
    };

    match fx.reject_with(archive, limits) {
        StageError::ArchiveTooLarge { length: got, limit } => {
            assert_eq!(got, length);
            assert_eq!(limit, length - 1);
        }
        other => panic!("unexpected error {other:?}"),
    }
}

#[test]
fn declared_expanded_size_above_the_limit_is_rejected() {
    let fx = Install::new();
    let limits = ArchiveLimits {
        max_expanded_bytes: 1024 * 1024,
        ..ArchiveLimits::default()
    };
    // A header that claims far more data than the archive holds.
    let archive = Tarball::release(ROOT)
        .declared(&format!("{ROOT}/huge"), 1 << 40)
        .gz();

    assert!(matches!(
        fx.reject_with(archive, limits),
        StageError::ExpandedTooLarge { limit } if limit == 1024 * 1024
    ));
}

#[test]
fn decompression_bombs_are_cut_off() {
    let fx = Install::new();
    let limits = ArchiveLimits {
        max_expanded_bytes: 1024 * 1024,
        ..ArchiveLimits::default()
    };
    let zeros = vec![0u8; 8 * 1024 * 1024];
    let archive = Tarball::release(ROOT)
        .file(&format!("{ROOT}/zeros"), &zeros, 0o644)
        .gz();
    assert!(archive.len() < 64 * 1024, "fixture must compress well");

    assert!(matches!(
        fx.reject_with(archive, limits),
        StageError::ExpandedTooLarge { .. }
    ));
}

#[test]
fn oversized_metadata_records_count_toward_the_expanded_limit() {
    let fx = Install::new();
    let limits = ArchiveLimits {
        max_expanded_bytes: 64 * 1024,
        ..ArchiveLimits::default()
    };
    // A GNU long-name record is read whole by the tar reader, so it must be
    // bounded by the stream limit rather than by any entry's declared size.
    let long = format!("{ROOT}/{}", "n/".repeat(100 * 1024));
    let archive = Tarball::release(ROOT).long_file(&long, b"").gz();

    assert!(matches!(
        fx.reject_with(archive, limits),
        StageError::ExpandedTooLarge { .. }
    ));
}

#[test]
fn corrupt_archives_are_rejected() {
    let fx = Install::new();
    let mut truncated = Tarball::release(ROOT).gz();
    truncated.truncate(truncated.len() / 2);
    for archive in [b"not a gzip stream".to_vec(), truncated] {
        assert!(matches!(fx.reject(archive), StageError::Malformed(_)));
    }
}

// ---------------------------------------------------------------------------
// Staged layout

#[test]
fn staged_layout_without_the_executable_is_rejected() {
    let fx = Install::new();
    let archive = Tarball::new()
        .dir(ROOT)
        .file(
            &format!("{ROOT}/share/{APP}/gpui-auto-update.managed"),
            marker_contents(APP).as_bytes(),
            0o644,
        )
        .gz();

    assert!(matches!(
        fx.reject(archive),
        StageError::InvalidLayout(LayoutError::MissingExecutable)
    ));
}

#[test]
fn staged_layout_with_a_non_executable_binary_is_rejected() {
    let fx = Install::new();
    let archive = Tarball::new()
        .file(&format!("{ROOT}/bin/{APP}"), b"bin", 0o644)
        .file(
            &format!("{ROOT}/share/{APP}/gpui-auto-update.managed"),
            marker_contents(APP).as_bytes(),
            0o644,
        )
        .gz();

    assert!(matches!(
        fx.reject(archive),
        StageError::InvalidLayout(LayoutError::NotExecutable)
    ));
}

#[test]
fn staged_layout_without_a_valid_marker_is_rejected() {
    let fx = Install::new();
    let without = Tarball::new()
        .file(&format!("{ROOT}/bin/{APP}"), b"bin", 0o755)
        .gz();
    assert!(matches!(
        fx.reject(without),
        StageError::InvalidLayout(LayoutError::MissingMarker)
    ));

    let wrong = Tarball::new()
        .file(&format!("{ROOT}/bin/{APP}"), b"bin", 0o755)
        .file(
            &format!("{ROOT}/share/{APP}/gpui-auto-update.managed"),
            marker_contents("other").as_bytes(),
            0o644,
        )
        .gz();
    assert!(matches!(
        fx.reject(wrong),
        StageError::InvalidLayout(LayoutError::InvalidMarker)
    ));
}

#[test]
fn validate_layout_rejects_symlinked_executables() {
    let dir = tempfile::tempdir().unwrap();
    let prefix = dir.path().join("demo");
    fs::create_dir_all(prefix.join("bin")).unwrap();
    fs::create_dir_all(prefix.join("share/demo")).unwrap();
    fs::write(
        prefix.join("share/demo/gpui-auto-update.managed"),
        marker_contents(APP),
    )
    .unwrap();
    let real = dir.path().join("real");
    fs::write(&real, b"bin").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&real, prefix.join("bin/demo")).unwrap();

    assert_eq!(
        validate_layout(&prefix, APP),
        Err(LayoutError::MissingExecutable)
    );
}

// ---------------------------------------------------------------------------
// Verified input and error reporting

#[test]
fn an_artifact_changed_after_verification_is_not_extracted() {
    let fx = Install::new();
    let artifact = fx.download("1.5.0", Tarball::release(ROOT).gz());
    let mut tampered = Tarball::release(ROOT)
        .file(&format!("{ROOT}/bin/extra"), b"x", 0o755)
        .gz();
    tampered.truncate(artifact.length() as usize - 1);
    fs::write(artifact.path(), tampered).unwrap();

    let error = ReleaseStager::new(Arch::X86_64)
        .stage(&fx.install, &artifact)
        .unwrap_err();

    assert!(matches!(error, StageError::ArtifactChanged));
    fx.assert_untouched();
}

#[test]
fn archive_rejections_map_to_archive_validation_errors() {
    let fx = Install::new();
    let traversal = fx.reject(
        Tarball::release(ROOT)
            .file(&format!("{ROOT}/../x"), b"x", 0o644)
            .gz(),
    );
    assert_eq!(
        UpdateError::from(traversal).kind(),
        ErrorKind::ArchiveValidation
    );

    let layout = fx.reject(Tarball::new().dir(ROOT).gz());
    assert_eq!(
        UpdateError::from(layout).kind(),
        ErrorKind::ArchiveValidation
    );
}

//! `gpui-auto-update doctor`: validates a consuming project's updater
//! integration from its Cargo.toml metadata, local artifacts, and published
//! feeds served from a loopback HTTP server. Nothing is published.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signer as _, SigningKey};

/// Disposable keys: seeds of 32 repeated bytes.
fn key(byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[byte; 32])
}

fn public(key: &SigningKey) -> String {
    B64.encode(key.verifying_key().to_bytes())
}

fn private(key: &SigningKey) -> String {
    B64.encode(key.to_bytes())
}

/// A loopback server whose files can be replaced while it runs. Unknown
/// paths answer 404.
#[derive(Clone)]
struct Server {
    base: String,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl Server {
    fn start() -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let files: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::default();
        let shared = Arc::clone(&files);
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let body = shared.lock().unwrap().get(request.url()).cloned();
                let _ = match body {
                    Some(body) => request.respond(tiny_http::Response::from_data(body)),
                    None => request.respond(tiny_http::Response::empty(404)),
                };
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            files,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn put(&self, path: &str, body: impl Into<Vec<u8>>) {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_owned(), body.into());
    }

    /// Publishes a native feed with one signed `version` entry for each
    /// `(os, arch)` and returns its URL.
    fn feed(&self, path: &str, signer: &SigningKey, entries: &[(&str, &str, &str)]) -> String {
        let mut items = String::new();
        for (version, os, arch) in entries {
            let artifact = format!("/releases/{version}/example-{version}-{os}-{arch}.bin");
            let bytes = format!("artifact {version} {os} {arch}").into_bytes();
            let signature = B64.encode(signer.sign(&bytes).to_bytes());
            items.push_str(&format!(
                r#"    <item>
      <sparkle:version>{version}</sparkle:version>
      <enclosure url="{url}" length="{length}" type="application/octet-stream" sparkle:os="{os}" gpui-auto-update:arch="{arch}" sparkle:edSignature="{signature}"/>
    </item>
"#,
                url = self.url(&artifact),
                length = bytes.len(),
            ));
            self.put(&artifact, bytes);
        }
        self.put(
            path,
            format!(
                r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" xmlns:gpui-auto-update="https://github.com/thedavidweng/gpui-auto-update/xml-namespaces/feed">
  <channel>
    <title>Example</title>
{items}  </channel>
</rss>
"#
            ),
        );
        self.url(path)
    }
}

/// A consuming application package in a temporary directory.
struct Project {
    dir: tempfile::TempDir,
}

impl Project {
    /// `metadata` is appended to a Cargo.toml for package `example` 1.2.3
    /// that depends on the facade.
    fn new(metadata: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            format!(
                "[package]\nname = \"example\"\nversion = \"1.2.3\"\nedition = \"2024\"\n\n\
                 [dependencies]\ngpui-auto-update = \"0.1\"\n\n{}",
                metadata.replace("@KEY@", &public(&key(1)))
            ),
        )
        .unwrap();
        Self { dir }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn manifest(&self) -> String {
        self.path("Cargo.toml").to_string_lossy().into_owned()
    }

    fn doctor(&self, extra: &[&str]) -> Output {
        self.doctor_with_env(extra, &[])
    }

    fn doctor_with_env(&self, extra: &[&str], env: &[(&str, &str)]) -> Output {
        let manifest = self.manifest();
        let mut command = Command::new(env!("CARGO_BIN_EXE_gpui-auto-update"));
        command
            .args(["doctor", "--manifest-path", &manifest])
            .args(extra);
        for (name, value) in env {
            command.env(name, value);
        }
        command.output().expect("failed to run gpui-auto-update")
    }
}

fn output(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn assert_passes(out: &Output) {
    assert!(out.status.success(), "{}", output(out));
}

fn assert_fails(out: &Output, needle: &str) {
    let text = output(out);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains(needle), "expected {needle:?} in:\n{text}");
}

/// A gzip tar archive containing `files` (path, contents, executable).
fn tar_gz(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    for (path, contents, executable) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(if *executable { 0o755 } else { 0o644 });
        header.set_entry_type(tar::EntryType::Regular);
        builder.append_data(&mut header, path, *contents).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

const MARKER: &[u8] = b"gpui-auto-update managed-install 1\napp=example\n";

/// A Linux release archive laid out as a managed install.
fn linux_release(root: &str) -> Vec<u8> {
    tar_gz(&[
        (&format!("{root}/bin/example"), b"\x7fELF", true),
        (
            &format!("{root}/share/example/gpui-auto-update.managed"),
            MARKER,
            false,
        ),
    ])
}

/// The smallest PE image header with the given COFF machine type.
fn pe(machine: u16) -> Vec<u8> {
    let mut bytes = vec![0u8; 0x80];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    bytes[0x40..0x44].copy_from_slice(b"PE\0\0");
    bytes[0x44..0x46].copy_from_slice(&machine.to_le_bytes());
    bytes
}

const AMD64: u16 = 0x8664;
const ARM64: u16 = 0xaa64;

/// A project configured for Linux and Windows on x86_64, with both feeds
/// published and both release artifacts built.
fn complete(server: &Server, signer: &SigningKey) -> Project {
    let linux = server.feed(
        "/appcast-linux-x86_64.xml",
        signer,
        &[("1.2.3", "linux", "x86_64")],
    );
    let windows = server.feed(
        "/appcast-windows-x86_64.xml",
        signer,
        &[("1.2.3", "windows", "x86_64")],
    );
    let project = Project::new(&format!(
        r#"[package.metadata.gpui-auto-update]
app-id = "dev.gpui-auto-update.example"
public-key = "{key}"

[package.metadata.gpui-auto-update.linux]
app-name = "example"
feeds = {{ x86_64 = "{linux}" }}
artifacts = {{ x86_64 = "dist/example-1.2.3-linux-x86_64.tar.gz" }}

[package.metadata.gpui-auto-update.windows]
strategy = "portable"
feeds = {{ x86_64 = "{windows}" }}
artifacts = {{ x86_64 = "dist/example-1.2.3-windows-x86_64.exe" }}
"#,
        key = public(signer),
    ));
    project.write(
        "dist/example-1.2.3-linux-x86_64.tar.gz",
        &linux_release("example-1.2.3-linux-x86_64"),
    );
    project.write("dist/example-1.2.3-windows-x86_64.exe", &pe(AMD64));
    project
}

#[test]
fn a_complete_valid_configuration_passes() {
    let server = Server::start();
    let signer = key(1);
    let project = complete(&server, &signer);
    let secret = private(&signer);
    let out = project.doctor_with_env(
        &["--allow-http", "--key-env", "DOCTOR_TEST_KEY"],
        &[("DOCTOR_TEST_KEY", &secret)],
    );
    assert_passes(&out);
    let text = output(&out);
    assert!(text.contains("0 errors"), "{text}");
    assert!(
        !text.contains(&secret),
        "the private key was printed:\n{text}"
    );
}

#[test]
fn verbose_mode_lists_each_passing_check() {
    let server = Server::start();
    let signer = key(1);
    let project = complete(&server, &signer);
    let quiet = output(&project.doctor(&["--allow-http"]));
    let verbose = output(&project.doctor(&["--allow-http", "--verbose"]));
    assert!(!quiet.contains("ok["), "{quiet}");
    for needle in [
        "ok[public-key]",
        "ok[linux]",
        "ok[windows]",
        "ok[feed]",
        "linux/x86_64",
    ] {
        assert!(
            verbose.contains(needle),
            "expected {needle:?} in:\n{verbose}"
        );
    }
}

#[test]
fn a_missing_public_key_is_explained() {
    let project = Project::new(
        r#"[package.metadata.gpui-auto-update]
app-id = "dev.gpui-auto-update.example"

[package.metadata.gpui-auto-update.linux]
app-name = "example"
feeds = { x86_64 = "https://updates.example.invalid/appcast-linux-x86_64.xml" }
"#,
    );
    let out = project.doctor(&["--offline"]);
    assert_fails(&out, "error[public-key]");
    assert_fails(&out, "gpui-auto-update keys generate");
}

#[test]
fn a_private_key_from_another_key_pair_is_rejected() {
    let server = Server::start();
    let signer = key(1);
    let project = complete(&server, &signer);
    let out = project.doctor_with_env(
        &["--allow-http", "--key-env", "DOCTOR_TEST_KEY"],
        &[("DOCTOR_TEST_KEY", &private(&key(2)))],
    );
    assert_fails(&out, "does not match the configured public-key");
    assert!(!output(&out).contains(&private(&key(2))));
}

#[test]
fn the_insecure_test_key_is_not_a_release_key() {
    let test_key = SigningKey::from_bytes(b"gpui-auto-update-INSECURE-test!!");
    let project = Project::new(&format!(
        r#"[package.metadata.gpui-auto-update]
app-id = "dev.gpui-auto-update.example"
public-key = "{}"

[package.metadata.gpui-auto-update.linux]
app-name = "example"
feeds = {{ x86_64 = "https://updates.example.invalid/appcast-linux-x86_64.xml" }}
"#,
        public(&test_key)
    ));
    let out = project.doctor(&["--offline"]);
    assert_fails(&out, "error[public-key]");
    assert_fails(&out, "insecure");
}

#[test]
fn an_unavailable_sparkle_distribution_is_reported() {
    let project = Project::new(
        r#"[package.metadata.gpui-auto-update]
app-id = "dev.gpui-auto-update.example"
public-key = "@KEY@"

[package.metadata.gpui-auto-update.macos]
feed-url = "https://updates.example.invalid/appcast.xml"
sparkle = "build/sparkle"
"#,
    );
    let out = project.doctor(&["--offline"]);
    assert_fails(&out, "error[sparkle]");
    assert_fails(&out, "gpui-auto-update sparkle fetch");
}

#[cfg(unix)]
#[test]
fn a_pinned_sparkle_distribution_is_accepted() {
    let project = Project::new(
        r#"[package.metadata.gpui-auto-update]
app-id = "dev.gpui-auto-update.example"
public-key = "@KEY@"

[package.metadata.gpui-auto-update.macos]
feed-url = "https://updates.example.invalid/appcast.xml"
sparkle-version = "2.10.0"
sparkle = "build/sparkle"
"#,
    );
    let framework = project.path("build/sparkle/Sparkle.framework");
    fs::create_dir_all(framework.join("Versions/B/Resources")).unwrap();
    std::os::unix::fs::symlink("B", framework.join("Versions/Current")).unwrap();
    fs::write(
        framework.join("Versions/B/Resources/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>org.sparkle-project.Sparkle</string>
<key>CFBundleShortVersionString</key><string>2.10.0</string>
</dict></plist>
"#,
    )
    .unwrap();
    for tool in ["sign_update", "generate_appcast"] {
        project.write(&format!("build/sparkle/bin/{tool}"), b"fixture");
    }
    let out = project.doctor(&["--offline", "--verbose"]);
    assert_passes(&out);
    assert!(output(&out).contains("Sparkle 2.10.0"), "{}", output(&out));

    project.write("build/sparkle/bin/sign_update", b"");
    fs::remove_file(project.path("build/sparkle/bin/generate_appcast")).unwrap();
    assert_fails(&project.doctor(&["--offline"]), "bin/generate_appcast");
}

#[test]
fn an_app_bundle_that_disagrees_with_the_configuration_is_reported() {
    let project = Project::new(
        r#"[package.metadata.gpui-auto-update]
app-id = "dev.gpui-auto-update.example"
public-key = "@KEY@"

[package.metadata.gpui-auto-update.macos]
feed-url = "https://updates.example.invalid/appcast.xml"
sandbox = "non-sandboxed"
app = "dist/Example.app"
"#,
    );
    project.write(
        "dist/Example.app/Contents/Info.plist",
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.gpui-auto-update.example</string>
<key>CFBundleExecutable</key><string>Example</string>
<key>CFBundleShortVersionString</key><string>1.2.3</string>
<key>CFBundleVersion</key><string>7</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
<key>SUFeedURL</key><string>https://updates.example.invalid/appcast.xml</string>
<key>SUPublicEDKey</key><string>{}</string>
</dict></plist>
"#,
            public(&key(2))
        )
        .as_bytes(),
    );
    let out = project.doctor(&["--offline"]);
    assert_fails(&out, "SUPublicEDKey");
    assert_fails(&out, "Sparkle.framework is missing");
}

#[test]
fn a_sparkle_archive_with_the_wrong_checksum_is_rejected() {
    let project = Project::new(
        r#"[package.metadata.gpui-auto-update]
app-id = "dev.gpui-auto-update.example"
public-key = "@KEY@"

[package.metadata.gpui-auto-update.macos]
feed-url = "https://updates.example.invalid/appcast.xml"
sparkle-version = "2.10.0"
sparkle-archive = "cache/Sparkle-2.10.0.tar.xz"
"#,
    );
    project.write("cache/Sparkle-2.10.0.tar.xz", b"not the official archive");
    let out = project.doctor(&["--offline"]);
    assert_fails(&out, "checksum mismatch");
    assert_fails(
        &out,
        "c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c",
    );
}

#[test]
fn a_missing_platform_artifact_is_reported() {
    let server = Server::start();
    let signer = key(1);
    let project = complete(&server, &signer);
    fs::remove_file(project.path("dist/example-1.2.3-linux-x86_64.tar.gz")).unwrap();
    let out = project.doctor(&["--allow-http"]);
    assert_fails(&out, "error[linux]");
    assert_fails(&out, "dist/example-1.2.3-linux-x86_64.tar.gz");
}

#[test]
fn a_feed_without_entries_for_its_platform_is_reported() {
    let server = Server::start();
    let signer = key(1);
    let project = complete(&server, &signer);
    server.feed(
        "/appcast-windows-x86_64.xml",
        &signer,
        &[("1.2.3", "linux", "x86_64")],
    );
    let out = project.doctor(&["--allow-http"]);
    assert_fails(&out, "windows/x86_64");
}

#[test]
fn a_linux_archive_for_another_architecture_is_an_arch_mismatch() {
    let server = Server::start();
    let signer = key(1);
    let project = complete(&server, &signer);
    project.write(
        "dist/example-1.2.3-linux-x86_64.tar.gz",
        &linux_release("example-1.2.3-linux-aarch64"),
    );
    let out = project.doctor(&["--allow-http"]);
    assert_fails(&out, "error[linux]");
    assert_fails(&out, "aarch64");
}

#[test]
fn a_portable_executable_for_another_architecture_is_an_arch_mismatch() {
    let server = Server::start();
    let signer = key(1);
    let project = complete(&server, &signer);
    project.write("dist/example-1.2.3-windows-x86_64.exe", &pe(ARM64));
    let out = project.doctor(&["--allow-http"]);
    assert_fails(&out, "error[windows]");
    assert_fails(&out, "aarch64");
}

#[test]
fn a_feed_listing_another_architecture_is_an_arch_mismatch() {
    let server = Server::start();
    let signer = key(1);
    let project = complete(&server, &signer);
    server.feed(
        "/appcast-linux-x86_64.xml",
        &signer,
        &[("1.2.3", "linux", "aarch64")],
    );
    let out = project.doctor(&["--allow-http"]);
    assert_fails(&out, "error[feed]");
    assert_fails(&out, "linux/aarch64");
}

#[test]
fn a_linux_archive_without_the_ownership_marker_is_rejected() {
    let server = Server::start();
    let signer = key(1);
    let project = complete(&server, &signer);
    project.write(
        "dist/example-1.2.3-linux-x86_64.tar.gz",
        &tar_gz(&[("example-1.2.3-linux-x86_64/bin/example", b"\x7fELF", true)]),
    );
    let out = project.doctor(&["--allow-http"]);
    assert_fails(&out, "gpui-auto-update.managed");
}

#[test]
fn broken_feed_urls_are_reported() {
    let server = Server::start();
    let signer = key(1);
    let project = complete(&server, &signer);
    // Unpublished: the server answers 404.
    server
        .files
        .lock()
        .unwrap()
        .remove("/appcast-linux-x86_64.xml");
    let out = project.doctor(&["--allow-http"]);
    assert_fails(&out, "error[feed]");
    assert_fails(&out, "/appcast-linux-x86_64.xml");

    let malformed = Project::new(
        r#"[package.metadata.gpui-auto-update]
app-id = "dev.gpui-auto-update.example"
public-key = "@KEY@"

[package.metadata.gpui-auto-update.linux]
app-name = "example"
feeds = { x86_64 = "updates.example.invalid/appcast.xml", aarch64 = "http://updates.example.invalid/appcast-aarch64.xml" }
"#,
    );
    let out = malformed.doctor(&["--offline"]);
    assert_fails(&out, "is not an absolute URL");
    assert_fails(&out, "must use https");
}

#[test]
fn offline_mode_skips_published_feeds() {
    let server = Server::start();
    let signer = key(1);
    let project = complete(&server, &signer);
    server.files.lock().unwrap().clear();
    assert_passes(&project.doctor(&["--allow-http", "--offline"]));
}

#[test]
fn unknown_configuration_keys_are_rejected() {
    let project = Project::new(
        r#"[package.metadata.gpui-auto-update]
app-id = "dev.gpui-auto-update.example"
public_key = "@KEY@"
"#,
    );
    assert_fails(&project.doctor(&["--offline"]), "public_key");
}

#[test]
fn a_project_without_configuration_points_to_init() {
    let project = Project::new("");
    assert_fails(&project.doctor(&["--offline"]), "gpui-auto-update init");
}

#[test]
fn secrets_on_the_command_line_in_ci_are_flagged() {
    let server = Server::start();
    let signer = key(1);
    let project = complete(&server, &signer);
    project.write(
        ".github/workflows/release.yml",
        b"jobs:\n  release:\n    steps:\n      - run: gpui-auto-update keys check --key-stdin --public-key x <<< \"${{ secrets.SPARKLE_PRIVATE_KEY }}\"\n      - run: gpui-auto-update feed native --allow-test-key --key-env K\n",
    );
    let out = project.doctor(&["--allow-http"]);
    assert_fails(&out, "error[ci]");
    assert_fails(&out, "release.yml:4");
    assert_fails(&out, "--allow-test-key");
}

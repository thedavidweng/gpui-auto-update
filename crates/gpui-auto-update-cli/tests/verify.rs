//! `gpui-auto-update verify`: audits published native feeds and Sparkle
//! appcasts, and their artifacts, served from a loopback HTTP server.

use std::collections::HashMap;
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

fn sign(key: &SigningKey, bytes: &[u8]) -> String {
    B64.encode(key.sign(bytes).to_bytes())
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
}

fn verify(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gpui-auto-update"))
        .arg("verify")
        .args(args)
        .output()
        .expect("failed to run gpui-auto-update")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn all_output(out: &Output) -> String {
    format!("{}{}", stdout(out), stderr(out))
}

/// One native feed item. `signature: None` leaves the enclosure unsigned.
struct Native {
    version: &'static str,
    path: String,
    length: usize,
    signature: Option<String>,
    os: &'static str,
    arch: &'static str,
}

impl Native {
    /// A signed linux/x86_64 release whose artifact is served at a
    /// versioned path.
    fn release(server: &Server, signer: &SigningKey, version: &'static str) -> Self {
        let path = format!("/releases/{version}/example-{version}-linux-x86_64.tar.gz");
        let bytes = format!("artifact {version}").into_bytes();
        server.put(&path, bytes.clone());
        Self {
            version,
            path,
            length: bytes.len(),
            signature: Some(sign(signer, &bytes)),
            os: "linux",
            arch: "x86_64",
        }
    }

    fn xml(&self, server: &Server) -> String {
        let signature = self
            .signature
            .as_ref()
            .map(|s| format!(" sparkle:edSignature=\"{s}\""))
            .unwrap_or_default();
        format!(
            r#"    <item>
      <sparkle:version>{version}</sparkle:version>
      <enclosure url="{url}" length="{length}" type="application/gzip" sparkle:os="{os}" gpui-auto-update:arch="{arch}"{signature}/>
    </item>
"#,
            version = self.version,
            url = server.url(&self.path),
            length = self.length,
            os = self.os,
            arch = self.arch,
        )
    }
}

fn native_feed(server: &Server, items: &[Native]) -> String {
    let items: String = items.iter().map(|i| i.xml(server)).collect();
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" xmlns:gpui-auto-update="https://github.com/thedavidweng/gpui-auto-update/xml-namespaces/feed">
  <channel>
    <title>Example</title>
{items}  </channel>
</rss>
"#
    )
}

/// Serves a feed of `items` at `/appcast.xml` and returns its URL.
fn publish(server: &Server, items: &[Native]) -> String {
    server.put("/appcast.xml", native_feed(server, items));
    server.url("/appcast.xml")
}

fn verify_native(feed: &str, signer: &SigningKey, extra: &[&str]) -> Output {
    let public = public(signer);
    let mut args = vec!["--feed", feed, "--public-key", &public, "--allow-http"];
    args.extend_from_slice(extra);
    verify(&args)
}

fn assert_fails(out: &Output, needle: &str) {
    assert_eq!(out.status.code(), Some(1), "{}", all_output(out));
    let text = all_output(out);
    assert!(text.contains(needle), "expected {needle:?} in:\n{text}");
}

#[test]
fn a_valid_native_release_candidate_passes() {
    let server = Server::start();
    let signer = key(1);
    let feed = publish(
        &server,
        &[
            Native::release(&server, &signer, "1.5.0"),
            Native::release(&server, &signer, "1.4.0"),
        ],
    );
    let out = verify_native(
        &feed,
        &signer,
        &[
            "--os",
            "linux",
            "--arch",
            "x86_64",
            "--expect-version",
            "1.5.0",
        ],
    );
    assert!(out.status.success(), "{}", all_output(&out));
    let text = stdout(&out);
    assert!(text.contains("native feed"), "{text}");
    assert!(text.contains("2 artifacts verified"), "{text}");
    assert!(text.contains("passed"), "{text}");
}

#[test]
fn an_unsigned_entry_is_reported_and_fails() {
    let server = Server::start();
    let signer = key(1);
    let mut unsigned = Native::release(&server, &signer, "1.4.0");
    unsigned.signature = None;
    let feed = publish(
        &server,
        &[Native::release(&server, &signer, "1.5.0"), unsigned],
    );
    let out = verify_native(&feed, &signer, &["--os", "linux", "--arch", "x86_64"]);
    assert_fails(&out, "unsigned (no sparkle:edSignature)");
    assert_fails(&out, "the updater rejects this feed");
    // The diagnostic names the offending item.
    assert_fails(&out, "item 1 (version 1.4.0)");
}

#[test]
fn a_signature_made_by_the_wrong_key_fails() {
    let server = Server::start();
    let signer = key(1);
    let feed = publish(&server, &[Native::release(&server, &signer, "1.5.0")]);
    let out = verify_native(&feed, &key(2), &[]);
    assert_fails(&out, "does not verify");
}

#[test]
fn an_artifact_that_does_not_exist_is_reported() {
    let server = Server::start();
    let signer = key(1);
    let mut missing = Native::release(&server, &signer, "1.5.0");
    missing.path = "/releases/1.5.0/never-uploaded.tar.gz".to_owned();
    let feed = publish(&server, &[missing]);
    let out = verify_native(&feed, &signer, &[]);
    assert_fails(&out, "cannot be downloaded");
    assert_fails(&out, "404");
}

#[test]
fn a_shorter_artifact_than_declared_is_reported() {
    let server = Server::start();
    let signer = key(1);
    let item = Native::release(&server, &signer, "1.5.0");
    // Replace the artifact with a truncated copy after it was signed.
    server.put(&item.path, b"artifact".to_vec());
    let feed = publish(&server, &[item]);
    let out = verify_native(&feed, &signer, &[]);
    assert_fails(&out, "does not verify");
    assert_fails(&out, "bytes but the feed declares");
}

#[test]
fn a_larger_artifact_than_declared_is_reported() {
    let server = Server::start();
    let signer = key(1);
    let mut item = Native::release(&server, &signer, "1.5.0");
    let mut bigger = format!("artifact {}", item.version).into_bytes();
    bigger.extend_from_slice(b"!!");
    server.put(&item.path, bigger);
    item.length -= 2; // declare fewer bytes than are served and signed for
    let feed = publish(&server, &[item]);
    let out = verify_native(&feed, &signer, &[]);
    assert_fails(&out, "larger than the");
}

#[test]
fn a_feed_without_entries_for_the_requested_platform_is_reported() {
    let server = Server::start();
    let signer = key(1);
    let feed = publish(&server, &[Native::release(&server, &signer, "1.5.0")]);
    let out = verify_native(&feed, &signer, &["--os", "linux", "--arch", "aarch64"]);
    assert_fails(&out, "no entries for linux/aarch64");
    assert_fails(&out, "linux/x86_64");
}

#[test]
fn an_unknown_os_or_arch_value_in_the_feed_is_reported() {
    let server = Server::start();
    let signer = key(1);
    let mut item = Native::release(&server, &signer, "1.5.0");
    item.arch = "amd64";
    let feed = publish(&server, &[item]);
    let out = verify_native(&feed, &signer, &[]);
    assert_fails(&out, "the updater rejects this feed");
    assert_fails(&out, "amd64");
}

#[test]
fn a_broken_feed_url_is_reported() {
    let server = Server::start();
    let signer = key(1);
    let public = public(&signer);
    let feed = server.url("/nothing/here.xml");
    let out = verify(&["--feed", &feed, "--public-key", &public, "--allow-http"]);
    assert_fails(&out, "cannot fetch");
}

#[test]
fn a_missing_public_key_is_a_usage_error() {
    let server = Server::start();
    let feed = server.url("/appcast.xml");
    let out = verify(&["--feed", &feed, "--allow-http"]);
    assert_eq!(out.status.code(), Some(2), "{}", all_output(&out));
    assert!(stderr(&out).contains("--public-key"), "{}", stderr(&out));
}

#[test]
fn an_unusable_public_key_is_a_clear_error() {
    let server = Server::start();
    let feed = server.url("/appcast.xml");
    let out = verify(&["--feed", &feed, "--public-key", "not a key", "--allow-http"]);
    assert_fails(&out, "--public-key is unusable");
}

#[test]
fn the_insecure_test_key_needs_explicit_permission() {
    let server = Server::start();
    let insecure = gpui_auto_update_core::trust::TrustedKey::insecure_test_key().to_base64();
    let feed = server.url("/appcast.xml");
    let out = verify(&["--feed", &feed, "--public-key", &insecure, "--allow-http"]);
    assert_fails(&out, "insecure");
}

#[test]
fn a_mutable_artifact_url_is_reported() {
    let server = Server::start();
    let signer = key(1);
    let mut item = Native::release(&server, &signer, "1.5.0");
    // Re-serve the same bytes at a stable, unversioned name and re-sign.
    let bytes = format!("artifact {}", item.version).into_bytes();
    item.path = "/releases/example-latest.tar.gz".to_owned();
    item.signature = Some(sign(&signer, &bytes));
    server.put(&item.path, bytes);
    let feed = publish(&server, &[item]);
    let out = verify_native(&feed, &signer, &[]);
    assert_fails(&out, "does not contain the version");
}

#[test]
fn expect_version_guards_the_highest_applicable_release() {
    let server = Server::start();
    let signer = key(1);
    let feed = publish(&server, &[Native::release(&server, &signer, "1.5.0")]);
    let ok = verify_native(
        &feed,
        &signer,
        &[
            "--os",
            "linux",
            "--arch",
            "x86_64",
            "--expect-version",
            "1.5.0",
        ],
    );
    assert!(ok.status.success(), "{}", all_output(&ok));
    let wrong = verify_native(
        &feed,
        &signer,
        &[
            "--os",
            "linux",
            "--arch",
            "x86_64",
            "--expect-version",
            "1.4.0",
        ],
    );
    assert_fails(
        &wrong,
        "highest applicable release is 1.5.0, expected 1.4.0",
    );
}

#[test]
fn a_local_feed_file_can_be_verified() {
    let server = Server::start();
    let signer = key(1);
    let items = [Native::release(&server, &signer, "1.5.0")];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("appcast-linux-x86_64.xml");
    std::fs::write(&path, native_feed(&server, &items)).unwrap();
    let public = public(&signer);
    let out = verify(&[
        "--feed",
        path.to_str().unwrap(),
        "--public-key",
        &public,
        "--allow-http",
    ]);
    assert!(out.status.success(), "{}", all_output(&out));
}

#[test]
fn verbose_lists_every_enclosure_checked() {
    let server = Server::start();
    let signer = key(1);
    let feed = publish(
        &server,
        &[
            Native::release(&server, &signer, "1.5.0"),
            Native::release(&server, &signer, "1.4.0"),
        ],
    );
    let out = verify_native(&feed, &signer, &["-v"]);
    assert!(out.status.success(), "{}", all_output(&out));
    let text = stdout(&out);
    assert_eq!(text.matches("fetching http://").count(), 2, "{text}");
}

/// A Sparkle appcast with one signed full archive and one signed delta.
struct Sparkle {
    full_path: String,
    delta_path: String,
    full_length: usize,
    delta_length: usize,
    full_signature: Option<String>,
    delta_signature: Option<String>,
}

impl Sparkle {
    fn release(server: &Server, signer: &SigningKey) -> Self {
        let full_path = "/mac/150/Example-150.zip".to_owned();
        let delta_path = "/mac/150/Example150-140.delta".to_owned();
        let full = b"full archive 150".to_vec();
        let delta = b"delta 140 to 150".to_vec();
        server.put(&full_path, full.clone());
        server.put(&delta_path, delta.clone());
        Self {
            full_path,
            delta_path,
            full_length: full.len(),
            delta_length: delta.len(),
            full_signature: Some(sign(signer, &full)),
            delta_signature: Some(sign(signer, &delta)),
        }
    }

    fn xml(&self, server: &Server) -> String {
        let sig = |s: &Option<String>| {
            s.as_ref()
                .map(|s| format!(" sparkle:edSignature=\"{s}\""))
                .unwrap_or_default()
        };
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Example</title>
    <item>
      <title>1.5.0</title>
      <sparkle:version>150</sparkle:version>
      <sparkle:shortVersionString>1.5.0</sparkle:shortVersionString>
      <enclosure url="{full_url}" length="{full_len}" type="application/octet-stream"{full_sig}/>
      <sparkle:deltas>
        <enclosure url="{delta_url}" sparkle:deltaFrom="140" length="{delta_len}" type="application/octet-stream"{delta_sig}/>
      </sparkle:deltas>
    </item>
  </channel>
</rss>
"#,
            full_url = server.url(&self.full_path),
            full_len = self.full_length,
            full_sig = sig(&self.full_signature),
            delta_url = server.url(&self.delta_path),
            delta_len = self.delta_length,
            delta_sig = sig(&self.delta_signature),
        )
    }

    fn publish(&self, server: &Server) -> String {
        server.put("/appcast.xml", self.xml(server));
        server.url("/appcast.xml")
    }
}

#[test]
fn a_valid_sparkle_appcast_with_deltas_passes() {
    let server = Server::start();
    let signer = key(3);
    let feed = Sparkle::release(&server, &signer).publish(&server);
    let out = verify_native(&feed, &signer, &[]);
    assert!(out.status.success(), "{}", all_output(&out));
    let text = stdout(&out);
    assert!(text.contains("Sparkle appcast"), "{text}");
    assert!(text.contains("2 enclosures (1 deltas)"), "{text}");
    assert!(text.contains("2 artifacts verified"), "{text}");
}

#[test]
fn a_delta_signed_by_another_key_fails() {
    let server = Server::start();
    let signer = key(3);
    let mut appcast = Sparkle::release(&server, &signer);
    let delta = b"delta 140 to 150".to_vec();
    appcast.delta_signature = Some(sign(&key(4), &delta));
    let feed = appcast.publish(&server);
    let out = verify_native(&feed, &signer, &[]);
    assert_fails(&out, "delta enclosure");
    assert_fails(&out, "does not verify");
    // The full archive still verifies, so the summary counts it.
    assert_fails(&out, "1 of 2 enclosures verified");
}

#[test]
fn an_unsigned_sparkle_enclosure_is_reported() {
    let server = Server::start();
    let signer = key(3);
    let mut appcast = Sparkle::release(&server, &signer);
    appcast.full_signature = None;
    let feed = appcast.publish(&server);
    let out = verify_native(&feed, &signer, &[]);
    assert_fails(&out, "unsigned (no sparkle:edSignature)");
}

//! `gpui-auto-update feed`: signed native feeds and Sparkle appcasts,
//! observed through command output and the generated files.

mod support;

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use gpui_auto_update_core::feed::{Arch, Feed, FeedLimits, Os, Selection, UpdateTarget};
use gpui_auto_update_core::trust::{EdSignature, TrustedKey};
use gpui_auto_update_core::version::ReleaseVersion;

/// The shared Ed25519 test vector. The key is RFC 8032 section 7.1 TEST 1
/// (public, disposable). `VECTOR_SIGNATURE` was computed independently with
/// `openssl pkeyutl -sign -rawin` over `VECTOR_ARTIFACT`.
const VECTOR_SEED: &str = "nWGxne/9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A=";
const VECTOR_PUBLIC: &str = "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=";
const VECTOR_ARTIFACT: &[u8] = b"gpui-auto-update shared Ed25519 test vector artifact\n";
const VECTOR_SIGNATURE: &str =
    "PoQxICp0by0zp2BFLEY1SixPztgbaK9CHBzdgUxpTuzpxHwACeI015heUJJ01bvUi7XUX04ArdIS/oKTt3P3Bg==";

/// A second disposable key (the seed is 32 bytes of 0x07).
const OTHER_SEED: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";

const INSECURE_TEST_SEED: &str = "Z3B1aS1hdXRvLXVwZGF0ZS1JTlNFQ1VSRS10ZXN0ISE=";

struct Run<'a> {
    args: Vec<String>,
    env: Vec<(&'a str, &'a str)>,
    stdin: Option<&'a str>,
}

impl<'a> Run<'a> {
    fn new(args: &[&str]) -> Self {
        Self {
            args: args.iter().map(|s| (*s).to_owned()).collect(),
            env: Vec::new(),
            stdin: None,
        }
    }

    fn arg(mut self, arg: impl AsRef<std::ffi::OsStr>) -> Self {
        self.args.push(arg.as_ref().to_string_lossy().into_owned());
        self
    }

    fn env(mut self, key: &'a str, value: &'a str) -> Self {
        self.env.push((key, value));
        self
    }

    fn stdin(mut self, input: &'a str) -> Self {
        self.stdin = Some(input);
        self
    }

    fn run(self) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_gpui-auto-update"));
        cmd.args(&self.args)
            .env_remove("SPARKLE_PRIVATE_KEY")
            .env_remove("SPARKLE_BIN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in self.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("failed to run gpui-auto-update");
        let mut stdin = child.stdin.take().unwrap();
        if let Some(input) = self.stdin {
            stdin.write_all(input.as_bytes()).unwrap();
        }
        drop(stdin);
        child.wait_with_output().unwrap()
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn parse(path: &Path) -> Feed {
    Feed::parse(&fs::read(path).unwrap(), &FeedLimits::default()).expect("core accepts the feed")
}

struct Native {
    tmp: tempfile::TempDir,
    artifact: PathBuf,
    output: PathBuf,
}

impl Native {
    fn new(artifact_name: &str, bytes: &[u8]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let artifact = tmp.path().join(artifact_name);
        fs::write(&artifact, bytes).unwrap();
        let output = tmp.path().join("feeds/appcast-linux-x86_64.xml");
        Self {
            tmp,
            artifact,
            output,
        }
    }

    /// `feed native` for linux/x86_64 1.5.0, signing with the key on stdin
    /// and trusting `public`.
    fn command(&self, public: &str) -> Run<'static> {
        Run::new(&[
            "feed",
            "native",
            "--os",
            "linux",
            "--arch",
            "x86_64",
            "--version",
            "1.5.0",
            "--download-url-prefix",
            "https://downloads.example.com/releases/1.5.0/",
            "--key-stdin",
            "--public-key",
        ])
        .arg(public)
        .arg("--artifact")
        .arg(&self.artifact)
        .arg("--output")
        .arg(&self.output)
    }
}

#[test]
fn native_feed_carries_the_shared_vector_signature_and_core_accepts_it() {
    let n = Native::new("example-1.5.0-linux-x86_64.tar.gz", VECTOR_ARTIFACT);
    let out = n.command(VECTOR_PUBLIC).stdin(VECTOR_SEED).run();
    assert!(out.status.success(), "{}", stderr(&out));

    let feed = parse(&n.output);
    let [item] = feed.items() else {
        panic!("expected one item")
    };
    assert_eq!(item.version.as_str(), "1.5.0");
    let artifact = &item.artifact;
    assert_eq!(artifact.os, Os::Linux);
    assert_eq!(artifact.arch, Arch::X86_64);
    assert_eq!(artifact.length, VECTOR_ARTIFACT.len() as u64);
    assert_eq!(
        artifact.url.as_str(),
        "https://downloads.example.com/releases/1.5.0/example-1.5.0-linux-x86_64.tar.gz"
    );
    assert_eq!(artifact.signature.to_base64(), VECTOR_SIGNATURE);
    assert_eq!(artifact.content_type.as_deref(), Some("application/gzip"));

    let key = TrustedKey::from_base64(VECTOR_PUBLIC).unwrap();
    key.verify_artifact(&artifact.signature, artifact.length, VECTOR_ARTIFACT)
        .expect("core verification accepts the generated signature");

    let target = UpdateTarget::new(Os::Linux, Arch::X86_64);
    let current = ReleaseVersion::parse("1.4.0").unwrap();
    assert!(matches!(
        feed.select(&target, &current).unwrap(),
        Selection::UpdateAvailable(_)
    ));
}

#[test]
fn the_shared_vector_signature_verifies_in_core_independently_of_the_cli() {
    let key = TrustedKey::from_base64(VECTOR_PUBLIC).unwrap();
    let sig = EdSignature::from_base64(VECTOR_SIGNATURE).unwrap();
    key.verify_artifact(&sig, VECTOR_ARTIFACT.len() as u64, VECTOR_ARTIFACT)
        .unwrap();
}

fn public_of(seed: &str) -> String {
    let out = Run::new(&["keys", "public-key", "--key-stdin"])
        .stdin(seed)
        .run();
    assert!(out.status.success(), "{}", stderr(&out));
    stdout(&out).trim().to_owned()
}

#[test]
fn a_private_key_that_does_not_match_the_app_key_is_refused() {
    let n = Native::new("example-1.5.0-linux-x86_64.tar.gz", VECTOR_ARTIFACT);
    let out = n.command(VECTOR_PUBLIC).stdin(OTHER_SEED).run();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("does not match the app's public key"), "{err}");
    assert!(err.contains("--bridge-from-public-key"), "{err}");
    assert!(!err.contains(OTHER_SEED));
    assert!(!n.output.exists());
}

#[test]
fn a_bridge_release_is_signed_with_the_previous_key_and_verifies_with_it() {
    let n = Native::new("example-1.5.0-linux-x86_64.tar.gz", VECTOR_ARTIFACT);
    let new_public = public_of(OTHER_SEED);
    let out = n
        .command(&new_public)
        .arg("--bridge-from-public-key")
        .arg(VECTOR_PUBLIC)
        .stdin(VECTOR_SEED)
        .run();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("bridge release"), "{}", stderr(&out));
    let feed = parse(&n.output);
    let artifact = &feed.items()[0].artifact;
    assert_eq!(artifact.signature.to_base64(), VECTOR_SIGNATURE);
    let old = TrustedKey::from_base64(VECTOR_PUBLIC).unwrap();
    old.verify_artifact(&artifact.signature, artifact.length, VECTOR_ARTIFACT)
        .unwrap();
}

#[test]
fn a_bridge_release_signed_with_the_new_key_is_refused() {
    let n = Native::new("example-1.5.0-linux-x86_64.tar.gz", VECTOR_ARTIFACT);
    let new_public = public_of(OTHER_SEED);
    let out = n
        .command(&new_public)
        .arg("--bridge-from-public-key")
        .arg(VECTOR_PUBLIC)
        .stdin(OTHER_SEED)
        .run();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("does not match --bridge-from-public-key"),
        "{}",
        stderr(&out)
    );
    assert!(!n.output.exists());
}

#[test]
fn a_bridge_to_the_same_key_is_a_usage_error() {
    let n = Native::new("example-1.5.0-linux-x86_64.tar.gz", VECTOR_ARTIFACT);
    let out = n
        .command(VECTOR_PUBLIC)
        .arg("--bridge-from-public-key")
        .arg(VECTOR_PUBLIC)
        .stdin(VECTOR_SEED)
        .run();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

#[test]
fn the_insecure_test_key_needs_explicit_permission() {
    let n = Native::new("example-1.5.0-linux-x86_64.tar.gz", VECTOR_ARTIFACT);
    let test_public = public_of(INSECURE_TEST_SEED);
    let refused = n.command(&test_public).stdin(INSECURE_TEST_SEED).run();
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        stderr(&refused).contains("insecure"),
        "{}",
        stderr(&refused)
    );
    assert!(!n.output.exists());
    let allowed = n
        .command(&test_public)
        .arg("--allow-test-key")
        .stdin(INSECURE_TEST_SEED)
        .run();
    assert!(allowed.status.success(), "{}", stderr(&allowed));
    assert!(stderr(&allowed).contains("INSECURE"));
}

#[test]
fn the_key_can_come_from_an_environment_variable() {
    let n = Native::new("example-1.5.0-linux-x86_64.tar.gz", VECTOR_ARTIFACT);
    let mut run = Run::new(&[]);
    run.args = n.command(VECTOR_PUBLIC).args;
    let pos = run.args.iter().position(|a| a == "--key-stdin").unwrap();
    run.args.splice(
        pos..=pos,
        ["--key-env".to_owned(), "RELEASE_KEY".to_owned()],
    );
    let out = run.env("RELEASE_KEY", VECTOR_SEED).run();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        parse(&n.output).items()[0].artifact.signature.to_base64(),
        VECTOR_SIGNATURE
    );
}

#[test]
fn private_keys_on_the_command_line_are_rejected_without_being_echoed() {
    let n = Native::new("example-1.5.0-linux-x86_64.tar.gz", VECTOR_ARTIFACT);
    for args in [
        vec!["--private-key", VECTOR_SEED],
        vec![VECTOR_SEED],
        vec!["--key-file=secret.txt"],
    ] {
        let mut run = n.command(VECTOR_PUBLIC);
        for a in &args {
            run = run.arg(a);
        }
        let out = run.stdin(VECTOR_SEED).run();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(!stderr(&out).contains(VECTOR_SEED), "{}", stderr(&out));
        assert!(!stdout(&out).contains(VECTOR_SEED));
        assert!(!n.output.exists());
    }
}

/// `feed native` for one release of `os`/`arch`, signed with the vector key.
fn release(
    dir: &Path,
    os: &str,
    arch: &str,
    version: &str,
    feed: Option<&Path>,
    output: &Path,
) -> Output {
    let name = format!("example-{version}-{os}-{arch}.bin");
    let artifact = dir.join(&name);
    fs::write(&artifact, format!("artifact {version} {os} {arch}")).unwrap();
    let mut run = Run::new(&[
        "feed",
        "native",
        "--os",
        os,
        "--arch",
        arch,
        "--version",
        version,
    ])
    .arg("--download-url-prefix")
    .arg(format!("https://downloads.example.com/{version}"))
    .arg("--public-key")
    .arg(VECTOR_PUBLIC)
    .arg("--key-stdin")
    .arg("--artifact")
    .arg(&artifact)
    .arg("--output")
    .arg(output)
    .arg("--pub-date")
    .arg("Mon, 05 Oct 2026 12:00:00 +0000");
    if let Some(feed) = feed {
        run = run.arg("--feed").arg(feed);
    }
    run.stdin(VECTOR_SEED).run()
}

#[test]
fn releases_accumulate_in_one_feed_per_platform_newest_first() {
    let tmp = tempfile::tempdir().unwrap();
    let feed = tmp.path().join("appcast-windows-aarch64.xml");
    for version in ["1.0.0", "1.2.0", "1.1.0"] {
        let feed_arg = feed.exists().then_some(feed.as_path());
        let out = release(tmp.path(), "windows", "aarch64", version, feed_arg, &feed);
        assert!(out.status.success(), "{version}: {}", stderr(&out));
    }
    let parsed = parse(&feed);
    let versions: Vec<_> = parsed.items().iter().map(|i| i.version.as_str()).collect();
    assert_eq!(versions, ["1.2.0", "1.1.0", "1.0.0"]);
    let key = TrustedKey::from_base64(VECTOR_PUBLIC).unwrap();
    for item in parsed.items() {
        let a = &item.artifact;
        assert_eq!((a.os, a.arch), (Os::Windows, Arch::Aarch64));
        let bytes = format!("artifact {} windows aarch64", item.version);
        key.verify_artifact(&a.signature, a.length, bytes.as_bytes())
            .unwrap();
        assert_eq!(a.content_type.as_deref(), Some("application/octet-stream"));
        assert_eq!(
            item.published.as_deref(),
            Some("Mon, 05 Oct 2026 12:00:00 +0000")
        );
    }
    let target = UpdateTarget::new(Os::Windows, Arch::Aarch64);
    let Selection::UpdateAvailable(update) = parsed
        .select(&target, &ReleaseVersion::parse("1.0.0").unwrap())
        .unwrap()
    else {
        panic!("expected an update")
    };
    assert_eq!(update.item.version.as_str(), "1.2.0");
}

#[test]
fn an_existing_feed_with_an_unsigned_entry_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let feed = tmp.path().join("appcast.xml");
    let out = release(tmp.path(), "linux", "x86_64", "1.0.0", None, &feed);
    assert!(out.status.success(), "{}", stderr(&out));
    let signed = fs::read_to_string(&feed).unwrap();
    let start = signed.find("sparkle:edSignature=").unwrap();
    let end = start + signed[start..].find("\"/>").unwrap() + 1;
    let unsigned = format!("{}{}", &signed[..start], &signed[end..]);
    fs::write(&feed, &unsigned).unwrap();

    let out = release(tmp.path(), "linux", "x86_64", "1.1.0", Some(&feed), &feed);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("sparkle:edSignature"),
        "{}",
        stderr(&out)
    );
    assert_eq!(fs::read_to_string(&feed).unwrap(), unsigned);
}

#[test]
fn a_feed_for_another_architecture_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let feed = tmp.path().join("appcast.xml");
    assert!(
        release(tmp.path(), "linux", "x86_64", "1.0.0", None, &feed)
            .status
            .success()
    );
    let out = release(tmp.path(), "linux", "aarch64", "1.1.0", Some(&feed), &feed);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("one feed per operating system and architecture"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_version_already_in_the_feed_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let feed = tmp.path().join("appcast.xml");
    assert!(
        release(tmp.path(), "linux", "x86_64", "1.0.0", None, &feed)
            .status
            .success()
    );
    let out = release(tmp.path(), "linux", "x86_64", "1.0.0", Some(&feed), &feed);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("already lists version 1.0.0"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn artifact_urls_must_be_versioned_and_https() {
    let n = Native::new("example-linux.tar.gz", VECTOR_ARTIFACT);
    let replace_prefix = |prefix: &str| {
        let mut run = n.command(VECTOR_PUBLIC);
        let pos = run
            .args
            .iter()
            .position(|a| a == "--download-url-prefix")
            .unwrap();
        run.args[pos + 1] = prefix.to_owned();
        run.stdin(VECTOR_SEED).run()
    };
    let unversioned = replace_prefix("https://downloads.example.com/latest/");
    assert_eq!(unversioned.status.code(), Some(1));
    assert!(
        stderr(&unversioned).contains("does not contain the version 1.5.0"),
        "{}",
        stderr(&unversioned)
    );
    let http = replace_prefix("http://downloads.example.com/1.5.0/");
    assert_eq!(http.status.code(), Some(2));
    assert!(stderr(&http).contains("https"), "{}", stderr(&http));
    assert!(!n.output.exists());
}

#[test]
fn architecture_is_required_and_aliases_are_rejected() {
    let n = Native::new("example-1.5.0-linux-x86_64.tar.gz", VECTOR_ARTIFACT);
    let mut run = n.command(VECTOR_PUBLIC);
    let pos = run.args.iter().position(|a| a == "--arch").unwrap();
    run.args[pos + 1] = "amd64".to_owned();
    let alias = run.stdin(VECTOR_SEED).run();
    assert_eq!(alias.status.code(), Some(2));
    assert!(stderr(&alias).contains("--arch"), "{}", stderr(&alias));

    let mut run = n.command(VECTOR_PUBLIC);
    run.args.drain(pos..pos + 2);
    let missing = run.stdin(VECTOR_SEED).run();
    assert_eq!(missing.status.code(), Some(2));
    assert!(
        stderr(&missing).contains("--arch is required"),
        "{}",
        stderr(&missing)
    );
}

#[test]
fn a_missing_artifact_fails_without_writing_a_feed() {
    let n = Native::new("example-1.5.0-linux-x86_64.tar.gz", VECTOR_ARTIFACT);
    fs::remove_file(&n.artifact).unwrap();
    let out = n.command(VECTOR_PUBLIC).stdin(VECTOR_SEED).run();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("cannot read"), "{}", stderr(&out));
    assert!(!n.output.exists());
    assert!(n.tmp.path().exists());
}

#[test]
fn invalid_release_metadata_is_rejected_by_the_updater_rules() {
    let n = Native::new("example-1.5.0-linux-x86_64.tar.gz", VECTOR_ARTIFACT);
    let out = n
        .command(VECTOR_PUBLIC)
        .arg("--channel")
        .arg(".hidden")
        .stdin(VECTOR_SEED)
        .run();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("updater would reject"),
        "{}",
        stderr(&out)
    );
    assert!(!n.output.exists());
}

#[test]
fn macos_artifacts_are_directed_to_sparkle() {
    let n = Native::new("example-1.5.0-linux-x86_64.tar.gz", VECTOR_ARTIFACT);
    let mut run = n.command(VECTOR_PUBLIC);
    let pos = run.args.iter().position(|a| a == "--os").unwrap();
    run.args[pos + 1] = "macos".to_owned();
    let out = run.stdin(VECTOR_SEED).run();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("feed sparkle"), "{}", stderr(&out));
}

/// `feed sparkle` against a fixture distribution whose `generate_appcast`
/// records how it was invoked and writes a canned appcast.
#[cfg(unix)]
mod sparkle_appcast {
    use super::*;

    const FAKE_GENERATE_APPCAST: &str = r#"#!/bin/sh
printf '%s\n' "$@" > "$FAKE_ARGS"
cat > "$FAKE_STDIN"
out=""
prev=""
for a in "$@"; do
  if [ "$prev" = "-o" ]; then out="$a"; fi
  prev="$a"
done
if [ -n "$FAKE_FAIL" ]; then echo "generate_appcast: $FAKE_FAIL" >&2; exit 1; fi
cp "$FAKE_APPCAST" "$out"
"#;

    struct Fixture {
        tmp: tempfile::TempDir,
        dist: PathBuf,
        archives: PathBuf,
        output: PathBuf,
        appcast: PathBuf,
    }

    impl Fixture {
        fn new(version: &str) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let dist = support::fixture_distribution(tmp.path(), version);
            let tool = dist.join("bin/generate_appcast");
            fs::write(&tool, FAKE_GENERATE_APPCAST).unwrap();
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
            let archives = tmp.path().join("archives");
            fs::create_dir_all(&archives).unwrap();
            fs::write(archives.join("Example 1.5.0.zip"), VECTOR_ARTIFACT).unwrap();
            fs::write(archives.join("Example1.5.0-1.4.0.delta"), VECTOR_ARTIFACT).unwrap();
            Self {
                output: tmp.path().join("site/appcast.xml"),
                appcast: tmp.path().join("canned.xml"),
                dist,
                archives,
                tmp,
            }
        }

        fn canned(&self, xml: &str) {
            fs::write(&self.appcast, xml).unwrap();
        }

        fn record(&self, name: &str) -> PathBuf {
            self.tmp.path().join(name)
        }

        fn run(&self, public: &str, seed: &str, extra: &[&str]) -> Output {
            let mut run = Run::new(&["feed", "sparkle", "--key-stdin", "--public-key", public])
                .arg("--sparkle")
                .arg(&self.dist)
                .arg("--archives")
                .arg(&self.archives)
                .arg("--output")
                .arg(&self.output);
            for a in extra {
                run = run.arg(a);
            }
            let args = self.record("args.txt");
            let stdin = self.record("stdin.txt");
            let appcast = self.appcast.clone();
            run.env("FAKE_ARGS", leak(&args))
                .env("FAKE_STDIN", leak(&stdin))
                .env("FAKE_APPCAST", leak(&appcast))
                .stdin(seed)
                .run()
        }
    }

    fn leak(path: &Path) -> &'static str {
        Box::leak(path.to_str().unwrap().to_owned().into_boxed_str())
    }

    fn appcast(full_signature: Option<&str>, delta_signature: Option<&str>) -> String {
        let sig = |s: Option<&str>| {
            s.map(|s| format!(" sparkle:edSignature=\"{s}\""))
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
      <enclosure url="https://downloads.example.com/1.5.0/Example%201.5.0.zip" length="{len}" type="application/octet-stream"{full}/>
      <sparkle:deltas>
        <enclosure url="https://downloads.example.com/1.5.0/Example1.5.0-1.4.0.delta" sparkle:deltaFrom="140" length="{len}" type="application/octet-stream"{delta}/>
      </sparkle:deltas>
    </item>
  </channel>
</rss>
"#,
            len = VECTOR_ARTIFACT.len(),
            full = sig(full_signature),
            delta = sig(delta_signature),
        )
    }

    #[test]
    fn generate_appcast_runs_from_the_pinned_distribution_with_the_key_on_stdin() {
        let f = Fixture::new("2.10.0");
        let xml = appcast(Some(VECTOR_SIGNATURE), Some(VECTOR_SIGNATURE));
        f.canned(&xml);
        let out = f.run(
            VECTOR_PUBLIC,
            VECTOR_SEED,
            &[
                "--download-url-prefix",
                "https://downloads.example.com/1.5.0/",
                "--maximum-deltas",
                "3",
            ],
        );
        assert!(out.status.success(), "{}", stderr(&out));
        assert_eq!(fs::read_to_string(&f.output).unwrap(), xml);
        assert!(
            stdout(&out).contains("2 signed enclosures (1 deltas), 2 verified"),
            "{}",
            stdout(&out)
        );

        let args = fs::read_to_string(f.record("args.txt")).unwrap();
        let args: Vec<&str> = args.lines().collect();
        assert_eq!(&args[..2], ["--ed-key-file", "-"]);
        assert!(args.windows(2).any(|w| w
            == [
                "--download-url-prefix",
                "https://downloads.example.com/1.5.0/"
            ]));
        assert!(args.windows(2).any(|w| w == ["--maximum-deltas", "3"]));
        assert_eq!(args.last(), Some(&f.archives.to_str().unwrap()));
        assert!(!args.iter().any(|a| a.contains(VECTOR_SEED)));
        assert_eq!(
            fs::read_to_string(f.record("stdin.txt")).unwrap(),
            VECTOR_SEED
        );
    }

    #[test]
    fn an_unsigned_delta_is_never_published() {
        let f = Fixture::new("2.10.0");
        f.canned(&appcast(Some(VECTOR_SIGNATURE), None));
        let out = f.run(VECTOR_PUBLIC, VECTOR_SEED, &[]);
        assert_eq!(out.status.code(), Some(1));
        let err = stderr(&out);
        assert!(err.contains("delta enclosure"), "{err}");
        assert!(err.contains("unsigned"), "{err}");
        assert!(!f.output.exists());
    }

    #[test]
    fn an_unsigned_update_leaves_the_published_appcast_untouched() {
        let f = Fixture::new("2.10.0");
        fs::create_dir_all(f.output.parent().unwrap()).unwrap();
        fs::write(&f.output, "previous appcast").unwrap();
        f.canned(&appcast(None, Some(VECTOR_SIGNATURE)));
        let out = f.run(VECTOR_PUBLIC, VECTOR_SEED, &[]);
        assert_eq!(out.status.code(), Some(1));
        assert!(stderr(&out).contains("unsigned"), "{}", stderr(&out));
        assert_eq!(fs::read_to_string(&f.output).unwrap(), "previous appcast");
        let leftovers: Vec<_> = fs::read_dir(f.output.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, ["appcast.xml"]);
    }

    #[test]
    fn a_signature_that_does_not_verify_against_the_signing_key_is_refused() {
        let f = Fixture::new("2.10.0");
        let mut wrong = EdSignature::from_base64(VECTOR_SIGNATURE)
            .unwrap()
            .to_base64()
            .into_bytes();
        wrong[0] = if wrong[0] == b'A' { b'B' } else { b'A' };
        let wrong = String::from_utf8(wrong).unwrap();
        f.canned(&appcast(Some(&wrong), Some(VECTOR_SIGNATURE)));
        let out = f.run(VECTOR_PUBLIC, VECTOR_SEED, &[]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr(&out).contains("does not verify against signing key"),
            "{}",
            stderr(&out)
        );
        assert!(!f.output.exists());
    }

    #[test]
    fn a_mismatched_key_pair_fails_before_sparkle_runs() {
        let f = Fixture::new("2.10.0");
        f.canned(&appcast(Some(VECTOR_SIGNATURE), Some(VECTOR_SIGNATURE)));
        let out = f.run(VECTOR_PUBLIC, OTHER_SEED, &[]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr(&out).contains("does not match the app's public key"),
            "{}",
            stderr(&out)
        );
        assert!(!f.record("args.txt").exists());
    }

    #[test]
    fn an_unpinned_sparkle_distribution_is_refused() {
        let f = Fixture::new("2.0.0");
        f.canned(&appcast(Some(VECTOR_SIGNATURE), Some(VECTOR_SIGNATURE)));
        let out = f.run(VECTOR_PUBLIC, VECTOR_SEED, &[]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr(&out).contains("not a pinned release"),
            "{}",
            stderr(&out)
        );
        assert!(!f.record("args.txt").exists());
    }

    #[test]
    fn an_unavailable_sparkle_distribution_is_reported() {
        let f = Fixture::new("2.10.0");
        fs::remove_file(f.dist.join("bin/generate_appcast")).unwrap();
        let out = f.run(VECTOR_PUBLIC, VECTOR_SEED, &[]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr(&out).contains("no bin/generate_appcast"),
            "{}",
            stderr(&out)
        );

        let mut run = Run::new(&[
            "feed",
            "sparkle",
            "--key-stdin",
            "--public-key",
            VECTOR_PUBLIC,
        ])
        .arg("--sparkle")
        .arg(f.tmp.path().join("missing"))
        .arg("--archives")
        .arg(&f.archives)
        .arg("--output")
        .arg(&f.output);
        run = run.stdin(VECTOR_SEED);
        let out = run.run();
        assert_eq!(out.status.code(), Some(1));
        assert!(stderr(&out).contains("sparkle fetch"), "{}", stderr(&out));
    }

    #[test]
    fn a_failing_generate_appcast_is_reported_with_its_output() {
        let f = Fixture::new("2.10.0");
        f.canned(&appcast(Some(VECTOR_SIGNATURE), Some(VECTOR_SIGNATURE)));
        let out = f.run(VECTOR_PUBLIC, VECTOR_SEED, &[]).status;
        assert!(out.success());
        let out = {
            let mut run = Run::new(&[
                "feed",
                "sparkle",
                "--key-stdin",
                "--public-key",
                VECTOR_PUBLIC,
            ])
            .arg("--sparkle")
            .arg(&f.dist)
            .arg("--archives")
            .arg(&f.archives)
            .arg("--output")
            .arg(&f.output);
            run = run
                .env("FAKE_ARGS", leak(&f.record("args.txt")))
                .env("FAKE_STDIN", leak(&f.record("stdin.txt")))
                .env("FAKE_APPCAST", leak(&f.appcast))
                .env("FAKE_FAIL", "no archives found");
            run.stdin(VECTOR_SEED).run()
        };
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr(&out).contains("no archives found"),
            "{}",
            stderr(&out)
        );
        assert!(f.output.exists(), "the previous appcast is kept");
    }
}

/// Downloads the official pinned Sparkle distribution and proves that the
/// shared vector key interoperates: Sparkle's `sign_update` produces and
/// accepts the same signature the CLI writes into native feeds, and
/// `feed sparkle` drives the real `generate_appcast` (with deltas) to an
/// appcast whose every enclosure core verification accepts. Run with
/// `--ignored`.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "downloads the official Sparkle archive"]
fn the_shared_vector_interoperates_with_official_sparkle_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().join("sparkle");
    let fetched = support::cli(&["sparkle", "fetch", "--out", dist.to_str().unwrap()]);
    assert!(fetched.status.success(), "{}", support::stderr(&fetched));
    let sign_update = dist.join("bin/sign_update");

    let n = Native::new("example-1.5.0-linux-x86_64.tar.gz", VECTOR_ARTIFACT);
    let out = n.command(VECTOR_PUBLIC).stdin(VECTOR_SEED).run();
    assert!(out.status.success(), "{}", stderr(&out));
    let native_signature = parse(&n.output).items()[0].artifact.signature.to_base64();

    let sparkle = |args: &[&std::ffi::OsStr]| {
        let mut child = Command::new(&sign_update)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(VECTOR_SEED.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let artifact = n.artifact.as_os_str();
    let signed = sparkle(&[
        "--ed-key-file".as_ref(),
        "-".as_ref(),
        "-p".as_ref(),
        artifact,
    ]);
    assert!(signed.status.success(), "{}", stderr(&signed));
    assert_eq!(stdout(&signed).trim(), VECTOR_SIGNATURE);
    let verified = sparkle(&[
        "--verify".as_ref(),
        "--ed-key-file".as_ref(),
        "-".as_ref(),
        artifact,
        native_signature.as_ref(),
    ]);
    assert!(verified.status.success(), "{}", stderr(&verified));

    let archives = tmp.path().join("archives");
    fs::create_dir_all(&archives).unwrap();
    for (short, build) in [("1.0", "1"), ("1.1", "2")] {
        sparkle_fixture_archive(tmp.path(), &archives, short, build);
    }
    let output = tmp.path().join("site/appcast.xml");
    let out = Run::new(&[
        "feed",
        "sparkle",
        "--key-stdin",
        "--public-key",
        VECTOR_PUBLIC,
    ])
    .arg("--sparkle")
    .arg(&dist)
    .arg("--archives")
    .arg(&archives)
    .arg("--output")
    .arg(&output)
    .arg("--download-url-prefix")
    .arg("https://downloads.example.com/mac/")
    .stdin(VECTOR_SEED)
    .run();
    assert!(out.status.success(), "{}", stderr(&out));
    let xml = fs::read_to_string(&output).unwrap();
    let doc = roxmltree::Document::parse(&xml).unwrap();
    let key = TrustedKey::from_base64(VECTOR_PUBLIC).unwrap();
    let ns = gpui_auto_update_core::feed::SPARKLE_NS;
    let enclosures: Vec<_> = doc
        .descendants()
        .filter(|n| n.has_tag_name("enclosure"))
        .collect();
    assert_eq!(enclosures.len(), 3, "two archives and one delta: {xml}");
    for enclosure in enclosures {
        let url = enclosure.attribute("url").unwrap();
        let file = archives.join(url.rsplit('/').next().unwrap());
        let sig =
            EdSignature::from_base64(enclosure.attribute((ns, "edSignature")).unwrap()).unwrap();
        let len: u64 = enclosure.attribute("length").unwrap().parse().unwrap();
        key.verify_artifact(&sig, len, fs::File::open(&file).unwrap())
            .unwrap_or_else(|e| panic!("{url}: {e}"));
    }
}

/// Writes `<archives>/Example-<short>.zip`, an ad hoc signed app bundle that
/// trusts the vector key.
#[cfg(target_os = "macos")]
fn sparkle_fixture_archive(root: &Path, archives: &Path, short: &str, build: &str) {
    use support::{plist_xml, string};
    let app = root.join(format!("build-{build}/Example.app"));
    support::write(
        &app.join("Contents/Info.plist"),
        &plist_xml(&[
            ("CFBundleIdentifier", string("com.example.FeedFixture")),
            ("CFBundleExecutable", string("Example")),
            ("CFBundlePackageType", string("APPL")),
            ("CFBundleShortVersionString", string(short)),
            ("CFBundleVersion", string(build)),
            ("LSMinimumSystemVersion", string("12.0")),
            ("SUPublicEDKey", string(VECTOR_PUBLIC)),
        ]),
    );
    let exe = app.join("Contents/MacOS/Example");
    support::write(&exe, &format!("#!/bin/sh\necho {short}\n"));
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    let signed = Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(&app)
        .status()
        .unwrap();
    assert!(signed.success());
    let zipped = Command::new("ditto")
        .args(["-c", "-k", "--keepParent"])
        .arg(&app)
        .arg(archives.join(format!("Example-{short}.zip")))
        .status()
        .unwrap();
    assert!(zipped.success());
}

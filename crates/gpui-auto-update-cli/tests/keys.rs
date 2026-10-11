//! Behavior of `gpui-auto-update keys` as seen from a shell.
//!
//! Key material is the RFC 8032 section 7.1 TEST 1 vector, so expected
//! public keys and signatures come from the RFC rather than from this code.

use std::io::Write as _;
use std::process::{Command, Output, Stdio};

/// RFC 8032 TEST 1 secret key (seed), base64.
const SEED: &str = "nWGxne/9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A=";
/// RFC 8032 TEST 1 public key, base64 (`SUPublicEDKey` form).
const PUBLIC: &str = "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=";
/// The same key in Sparkle's legacy 96-byte form: clamped SHA-512(seed)
/// scalar, hash prefix, then the public key.
const LEGACY: &str = "MHyDhk8oM8tCei7xwAoBPP3/J2jZgMCjpSDwBpBN6U+bTwr+KAt0aneGhOdUQlAgV7dHOgPwj5b1o46Sh+Afj9damAGCsQq31Uv+08lkBzoO4XLz2qYjJa8CGmj3B1Ea";
/// A different, valid public key (RFC 8032 TEST 2).
const OTHER_PUBLIC: &str = "PUAXw+hDiVqStwqnTRt+vJyYLM8uxJaMwM1V8Sr0Zgw=";

struct Run<'a> {
    args: &'a [&'a str],
    stdin: Option<&'a str>,
    env: &'a [(&'a str, &'a str)],
}

impl<'a> Run<'a> {
    fn new(args: &'a [&'a str]) -> Self {
        Self {
            args,
            stdin: None,
            env: &[],
        }
    }

    fn stdin(mut self, input: &'a str) -> Self {
        self.stdin = Some(input);
        self
    }

    fn env(mut self, env: &'a [(&'a str, &'a str)]) -> Self {
        self.env = env;
        self
    }

    fn output(self) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_gpui-auto-update"));
        cmd.args(self.args)
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

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_no_secret(out: &Output, secret: &str) {
    assert!(!stdout(out).contains(secret), "secret leaked to stdout");
    assert!(!stderr(out).contains(secret), "secret leaked to stderr");
}

#[test]
fn public_key_is_derived_from_a_seed_on_stdin() {
    let out = Run::new(&["keys", "public-key", "--key-stdin"])
        .stdin(&format!("{SEED}\n"))
        .output();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), PUBLIC);
    assert_no_secret(&out, SEED);
}

/// Public key of the published insecure test key.
const TEST_KEY_PUBLIC: &str = "X4kvjOoTZUsPKjjF0W/qzutLOb3d9LPmYZ4szZGg8wI=";

#[test]
fn legacy_96_byte_key_from_env_yields_the_same_public_key() {
    let out = Run::new(&["keys", "public-key", "--key-env", "MY_SIGNING_KEY"])
        .env(&[("MY_SIGNING_KEY", LEGACY)])
        .output();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), PUBLIC);
    assert!(stderr(&out).contains("legacy 96-byte"));
    assert_no_secret(&out, LEGACY);
}

#[test]
fn key_file_exported_by_generate_keys_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sparkle_private_key");
    std::fs::write(&path, SEED).unwrap();
    let out = Run::new(&["keys", "public-key", "--key-file", path.to_str().unwrap()]).output();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), PUBLIC);
}

#[test]
fn check_passes_when_the_key_pair_matches() {
    let out = Run::new(&["keys", "check", "--public-key", PUBLIC, "--key-stdin"])
        .stdin(SEED)
        .output();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("matches"));
    assert_no_secret(&out, SEED);
}

#[test]
fn check_fails_closed_on_a_key_pair_mismatch() {
    let out = Run::new(&[
        "keys",
        "check",
        "--public-key",
        OTHER_PUBLIC,
        "--key-env",
        "SPARKLE_PRIVATE_KEY",
    ])
    .env(&[("SPARKLE_PRIVATE_KEY", SEED)])
    .output();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("does not match"), "{err}");
    assert!(err.contains(OTHER_PUBLIC) && err.contains(PUBLIC), "{err}");
    assert!(!stdout(&out).contains("ok"));
    assert_no_secret(&out, SEED);
}

#[test]
fn check_rejects_an_unusable_configured_public_key() {
    let out = Run::new(&["keys", "check", "--public-key", "AAAA", "--key-stdin"])
        .stdin(SEED)
        .output();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("configured public key"));
}

#[test]
fn private_keys_are_refused_as_arguments_without_echoing_them() {
    for args in [
        &["keys", "check", "--private-key", SEED][..],
        &["keys", "public-key", &format!("--key={SEED}")],
        &["keys", "public-key", SEED],
    ] {
        let out = Run::new(args).output();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_no_secret(&out, SEED);
        assert_no_secret(&out, &SEED[..12]);
    }
}

#[test]
fn malformed_key_input_is_not_echoed() {
    let garbage = "c2VjcmV0LXNlY3JldC1zZWNyZXQ=";
    let out = Run::new(&["keys", "public-key", "--key-stdin"])
        .stdin(garbage)
        .output();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("32 bytes"));
    assert_no_secret(&out, garbage);
}

#[test]
fn missing_env_var_is_reported_by_name() {
    let out = Run::new(&["keys", "public-key", "--key-env", "SPARKLE_PRIVATE_KEY"]).output();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("`SPARKLE_PRIVATE_KEY` is not set"));
}

#[test]
fn generate_writes_a_private_owner_only_file_and_prints_only_the_public_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("key");
    let path_str = path.to_str().unwrap();
    let out = Run::new(&["keys", "generate", "--output", path_str]).output();
    assert!(out.status.success(), "{}", stderr(&out));
    let secret = std::fs::read_to_string(&path).unwrap();
    assert_no_secret(&out, secret.trim());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let public = stdout(&out).trim().to_owned();
    let derived = Run::new(&["keys", "public-key", "--key-file", path_str]).output();
    assert_eq!(stdout(&derived).trim(), public);
    assert_ne!(public, TEST_KEY_PUBLIC);
}

#[test]
fn generate_never_overwrites_an_existing_key_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("key");
    std::fs::write(&path, SEED).unwrap();
    let out = Run::new(&["keys", "generate", "--output", path.to_str().unwrap()]).output();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("already exists"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SEED);
}

#[test]
fn generate_requires_a_destination_rather_than_printing_the_key() {
    let out = Run::new(&["keys", "generate"]).output();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

#[test]
fn test_key_is_marked_and_refused_for_production() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test-key");
    let path_str = path.to_str().unwrap();
    let out = Run::new(&["keys", "generate", "--test", "--output", path_str]).output();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), TEST_KEY_PUBLIC);
    assert!(stderr(&out).contains("INSECURE"));

    let check = |extra: &[&str]| {
        let mut args = vec![
            "keys",
            "check",
            "--public-key",
            TEST_KEY_PUBLIC,
            "--key-file",
            path_str,
        ];
        args.extend_from_slice(extra);
        Run::new(&args).output()
    };
    let refused = check(&[]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(stderr(&refused).contains("insecure"));
    let allowed = check(&["--allow-test-key"]);
    assert!(allowed.status.success(), "{}", stderr(&allowed));
    assert!(stderr(&allowed).contains("INSECURE"));
}

#[test]
fn import_writes_the_key_unchanged_in_sparkle_format() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("imported");
    let out = Run::new(&[
        "keys",
        "import",
        "--key-stdin",
        "--output",
        path.to_str().unwrap(),
    ])
    .stdin(&format!("{LEGACY}\n"))
    .output();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), PUBLIC);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), LEGACY);
    assert_no_secret(&out, LEGACY);
}

#[test]
fn import_refuses_the_test_key() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("test-key");
    let src_str = src.to_str().unwrap();
    Run::new(&["keys", "generate", "--test", "--output", src_str]).output();
    let dest = dir.path().join("dest");
    let out = Run::new(&[
        "keys",
        "import",
        "--key-file",
        src_str,
        "--output",
        dest.to_str().unwrap(),
    ])
    .output();
    assert_eq!(out.status.code(), Some(1));
    assert!(!dest.exists());
}

/// A stand-in for Sparkle's `generate_keys` that keeps its "Keychain" in a
/// directory, so the Keychain workflow is exercised without touching the
/// real login Keychain.
#[cfg(target_os = "macos")]
fn fake_generate_keys(dir: &std::path::Path, generated_public: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let bin = dir.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let store = dir.join("store");
    let script = format!(
        r#"#!/bin/sh
echo "$@" >> "{store}.args"
[ "$1" = "--account" ] || exit 9
account="$2"; shift 2
case "$1" in
  -p) [ -f "{store}.$account.pub" ] || {{ echo "no key" >&2; exit 1; }}; cat "{store}.$account.pub" ;;
  -f) cp "$2" "{store}.$account.imported"; echo "{PUBLIC}" > "{store}.$account.pub" ;;
  "") [ -f "{store}.$account.pub" ] || echo "{generated_public}" > "{store}.$account.pub"; echo "snippet" ;;
  *) exit 8 ;;
esac
"#,
        store = store.display(),
    );
    let tool = bin.join("generate_keys");
    std::fs::write(&tool, script).unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

#[cfg(target_os = "macos")]
#[test]
fn keychain_workflow_uses_sparkle_generate_keys() {
    let dir = tempfile::tempdir().unwrap();
    let bin = fake_generate_keys(dir.path(), OTHER_PUBLIC);
    let bin_str = bin.to_str().unwrap();

    let generated = Run::new(&["keys", "generate", "--keychain", "--account", "app"])
        .env(&[("SPARKLE_BIN", bin_str)])
        .output();
    assert!(generated.status.success(), "{}", stderr(&generated));
    assert_eq!(stdout(&generated).trim(), OTHER_PUBLIC);

    let check = Run::new(&[
        "keys",
        "check",
        "--keychain",
        "--account",
        "app",
        "--sparkle-bin",
        bin_str,
        "--public-key",
        OTHER_PUBLIC,
    ])
    .output();
    assert!(check.status.success(), "{}", stderr(&check));
    let mismatch = Run::new(&[
        "keys",
        "check",
        "--keychain",
        "--account",
        "app",
        "--sparkle-bin",
        bin_str,
        "--public-key",
        PUBLIC,
    ])
    .output();
    assert_eq!(mismatch.status.code(), Some(1));

    let imported = Run::new(&[
        "keys",
        "import",
        "--key-stdin",
        "--keychain",
        "--sparkle-bin",
        bin_str,
    ])
    .stdin(SEED)
    .output();
    assert!(imported.status.success(), "{}", stderr(&imported));
    assert_eq!(stdout(&imported).trim(), PUBLIC);
    assert_no_secret(&imported, SEED);
    let handed_over = std::fs::read_to_string(dir.path().join("store.ed25519.imported")).unwrap();
    assert_eq!(handed_over, SEED);
    let args = std::fs::read_to_string(dir.path().join("store.args")).unwrap();
    let staged = args
        .lines()
        .find_map(|l| l.strip_prefix("--account ed25519 -f "))
        .expect("generate_keys -f was called");
    assert!(
        !std::path::Path::new(staged).exists(),
        "staged key file was not removed"
    );
    assert!(!args.contains(SEED), "key was passed as an argument");
}

#[cfg(target_os = "macos")]
#[test]
fn keychain_import_fails_when_the_account_keeps_a_different_key() {
    let dir = tempfile::tempdir().unwrap();
    let bin = fake_generate_keys(dir.path(), OTHER_PUBLIC);
    let bin_str = bin.to_str().unwrap();
    // The fake always reports PUBLIC after -f, so importing another key
    // models Sparkle keeping a pre-existing key.
    // RFC 8032 TEST 2 seed.
    let other_seed = "TM0Imyj/ltqdtsNG7BFOD1uKMZ81q6Yk2oz27U+4pvs=";
    let out = Run::new(&[
        "keys",
        "import",
        "--key-stdin",
        "--keychain",
        "--sparkle-bin",
        bin_str,
    ])
    .stdin(other_seed)
    .output();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("different key"));
    assert_no_secret(&out, other_seed);
}

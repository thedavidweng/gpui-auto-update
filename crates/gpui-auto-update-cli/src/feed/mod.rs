//! `gpui-auto-update feed`: signed update feeds.
//!
//! `feed native` signs a Windows or Linux artifact and adds it to that
//! platform's native feed (docs/feed-format.md). `feed sparkle` runs
//! Sparkle's own `generate_appcast` from a pinned distribution to produce the
//! macOS appcast and its deltas. Both refuse to emit an entry that is not
//! signed by the expected key.

mod appcast;
mod args;
mod native;

use std::process::ExitCode;

use gpui_auto_update_core::trust::TrustedKey;

use args::{Failure, ParsedArgs, usage};

pub const USAGE: &str = "\
Generate signed update feeds.

Usage: gpui-auto-update feed <command> [options]

Commands:
  native   Sign a Windows or Linux artifact and add it to that platform's feed
  sparkle  Run Sparkle's generate_appcast to build the macOS appcast and deltas

feed native options:
  --os <windows|linux>          Target operating system (required)
  --arch <x86_64|aarch64>       Target architecture (required, never guessed)
  --version <semver>            sparkle:version of the release (required)
  --artifact <path>             The artifact to sign (required)
  --download-url-prefix <url>   Immutable, versioned URL directory of the artifact
  --url <url>                   ...or the artifact's full URL
  --output <path>               Feed file to write (required)
  --feed <path>                 Existing feed to add the release to (may equal --output)
  --feed-title <text>           Channel title
  --title <text>  --display-version <text>  --pub-date <rfc822>
  --channel <name>  --minimum-system-version <x.y.z>
  --critical  --critical-below <semver>
  --release-notes-url <url>  --full-release-notes-url <url>
  --description-file <path>  --type <mime>
  --allow-http                  Permit http:// artifact URLs (local testing only)

feed sparkle options:
  --sparkle <dir>               Distribution from `gpui-auto-update sparkle fetch` (required)
  --archives <dir>              Directory of macOS update archives (required)
  --output <path>               Appcast file to write or update (required)
  --keychain [--account <name>] Sign with Sparkle's Keychain item instead of a key source
  Passed to generate_appcast: --download-url-prefix, --release-notes-url-prefix,
  --full-release-notes-url, --link, --channel, --versions, --maximum-versions,
  --maximum-deltas, --delta-compression, --critical-update-version,
  --informational-update-versions, --phased-rollout-interval,
  --minimum-update-version, --major-version,
  --ignore-skipped-upgrades-below-version, --embed-release-notes,
  --auto-prune-update-files, --disable-signing-warning

Signing (both commands):
  --public-key <key>            Public key the released app trusts (required)
  --bridge-from-public-key <key>
                                Key-rotation bridge release: sign with the private key of
                                this previous public key while the app trusts --public-key
  --allow-test-key              Permit the insecure test key (development only)
  --key-stdin | --key-env <VAR> | --key-file <path>
                                Private key source (never accepted as an argument)

See docs/feed-generation.md.";

/// Runs `feed` with the arguments that follow it.
pub fn run(mut args: impl Iterator<Item = String>) -> ExitCode {
    let result = match args.next().as_deref() {
        None | Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some("native") => native::run(args),
        Some("sparkle") => appcast::run(args),
        Some(other) if other.starts_with('-') => {
            usage("expected a feed command (native or sparkle) before any option")
        }
        Some(other) => usage(format!("unknown feed command `{other}`")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Help) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Err(Failure::Usage(msg)) => {
            eprintln!("error: {msg}\n\nRun `gpui-auto-update feed --help` for usage.");
            ExitCode::from(2)
        }
        Err(Failure::Failed(msg)) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// Which key may sign this release, decided from the app's trusted key and,
/// for a key-rotation bridge release, the previous key.
struct TrustPolicy {
    app_key: TrustedKey,
    bridge_from: Option<TrustedKey>,
    allow_test_key: bool,
}

impl TrustPolicy {
    fn from_args(args: &ParsedArgs) -> Result<Self, Failure> {
        let app_key = public_key(args.required("--public-key")?, "--public-key")?;
        let bridge_from = args
            .value("--bridge-from-public-key")?
            .map(|k| public_key(k, "--bridge-from-public-key"))
            .transpose()?;
        if bridge_from.as_ref() == Some(&app_key) {
            return usage(
                "--bridge-from-public-key equals --public-key; a bridge release changes the \
                 trusted key, so omit the option for an ordinary release",
            );
        }
        Ok(Self {
            app_key,
            bridge_from,
            allow_test_key: args.switch("--allow-test-key"),
        })
    }

    /// The key whose signature installed copies will check: the previous
    /// key for a bridge release, otherwise the app's key.
    fn expected_signer(&self) -> &TrustedKey {
        self.bridge_from.as_ref().unwrap_or(&self.app_key)
    }

    /// Fails unless `signer` is the key this release must be signed with.
    fn check(&self, signer: &TrustedKey) -> Result<(), Failure> {
        let expected = self.expected_signer();
        if signer != expected {
            let msg = match &self.bridge_from {
                None => format!(
                    "the private key does not match the app's public key\n  \
                     --public-key:              {}\n  \
                     private key's public key:  {}\n\
                     Installed copies would reject this release. If this is a key-rotation \
                     bridge release signed with the previous key, pass \
                     --bridge-from-public-key <previous key> (see docs/key-management.md).",
                    self.app_key.to_base64(),
                    signer.to_base64()
                ),
                Some(_) => format!(
                    "the private key does not match --bridge-from-public-key\n  \
                     --bridge-from-public-key:  {}\n  \
                     private key's public key:  {}\n\
                     A bridge release must be signed with the previous key that installed \
                     copies trust.",
                    expected.to_base64(),
                    signer.to_base64()
                ),
            };
            return Err(Failure::Failed(msg));
        }
        if signer.is_insecure_test_key() || self.app_key.is_insecure_test_key() {
            if !self.allow_test_key {
                return Err(Failure::Failed(
                    "refusing to use the insecure, publicly known test key for a release \
                     (pass --allow-test-key for local development)"
                        .into(),
                ));
            }
            eprintln!("warning: using the INSECURE, publicly known test key; never ship this feed");
        }
        if let Some(previous) = &self.bridge_from {
            eprintln!(
                "note: bridge release: signed with the previous key {} for an app that trusts {}. \
                 Publish it only in the old feeds; list it in the new feeds with a signature \
                 made by the new key (docs/key-management.md).",
                previous.to_base64(),
                self.app_key.to_base64()
            );
        }
        Ok(())
    }
}

fn public_key(text: &str, flag: &str) -> Result<TrustedKey, Failure> {
    TrustedKey::from_base64(text).map_err(|e| Failure::Failed(format!("{flag} is unusable: {e}")))
}

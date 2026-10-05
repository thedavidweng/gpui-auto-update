//! Thin wrapper around Apple's `codesign` tool and parsing of its output.

// Signature inspection only runs on macOS; the parser stays portable so its
// tests run everywhere.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::process::Command;

use super::plist::{self, Dictionary};
use anyhow::{Context, Result, bail};

/// What `codesign -dv --verbose=4` reports about a signed item.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SignatureInfo {
    pub identifier: Option<String>,
    /// Names from the CodeDirectory flags, for example `adhoc` and `runtime`.
    pub flags: Vec<String>,
    /// Certificate chain, leaf first. Empty for ad-hoc signatures.
    pub authorities: Vec<String>,
    pub team_id: Option<String>,
    pub timestamp: Option<String>,
}

impl SignatureInfo {
    pub fn is_adhoc(&self) -> bool {
        self.flags.iter().any(|f| f == "adhoc")
    }

    pub fn has_hardened_runtime(&self) -> bool {
        self.flags.iter().any(|f| f == "runtime")
    }

    pub fn enforces_library_validation(&self) -> bool {
        self.flags
            .iter()
            .any(|f| f == "runtime" || f == "library-validation")
    }

    pub fn is_developer_id(&self) -> bool {
        self.authorities
            .first()
            .is_some_and(|a| a.starts_with("Developer ID Application:"))
    }

    pub fn team(&self) -> &str {
        self.team_id.as_deref().unwrap_or("not set")
    }
}

/// Parses the stderr of `codesign -dv --verbose=4`.
pub fn parse_details(text: &str) -> SignatureInfo {
    let mut info = SignatureInfo::default();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("Identifier=") {
            info.identifier = Some(v.to_owned());
        } else if let Some(v) = line.strip_prefix("Authority=") {
            info.authorities.push(v.to_owned());
        } else if let Some(v) = line.strip_prefix("Timestamp=") {
            info.timestamp = Some(v.to_owned());
        } else if let Some(v) = line.strip_prefix("TeamIdentifier=") {
            info.team_id = (v != "not set").then(|| v.to_owned());
        } else if line.starts_with("CodeDirectory ") {
            if let Some(flags) = line
                .split_whitespace()
                .find_map(|w| w.strip_prefix("flags="))
                .and_then(|f| f.split_once('('))
                .and_then(|(_, rest)| rest.strip_suffix(')'))
            {
                info.flags = flags
                    .split(',')
                    .filter(|f| !f.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
        }
    }
    info
}

fn codesign() -> Command {
    Command::new("/usr/bin/codesign")
}

/// `Ok(None)` when the item is not signed at all.
pub fn details(path: &Path) -> Result<Option<SignatureInfo>> {
    let out = codesign()
        .args(["-dv", "--verbose=4"])
        .arg(path)
        .output()
        .context("cannot run codesign")?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        Ok(Some(parse_details(&stderr)))
    } else if stderr.contains("not signed at all") {
        Ok(None)
    } else {
        bail!("codesign cannot read {}: {}", path.display(), stderr.trim())
    }
}

/// The entitlements embedded in a signature, if any.
pub fn entitlements(path: &Path) -> Result<Option<Dictionary>> {
    let out = codesign()
        .args(["-d", "--entitlements", "-", "--xml"])
        .arg(path)
        .output()
        .context("cannot run codesign")?;
    if !out.status.success() || out.stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let dict = plist::parse_dictionary(&out.stdout)
        .with_context(|| format!("cannot parse the entitlements of {}", path.display()))?;
    Ok(Some(dict))
}

/// `codesign --verify --deep --strict`; returns codesign's message on failure.
pub fn verify_strict(path: &Path) -> Result<std::result::Result<(), String>> {
    let out = codesign()
        .args(["--verify", "--deep", "--strict", "--verbose=2"])
        .arg(path)
        .output()
        .context("cannot run codesign")?;
    Ok(if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    })
}

pub struct SignRequest<'a> {
    pub identity: &'a str,
    pub keychain: Option<&'a Path>,
    pub entitlements: Option<PathBuf>,
    pub preserve_entitlements: bool,
    pub hardened_runtime: bool,
}

pub fn sign(path: &Path, req: &SignRequest<'_>) -> Result<()> {
    let mut cmd = codesign();
    cmd.args(["--force", "--sign", req.identity]);
    if req.hardened_runtime {
        cmd.args(["--options", "runtime"]);
    }
    // Developer ID distribution (and notarization) needs a secure timestamp;
    // ad-hoc signatures cannot carry one.
    cmd.arg(if req.identity == "-" {
        "--timestamp=none"
    } else {
        "--timestamp"
    });
    if let Some(keychain) = req.keychain {
        cmd.arg("--keychain").arg(keychain);
    }
    if let Some(entitlements) = &req.entitlements {
        cmd.arg("--entitlements").arg(entitlements);
    } else if req.preserve_entitlements {
        cmd.arg("--preserve-metadata=entitlements");
    }
    let out = cmd.arg(path).output().context("cannot run codesign")?;
    if !out.status.success() {
        bail!(
            "codesign failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_ad_hoc_signature() {
        let text = "\
Executable=/tmp/Sparkle.framework/Versions/B/Autoupdate
Identifier=Autoupdate-5555494414702111ff343bf9aa9139b533091c57
Format=Mach-O universal (x86_64 arm64)
CodeDirectory v=20500 size=1012 flags=0x10002(adhoc,runtime) hashes=20+7 location=embedded
Signature=adhoc
TeamIdentifier=not set
";
        let info = parse_details(text);
        assert!(info.is_adhoc());
        assert!(info.has_hardened_runtime());
        assert!(!info.is_developer_id());
        assert_eq!(info.team_id, None);
        assert_eq!(info.team(), "not set");
        assert!(info.authorities.is_empty());
    }

    #[test]
    fn parses_a_developer_id_signature() {
        let text = "\
Executable=/Applications/Example.app/Contents/MacOS/Example
Identifier=com.example.app
Format=app bundle with Mach-O thin (arm64)
CodeDirectory v=20500 size=4242 flags=0x10000(runtime) hashes=120+7 location=embedded
Signature size=9042
Authority=Developer ID Application: Example Corp (ABCDE12345)
Authority=Developer ID Certification Authority
Authority=Apple Root CA
Timestamp=Oct 5, 2026 at 10:00:00
Info.plist entries=24
TeamIdentifier=ABCDE12345
";
        let info = parse_details(text);
        assert_eq!(info.identifier.as_deref(), Some("com.example.app"));
        assert_eq!(info.flags, ["runtime"]);
        assert!(info.is_developer_id());
        assert!(!info.is_adhoc());
        assert_eq!(info.team(), "ABCDE12345");
        assert_eq!(info.timestamp.as_deref(), Some("Oct 5, 2026 at 10:00:00"));
    }

    #[test]
    fn a_signature_without_flags_has_no_hardened_runtime() {
        let info = parse_details("CodeDirectory v=20400 size=300 flags=0x0(none) hashes=4+2\n");
        assert_eq!(info.flags, ["none"]);
        assert!(!info.has_hardened_runtime());
    }
}

//! `feed sparkle`: the macOS appcast, produced by Sparkle's own
//! `generate_appcast` from a pinned distribution, then checked so that no
//! unsigned entry is ever published.

use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gpui_auto_update_core::feed::SPARKLE_NS;
use gpui_auto_update_core::trust::{EdSignature, TrustedKey};

use super::TrustPolicy;
use super::args::{Failure, ParsedArgs, Spec, usage};
use crate::keys::Keychain;
use crate::sparkle::distribution_tool;

/// Value options handed to `generate_appcast` unchanged.
const PASSTHROUGH: &[&str] = &[
    "--download-url-prefix",
    "--release-notes-url-prefix",
    "--full-release-notes-url",
    "--link",
    "--channel",
    "--versions",
    "--maximum-versions",
    "--maximum-deltas",
    "--delta-compression",
    "--critical-update-version",
    "--informational-update-versions",
    "--phased-rollout-interval",
    "--minimum-update-version",
    "--major-version",
    "--ignore-skipped-upgrades-below-version",
];

/// Switches handed to `generate_appcast` unchanged.
const PASSTHROUGH_SWITCHES: &[&str] = &[
    "--embed-release-notes",
    "--auto-prune-update-files",
    "--disable-signing-warning",
];

const SPEC: Spec = Spec {
    values: &[
        "--sparkle",
        "--archives",
        "--output",
        "--account",
        "--public-key",
        "--bridge-from-public-key",
        "--download-url-prefix",
        "--release-notes-url-prefix",
        "--full-release-notes-url",
        "--link",
        "--channel",
        "--versions",
        "--maximum-versions",
        "--maximum-deltas",
        "--delta-compression",
        "--critical-update-version",
        "--informational-update-versions",
        "--phased-rollout-interval",
        "--minimum-update-version",
        "--major-version",
        "--ignore-skipped-upgrades-below-version",
    ],
    switches: &[
        "--keychain",
        "--allow-test-key",
        "--embed-release-notes",
        "--auto-prune-update-files",
        "--disable-signing-warning",
    ],
};

/// Archives older than the newest few are moved here by `generate_appcast`.
const OLD_UPDATES: &str = "old_updates";

pub fn run(args: impl Iterator<Item = String>) -> Result<(), Failure> {
    let args = ParsedArgs::parse(args, &SPEC)?;
    let dist = args
        .path("--sparkle")?
        .ok_or_else(|| Failure::Usage("--sparkle <dir> is required".into()))?;
    let archives = args
        .path("--archives")?
        .ok_or_else(|| Failure::Usage("--archives <dir> is required".into()))?;
    let output = args
        .path("--output")?
        .ok_or_else(|| Failure::Usage("--output <path> is required".into()))?;
    let trust = TrustPolicy::from_args(&args)?;
    if !archives.is_dir() {
        return Err(Failure::Failed(format!(
            "{} is not a directory of update archives",
            archives.display()
        )));
    }

    let tool = distribution_tool(&dist, "generate_appcast")?;
    let source = args.single_source()?;
    let account = args.value("--account")?;
    let (signer, key_text) = match (source, args.switch("--keychain")) {
        (Some(source), false) => {
            if account.is_some() {
                return usage("--account only applies with --keychain");
            }
            let key = source.read()?;
            (key.public_key(), Some(key.to_sparkle_base64()))
        }
        (None, true) => {
            let keychain = Keychain::new(Some(dist.join("bin")), account.map(str::to_owned))?;
            (keychain.public_key()?, None)
        }
        (Some(_), true) => return usage("give exactly one key source"),
        (None, false) => {
            return usage(
                "a key source is required: --key-stdin, --key-env <VAR>, --key-file <path>, \
                 or --keychain",
            );
        }
    };
    trust.check(&signer)?;

    // generate_appcast writes next to the final appcast so that the result
    // can be checked before it replaces the published file.
    let parent = match output.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::create_dir_all(&parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".appcast-")
        .suffix(".xml")
        .tempfile_in(&parent)
        .map_err(|e| format!("cannot create a staging file in {}: {e}", parent.display()))?;
    if output.exists() {
        fs::copy(&output, staging.path())
            .map_err(|e| format!("cannot read {}: {e}", output.display()))?;
    } else {
        // generate_appcast treats an existing -o file as the previous appcast.
        fs::remove_file(staging.path()).ok();
    }

    let mut cmd = Command::new(&tool);
    match (&key_text, account) {
        (Some(_), _) => {
            cmd.args(["--ed-key-file", "-"]);
        }
        (None, Some(account)) => {
            cmd.args(["--account", account]);
        }
        (None, None) => {}
    }
    for (name, value) in args.passthrough(PASSTHROUGH) {
        cmd.args([name, value]);
    }
    for name in PASSTHROUGH_SWITCHES {
        if args.switch(name) {
            cmd.arg(name);
        }
    }
    cmd.arg("-o")
        .arg(staging.path())
        .arg(&archives)
        .stdin(if key_text.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to run Sparkle's {}: {e}", tool.display()))?;
    if let Some(text) = &key_text {
        let mut stdin = child.stdin.take().expect("stdin is piped");
        // A tool that exits without reading its key fails below with its own
        // diagnostics, so a broken pipe here is not reported separately.
        let _ = stdin.write_all(text.as_bytes());
    }
    drop(key_text);
    let result = child
        .wait_with_output()
        .map_err(|e| format!("failed to run Sparkle's generate_appcast: {e}"))?;
    let tool_output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    if !result.status.success() {
        return Err(Failure::Failed(format!(
            "Sparkle's generate_appcast failed ({}):\n{}",
            result.status,
            tool_output.trim()
        )));
    }
    if !tool_output.trim().is_empty() {
        eprintln!("{}", tool_output.trim_end());
    }

    let bytes = fs::read(staging.path())
        .map_err(|e| format!("generate_appcast did not write an appcast ({e})"))?;
    let report = check_appcast(&bytes, &signer, &archives).map_err(|problems| {
        format!(
            "refusing to publish the appcast generated by Sparkle:\n  - {}",
            problems.join("\n  - ")
        )
    })?;
    staging
        .persist(&output)
        .map_err(|e| format!("cannot write {}: {}", output.display(), e.error))?;
    println!(
        "{}: {} items, {} signed enclosures ({} deltas), {} verified against local archives",
        output.display(),
        report.items,
        report.enclosures,
        report.deltas,
        report.verified
    );
    Ok(())
}

#[derive(Debug, Default)]
struct Report {
    items: usize,
    enclosures: usize,
    deltas: usize,
    verified: usize,
}

/// Checks that every downloadable enclosure (full and delta) of an appcast
/// carries a well-formed `sparkle:edSignature`, and that every enclosure
/// whose archive is present locally verifies against `signer`.
fn check_appcast(
    bytes: &[u8],
    signer: &TrustedKey,
    archives: &Path,
) -> Result<Report, Vec<String>> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| vec!["the appcast is not UTF-8".to_owned()])?;
    let options = roxmltree::ParsingOptions {
        allow_dtd: false,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(text, options)
        .map_err(|e| vec![format!("the appcast is not valid XML: {e}")])?;
    let channel = doc
        .root_element()
        .children()
        .find(|n| n.has_tag_name("channel"))
        .filter(|_| doc.root_element().has_tag_name("rss"))
        .ok_or_else(|| vec!["the appcast is not an RSS document with a channel".to_owned()])?;

    let mut problems = Vec::new();
    let mut report = Report::default();
    for (index, item) in channel
        .children()
        .filter(|n| n.has_tag_name("item"))
        .enumerate()
    {
        report.items += 1;
        let version = item
            .children()
            .find(|n| n.has_tag_name((SPARKLE_NS, "version")))
            .and_then(|n| n.text())
            .unwrap_or("?");
        let label = format!("item {index} (version {version})");
        let mut enclosures: Vec<(roxmltree::Node<'_, '_>, bool)> = item
            .children()
            .filter(|n| n.has_tag_name("enclosure"))
            .map(|n| (n, false))
            .collect();
        for deltas in item
            .children()
            .filter(|n| n.has_tag_name((SPARKLE_NS, "deltas")))
        {
            enclosures.extend(
                deltas
                    .children()
                    .filter(|n| n.has_tag_name("enclosure"))
                    .map(|n| (n, true)),
            );
        }
        let informational = item
            .children()
            .any(|n| n.has_tag_name((SPARKLE_NS, "informationalUpdate")));
        if enclosures.is_empty() && !informational {
            problems.push(format!("{label} has no enclosure"));
        }
        for (enclosure, delta) in enclosures {
            let kind = if delta {
                "delta enclosure"
            } else {
                "enclosure"
            };
            let url = enclosure.attribute("url").unwrap_or_default();
            let Some(sig) = enclosure.attribute((SPARKLE_NS, "edSignature")) else {
                problems.push(format!(
                    "{label}: {kind} {url} is unsigned (no sparkle:edSignature)"
                ));
                continue;
            };
            let signature = match EdSignature::from_base64(sig) {
                Ok(s) => s,
                Err(e) => {
                    problems.push(format!(
                        "{label}: {kind} {url} has an invalid signature: {e}"
                    ));
                    continue;
                }
            };
            let Some(length) = enclosure
                .attribute("length")
                .and_then(|l| l.parse::<u64>().ok())
                .filter(|l| *l > 0)
            else {
                problems.push(format!("{label}: {kind} {url} has no valid length"));
                continue;
            };
            report.enclosures += 1;
            if delta {
                report.deltas += 1;
            }
            let Some(file) = local_archive(archives, url) else {
                continue;
            };
            let verified = File::open(&file).map_err(|e| e.to_string()).and_then(|f| {
                signer
                    .verify_artifact(&signature, length, f)
                    .map_err(|e| e.to_string())
            });
            match verified {
                Ok(()) => report.verified += 1,
                Err(e) => problems.push(format!(
                    "{label}: {kind} {} does not verify against signing key {}: {e}",
                    file.display(),
                    signer.to_base64()
                )),
            }
        }
    }
    if problems.is_empty() {
        Ok(report)
    } else {
        Err(problems)
    }
}

/// The local file an enclosure URL refers to, if `generate_appcast` left it
/// in the archives directory.
fn local_archive(archives: &Path, url: &str) -> Option<PathBuf> {
    let url = url::Url::parse(url).ok()?;
    let name = percent_decode(url.path_segments()?.next_back()?)?;
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return None;
    }
    [archives.join(&name), archives.join(OLD_UPDATES).join(&name)]
        .into_iter()
        .find(|p| p.is_file())
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

//! `gpui-auto-update verify`: audit a published feed and its artifacts
//! without installing anything.
//!
//! Both native feeds (Windows/Linux, docs/feed-format.md) and Sparkle
//! appcasts (macOS) are supported. Every enclosure is downloaded with a
//! bounded size and its Ed25519 signature is checked against the declared
//! public key. Native feeds additionally go through core's fail-closed
//! parser, so an unsigned or malformed entry is reported even though
//! parsing alone would reject the feed.

pub mod scan;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Args;
use gpui_auto_update_core::feed::{Arch, Feed, FeedLimits, Os, UpdateTarget};
use gpui_auto_update_core::fetch::{FetchError, FetchPolicy, HttpClient};
use gpui_auto_update_core::trust::{EdSignature, TrustedKey};
use gpui_auto_update_core::version::ReleaseVersion;
use url::Url;

use scan::{Document, Enclosure, Format};

/// Default artifact bound for verify. Larger than the updater's 512 MiB
/// client limit so a feed that declares an oversized artifact fails on the
/// declared length with a clear diagnostic instead of a download cutoff.
const DEFAULT_MAX_ARTIFACT_BYTES: u64 = 1024 * 1024 * 1024;

/// Audit a published feed and its artifacts without installing.
#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Feed URL or local feed file to audit.
    #[arg(long, value_name = "URL|PATH")]
    feed: String,
    /// Public key (SUPublicEDKey form) the released application trusts.
    #[arg(long, value_name = "BASE64")]
    public_key: String,
    /// Feed format. Auto-detected by default.
    #[arg(long, value_enum)]
    format: Option<FormatArg>,
    /// Expected target OS for a native feed (windows or linux).
    #[arg(long, value_enum, requires = "arch")]
    os: Option<OsArg>,
    /// Expected target architecture for a native feed.
    #[arg(long, value_enum, requires = "os")]
    arch: Option<ArchArg>,
    /// Require the highest applicable version to be exactly this version.
    #[arg(long, value_name = "SEMVER")]
    expect_version: Option<String>,
    /// Largest artifact to download, in bytes.
    #[arg(long, default_value_t = DEFAULT_MAX_ARTIFACT_BYTES)]
    max_artifact_bytes: u64,
    /// Permit http:// feed and artifact URLs (local testing only).
    #[arg(long)]
    allow_http: bool,
    /// Permit the insecure, publicly known test key (local development only).
    #[arg(long)]
    allow_test_key: bool,
    /// List every enclosure as it is checked.
    #[arg(long, short)]
    verbose: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum FormatArg {
    Native,
    Sparkle,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum OsArg {
    Windows,
    Linux,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ArchArg {
    #[value(name = "x86_64")]
    X86_64,
    #[value(name = "aarch64")]
    Aarch64,
}

struct Report {
    problems: Vec<String>,
    warnings: Vec<String>,
    verbose: bool,
    items: usize,
    enclosures: usize,
    deltas: usize,
    verified: usize,
    bytes: u64,
}

impl Report {
    fn new(verbose: bool) -> Self {
        Self {
            problems: Vec::new(),
            warnings: Vec::new(),
            verbose,
            items: 0,
            enclosures: 0,
            deltas: 0,
            verified: 0,
            bytes: 0,
        }
    }

    fn problem(&mut self, problem: impl Into<String>) {
        self.problems.push(problem.into());
    }

    fn detail(&self, message: String) {
        if self.verbose {
            println!("  {message}");
        }
    }

    /// Prints diagnostics and returns the process exit code.
    fn finish(self, feed: &str, format: Format) -> ExitCode {
        for warning in &self.warnings {
            eprintln!("warning: {warning}");
        }
        if self.problems.is_empty() {
            println!(
                "{feed}: {format}, {} items, {} enclosures ({} deltas), \
                 {} artifacts verified ({} bytes): passed",
                self.items, self.enclosures, self.deltas, self.verified, self.bytes
            );
            ExitCode::SUCCESS
        } else {
            eprintln!("{feed}: {} problem(s):", self.problems.len());
            for problem in &self.problems {
                eprintln!("  - {problem}");
            }
            eprintln!(
                "verification failed ({} of {} enclosures verified)",
                self.verified, self.enclosures
            );
            ExitCode::FAILURE
        }
    }
}

pub fn run(args: VerifyArgs) -> Result<ExitCode> {
    let key = TrustedKey::from_base64(&args.public_key)
        .map_err(|e| anyhow::anyhow!("--public-key is unusable: {e}"))?;
    if key.is_insecure_test_key() {
        if !args.allow_test_key {
            bail!(
                "the insecure, publicly known test key is not a real release key \
                 (pass --allow-test-key for local development)"
            );
        }
        eprintln!("warning: verifying against the INSECURE, publicly known test key");
    }
    let limits = FeedLimits::default();
    let client = HttpClient::new(FetchPolicy {
        allow_insecure_http: args.allow_http,
        ..FetchPolicy::default()
    });

    let bytes = load_feed(&args.feed, &client, limits.max_feed_bytes)
        .with_context(|| format!("cannot read feed {}", args.feed))?;
    let mut report = Report::new(args.verbose);

    let document = match Document::parse(&bytes) {
        Ok(document) => document,
        Err(problem) => {
            report.problem(problem);
            return Ok(report.finish(&args.feed, Format::Native));
        }
    };
    report.items = document.items.len();
    let format = match args.format {
        Some(FormatArg::Native) => Format::Native,
        Some(FormatArg::Sparkle) => Format::Sparkle,
        None => document.detect_format(),
    };

    // Native feeds must also satisfy core's fail-closed parser: an unsigned
    // or malformed entry rejects the feed for the updater, so say so even
    // when the per-enclosure checks below could report the same entry.
    let native = match format {
        Format::Native => match Feed::parse(&bytes, &limits) {
            Ok(feed) => Some(feed),
            Err(e) => {
                report.problem(format!("the updater rejects this feed: {e}"));
                None
            }
        },
        Format::Sparkle => {
            sparkle_checks(&args, &document, &mut report);
            None
        }
    };

    for item in &document.items {
        if item.enclosures.is_empty() && !item.informational {
            report.problem(format!("{} has no enclosure", item.label()));
        }
        for enclosure in &item.enclosures {
            check_enclosure(item, enclosure, format, &args, &client, &key, &mut report);
        }
    }

    if let Some(feed) = &native {
        let target = match (args.os, args.arch) {
            (Some(os), Some(arch)) => Some(UpdateTarget::new(
                match os {
                    OsArg::Windows => Os::Windows,
                    OsArg::Linux => Os::Linux,
                },
                match arch {
                    ArchArg::X86_64 => Arch::X86_64,
                    ArchArg::Aarch64 => Arch::Aarch64,
                },
            )),
            _ => None,
        };
        if let Some(target) = target {
            platform_checks(&args, feed, &target, &mut report);
        }
    }

    Ok(report.finish(&args.feed, format))
}

fn load_feed(feed: &str, client: &HttpClient, max: u64) -> Result<Vec<u8>> {
    match Url::parse(feed) {
        Ok(url) if matches!(url.scheme(), "https" | "http") => Ok(client
            .get_bytes(&url, max)
            .with_context(|| format!("cannot fetch {url}"))?),
        _ => {
            let path = PathBuf::from(feed);
            let bytes =
                std::fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
            if bytes.len() as u64 > max {
                bail!(
                    "{} is larger than the {max}-byte feed limit",
                    path.display()
                );
            }
            Ok(bytes)
        }
    }
}

/// Platform coverage and selection checks for a parsed native feed.
fn platform_checks(args: &VerifyArgs, feed: &Feed, target: &UpdateTarget, report: &mut Report) {
    let mut platforms: Vec<String> = feed
        .items()
        .iter()
        .map(|i| format!("{}/{}", i.artifact.os, i.artifact.arch))
        .collect();
    platforms.sort();
    platforms.dedup();
    let wanted = format!("{}/{}", target.os, target.arch);
    if !platforms.contains(&wanted) {
        report.problem(format!(
            "feed has no entries for {wanted} (entries are for: {})",
            platforms.join(", ")
        ));
    }

    let Some(expected) = args.expect_version.as_deref() else {
        return;
    };
    let expected = match ReleaseVersion::parse(expected) {
        Ok(v) => v,
        Err(e) => {
            report.problem(format!("--expect-version: {e}"));
            return;
        }
    };
    // The feed, not the running machine, decides what the latest release is:
    // select against version 0.0.0 so every listed release qualifies.
    let floor = ReleaseVersion::parse("0.0.0").unwrap();
    match feed.select(target, &floor) {
        Ok(gpui_auto_update_core::feed::Selection::UpdateAvailable(update)) => {
            if update.item.version != expected {
                report.problem(format!(
                    "highest applicable release is {}, expected {expected}",
                    update.item.version
                ));
            }
        }
        Ok(gpui_auto_update_core::feed::Selection::UpToDate) => report.problem(format!(
            "no release is applicable for {}/{}",
            target.os, target.arch
        )),
        Err(e) => report.problem(format!("selection failed: {e}")),
    }
}

/// Appcast-level checks that do not need core's native parser.
fn sparkle_checks(args: &VerifyArgs, document: &Document, report: &mut Report) {
    if args.os.is_some() || args.arch.is_some() {
        report.warnings.push(
            "--os/--arch are ignored for a Sparkle appcast; macOS artifacts carry no \
             architecture and one appcast serves all of them"
                .to_owned(),
        );
    }
    if args.expect_version.is_some() {
        report.warnings.push(
            "--expect-version is ignored for a Sparkle appcast; Sparkle selects by build \
             version at runtime"
                .to_owned(),
        );
    }
    for item in &document.items {
        if item.version.is_none() {
            report.problem(format!("{} has no sparkle:version", item.label()));
        }
    }
}

/// Downloads one enclosure and checks its length and signature.
fn check_enclosure(
    item: &scan::Item,
    enclosure: &Enclosure,
    format: Format,
    args: &VerifyArgs,
    client: &HttpClient,
    key: &TrustedKey,
    report: &mut Report,
) {
    let label = item.label();
    let kind = enclosure.kind();

    let Some(url) = enclosure.url.as_deref() else {
        report.problem(format!("{label}: {kind} has no url"));
        return;
    };
    report.enclosures += 1;
    if enclosure.delta_from.is_some() {
        report.deltas += 1;
    }
    let url = match Url::parse(url) {
        Ok(url) if matches!(url.scheme(), "https" | "http") && url.host().is_some() => url,
        _ => {
            report.problem(format!(
                "{label}: {kind} url {url:?} is not an absolute URL"
            ));
            return;
        }
    };

    if format == Format::Native {
        for (value, what) in [(&enclosure.os, "sparkle:os"), (&enclosure.arch, "arch")] {
            match value {
                Some(_) => {}
                None => report.problem(format!(
                    "{label}: native feed {kind} {url} is missing {what}"
                )),
            }
        }
    } else {
        if let Some(os) = &enclosure.os {
            report.problem(format!(
                "{label}: Sparkle appcast {kind} {url} declares sparkle:os={os:?}; \
                 Sparkle ignores sparkle:os on macOS appcasts"
            ));
        }
        if enclosure.delta_from == Some(None) {
            report.problem(format!(
                "{label}: delta enclosure {url} is missing sparkle:deltaFrom"
            ));
        }
    }

    let length = match enclosure.length.as_deref() {
        Some(text) => match text.parse::<u64>() {
            Ok(n) if n > 0 => Some(n),
            _ => {
                report.problem(format!(
                    "{label}: {kind} {url} has an invalid length {text:?}"
                ));
                None
            }
        },
        None => {
            report.problem(format!("{label}: {kind} {url} has no length"));
            None
        }
    };

    let signature = match enclosure.signature.as_deref() {
        Some(text) => match EdSignature::from_base64(text) {
            Ok(sig) => Some(sig),
            Err(e) => {
                report.problem(format!(
                    "{label}: {kind} {url} is unsigned or malformed: {e}"
                ));
                None
            }
        },
        None => {
            report.problem(format!(
                "{label}: {kind} {url} is unsigned (no sparkle:edSignature)"
            ));
            None
        }
    };

    // Immutable naming: every artifact URL should name the version it
    // carries, so a published artifact is never silently replaced.
    let names: Vec<&str> = [item.version.as_deref(), item.short_version.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    match enclosure.delta_from.as_ref() {
        Some(from) => {
            let from_ok = match from {
                Some(from) => url.path().contains(from.as_str()),
                None => false,
            };
            if !from_ok || !names.iter().any(|v| url.path().contains(v)) {
                report.problem(format!(
                    "{label}: delta artifact URL {url} does not contain both versions it \
                     connects; use immutable, versioned URLs"
                ));
            }
        }
        None if !names.is_empty() && !names.iter().any(|v| url.path().contains(v)) => {
            report.problem(format!(
                "{label}: artifact URL {url} does not contain the version {}; use an \
                 immutable, versioned URL so a published artifact is never replaced",
                names[0]
            ));
        }
        None => {}
    }

    let (Some(length), Some(signature)) = (length, signature) else {
        return;
    };
    if length > args.max_artifact_bytes {
        report.problem(format!(
            "{label}: {kind} {url} declares {length} bytes, over the {}-byte limit",
            args.max_artifact_bytes
        ));
        return;
    }

    report.detail(format!("fetching {url} ({length} bytes)"));
    let bytes = match client.get_bytes(&url, length) {
        Ok(bytes) => bytes,
        Err(FetchError::TooLarge { .. }) => {
            report.problem(format!(
                "{label}: {kind} {url} is larger than the {length} bytes the feed declares"
            ));
            return;
        }
        Err(e) => {
            report.problem(format!("{label}: {kind} {url} cannot be downloaded: {e}"));
            return;
        }
    };
    match key.verify_artifact(&signature, length, bytes.as_slice()) {
        Ok(()) => {
            report.verified += 1;
            report.bytes += bytes.len() as u64;
        }
        Err(e) => report.problem(format!("{label}: {kind} {url} does not verify: {e}")),
    }
}

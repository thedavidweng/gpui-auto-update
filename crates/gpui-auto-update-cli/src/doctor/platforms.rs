//! Per-platform doctor checks: release URLs, Sparkle, installers, ownership,
//! and built artifacts.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use clap::ValueEnum as _;
use gpui_auto_update_core::feed::{Arch, Feed, FeedLimits, Os};
use gpui_auto_update_core::fetch::{FetchPolicy, HttpClient};
use sha2::Digest as _;
use url::Url;

use super::artifacts::{self, PeMachine};
use super::{Context, Report, required};
use crate::project::{self, Config, LinuxConfig, MacosConfig, WindowsConfig};
use crate::sparkle::{self, Pins, SandboxMode};
use crate::verify::scan::{Document, Format};

/// Parses a configured feed URL and applies the transport policy.
fn check_url(
    cx: &Context<'_>,
    area: &'static str,
    key: &str,
    text: &str,
    report: &mut Report,
) -> Option<Url> {
    if project::is_placeholder(text) {
        report.error(
            area,
            format!("{key} is still the placeholder {text:?}; set it to the https URL where you publish this feed"),
        );
        return None;
    }
    let url = match Url::parse(text) {
        Ok(url) if url.host().is_some() => url,
        _ => {
            report.error(area, format!("{key} {text:?} is not an absolute URL"));
            return None;
        }
    };
    match url.scheme() {
        "https" => Some(url),
        "http" if cx.args.allow_http => Some(url),
        _ => {
            report.error(
                area,
                format!("{key} {url} must use https; the updater refuses other schemes"),
            );
            None
        }
    }
}

/// Fetches a published feed, reporting a broken URL.
fn fetch(cx: &Context<'_>, url: &Url, report: &mut Report) -> Option<Vec<u8>> {
    let client = HttpClient::new(FetchPolicy {
        allow_insecure_http: cx.args.allow_http,
        ..FetchPolicy::default()
    });
    match client.get_bytes(url, FeedLimits::default().max_feed_bytes) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            report.error(
                "feed",
                format!(
                    "{url} cannot be fetched: {e}; check the URL (before the first release, publish an empty feed or run with --offline)"
                ),
            );
            None
        }
    }
}

/// The per-architecture feed URLs of a native platform.
fn arch_feeds(
    cx: &Context<'_>,
    area: &'static str,
    os: Os,
    feeds: &BTreeMap<String, String>,
    report: &mut Report,
) -> Vec<(Arch, Url)> {
    if feeds.is_empty() {
        report.error(
            area,
            format!("{os}.feeds is empty; add one https feed URL per architecture you ship (x86_64, aarch64)"),
        );
    }
    let mut out: Vec<(Arch, Url)> = Vec::new();
    for (name, text) in feeds {
        let Some(arch) = Arch::parse(name) else {
            report.error(
                area,
                format!(
                    "{os}.feeds has an entry for {name:?}; architectures are x86_64 and aarch64"
                ),
            );
            continue;
        };
        let key = format!("{os}.feeds.{arch}");
        if let Some(url) = check_url(cx, area, &key, text, report) {
            if let Some((other, _)) = out.iter().find(|(_, u)| *u == url) {
                report.error(
                    area,
                    format!("{os}/{other} and {os}/{arch} share the feed {url}; each architecture needs its own feed"),
                );
            }
            out.push((arch, url));
        }
    }
    out
}

/// Fetches and parses a native feed and checks that it serves `os`/`arch`.
fn native_feed(cx: &Context<'_>, os: Os, arch: Arch, url: &Url, report: &mut Report) {
    if cx.args.offline {
        return;
    }
    let Some(bytes) = fetch(cx, url, report) else {
        return;
    };
    let feed = match Feed::parse(&bytes, &FeedLimits::default()) {
        Ok(feed) => feed,
        Err(e) => {
            report.error("feed", format!("{url}: the updater rejects this feed: {e}"));
            return;
        }
    };
    let wanted = format!("{os}/{arch}");
    let mut others: Vec<String> = feed
        .items()
        .iter()
        .map(|i| format!("{}/{}", i.artifact.os, i.artifact.arch))
        .filter(|p| *p != wanted)
        .collect();
    others.sort();
    others.dedup();
    if !others.is_empty() {
        report.error(
            "feed",
            format!(
                "{url} is the {wanted} feed but lists entries for {}; publish each platform to its own feed",
                others.join(", ")
            ),
        );
    }
    let mut versions = feed
        .items()
        .iter()
        .filter(|i| i.artifact.os == os && i.artifact.arch == arch)
        .map(|i| i.version.clone());
    match versions.next() {
        None => report.warn(
            "feed",
            format!("{url} has no release for {wanted} yet; installed copies will report that they are up to date"),
        ),
        Some(first) => {
            let newest = versions.fold(first, |a, b| if b.is_newer_than(&a) { b } else { a });
            report.ok("feed", format!("{url}: {wanted}, newest release {newest}"));
        }
    }
}

/// Checks for a configured artifact that exists and is named immutably.
fn artifact_path(
    cx: &Context<'_>,
    area: &'static str,
    os: Os,
    name: &str,
    path: &Path,
    configured_feeds: &BTreeMap<String, String>,
    report: &mut Report,
) -> Option<(Arch, PathBuf)> {
    let Some(arch) = Arch::parse(name) else {
        report.error(
            area,
            format!(
                "{os}.artifacts has an entry for {name:?}; architectures are x86_64 and aarch64"
            ),
        );
        return None;
    };
    if !configured_feeds.contains_key(name) {
        report.error(
            area,
            format!("{os}.artifacts.{arch} has no matching {os}.feeds.{arch}; installed copies would never see it"),
        );
    }
    let resolved = cx.project.resolve(path);
    if !resolved.is_file() {
        report.error(
            area,
            format!(
                "the {os}/{arch} artifact {} is missing; build it before running doctor, or remove {os}.artifacts.{arch}",
                path.display()
            ),
        );
        return None;
    }
    if let Some(version) = &cx.version {
        let file_name = resolved
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !file_name.contains(version.as_str()) {
            report.error(
                area,
                format!(
                    "the {os}/{arch} artifact name {file_name} does not contain the version {version}; \
                     release URLs must be immutable, so `feed native` refuses unversioned names"
                ),
            );
        }
    }
    Some((arch, resolved))
}

pub(super) fn windows(cx: &Context<'_>, config: &WindowsConfig, report: &mut Report) {
    const AREA: &str = "windows";
    let help = "choose inno-setup (silent per-user installer), portable (self-replacing executable), or custom (your own InstallerStrategy); see docs/windows-installers.md";
    let strategy = required(
        config.strategy.as_deref(),
        AREA,
        "windows.strategy",
        help,
        report,
    );
    match strategy {
        Some("inno-setup") => report.ok(
            AREA,
            "strategy inno-setup: the installer must install per user and must not require elevation",
        ),
        Some("portable") => report.ok(AREA, "strategy portable"),
        Some("custom") => report.note(
            AREA,
            "strategy custom: your InstallerStrategy owns installer arguments and version confirmation",
        ),
        Some(other) => report.error(AREA, format!("windows.strategy {other:?} is unknown; {help}")),
        None => {}
    }

    let feeds = arch_feeds(cx, AREA, Os::Windows, &config.feeds, report);
    for (name, path) in &config.artifacts {
        let Some((arch, resolved)) =
            artifact_path(cx, AREA, Os::Windows, name, path, &config.feeds, report)
        else {
            continue;
        };
        match artifacts::pe_machine(&resolved) {
            Err(message) => report.error(AREA, message),
            Ok(machine) if strategy == Some("portable") && machine != PeMachine::Known(arch) => {
                report.error(
                    AREA,
                    format!(
                        "{} is built for {machine}, but it is configured as the {arch} portable executable",
                        path.display()
                    ),
                );
            }
            Ok(machine) => report.ok(
                AREA,
                format!("windows/{arch} artifact {} ({machine})", path.display()),
            ),
        }
    }
    for (arch, url) in &feeds {
        native_feed(cx, Os::Windows, *arch, url, report);
    }
}

/// The Linux application-name rules from docs/linux-managed-install.md.
fn valid_app_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.starts_with(['.', '-'])
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

pub(super) fn linux(cx: &Context<'_>, config: &LinuxConfig, report: &mut Report) {
    const AREA: &str = "linux";
    let help = "set it to the executable name installed as <prefix>/bin/<app-name>";
    let app = required(config.app_name.as_deref(), AREA, "linux.app-name", help, report)
        .filter(|name| {
            let ok = valid_app_name(name);
            if !ok {
                report.error(
                    AREA,
                    format!("linux.app-name {name:?} must be 1 to 64 characters from A-Z a-z 0-9 . _ - and not start with . or -"),
                );
            }
            ok
        });
    if let Some(app) = app {
        report.ok(
            AREA,
            format!(
                "managed installs are <prefix>/bin/{app} with the marker share/{app}/{}",
                artifacts::MARKER_FILE_NAME
            ),
        );
        report.note(
            AREA,
            "the update helper is a mode of bin/<app-name>; the application must call the Linux backend's helper entry point first thing in main",
        );
    }

    let feeds = arch_feeds(cx, AREA, Os::Linux, &config.feeds, report);
    for (name, path) in &config.artifacts {
        let Some((arch, resolved)) =
            artifact_path(cx, AREA, Os::Linux, name, path, &config.feeds, report)
        else {
            continue;
        };
        let (Some(app), Some(version)) = (app, cx.version.as_deref()) else {
            continue;
        };
        let problems = artifacts::linux_archive(&resolved, app, version, arch);
        if problems.is_empty() {
            report.ok(
                AREA,
                format!(
                    "linux/{arch} artifact {} is a managed-install release",
                    path.display()
                ),
            );
        }
        for problem in problems {
            report.error(AREA, problem);
        }
    }
    for (arch, url) in &feeds {
        native_feed(cx, Os::Linux, *arch, url, report);
    }
}

pub(super) fn macos(cx: &Context<'_>, config: &Config, macos: &MacosConfig, report: &mut Report) {
    const AREA: &str = "macos";
    let feed = required(
        macos.feed_url.as_deref(),
        AREA,
        "macos.feed-url",
        "set it to the https URL of your Sparkle appcast (SUFeedURL)",
        report,
    )
    .and_then(|text| check_url(cx, AREA, "macos.feed-url", text, report));

    let sandbox = match macos.sandbox.as_deref() {
        None => {
            report.warn(
                AREA,
                "macos.sandbox is not set; set non-sandboxed, sandboxed, or sandboxed-network-client so Sparkle's XPC services can be checked",
            );
            None
        }
        Some(text) => match SandboxMode::from_str(text, false) {
            Ok(mode) => Some(mode),
            Err(_) => {
                let what = if project::is_placeholder(text) {
                    "is still the placeholder"
                } else {
                    "is not one of non-sandboxed, sandboxed, sandboxed-network-client:"
                };
                report.error(AREA, format!("macos.sandbox {what} {text:?}"));
                None
            }
        },
    };

    let pins = match Pins::load() {
        Ok(pins) => pins,
        Err(e) => {
            report.error("sparkle", format!("{e:#}"));
            return;
        }
    };
    let pin = match pins.resolve(macos.sparkle_version.as_deref()) {
        Ok(pin) => {
            report.ok(
                "sparkle",
                format!("Sparkle {} (sha256:{})", pin.version, pin.sha256),
            );
            Some(pin)
        }
        Err(e) => {
            report.error("sparkle", format!("macos.sparkle-version: {e:#}"));
            None
        }
    };

    if macos.sparkle.is_none() && macos.sparkle_archive.is_none() && macos.app.is_none() {
        report.warn(
            "sparkle",
            "no Sparkle distribution is configured; run `gpui-auto-update sparkle fetch --out build/sparkle` and set macos.sparkle = \"build/sparkle\"",
        );
    }
    if let Some(dist) = &macos.sparkle {
        check_distribution(cx, dist, pin.map(|p| p.version.as_str()), &pins, report);
    }
    if let (Some(archive), Some(pin)) = (&macos.sparkle_archive, pin) {
        check_archive(cx, archive, &pin.sha256, pin.size, &pin.version, report);
    }
    if let Some(app) = &macos.app {
        check_app(cx, config, app, macos.feed_url.as_deref(), sandbox, report);
    }

    if let (Some(url), false) = (&feed, cx.args.offline) {
        if let Some(bytes) = fetch(cx, url, report) {
            match Document::parse(&bytes) {
                Ok(doc) if doc.detect_format() == Format::Sparkle => report.ok(
                    "feed",
                    format!("{url}: Sparkle appcast with {} items", doc.items.len()),
                ),
                Ok(_) => report.error(
                    "feed",
                    format!("{url} is a native feed, but macos.feed-url must be a Sparkle appcast"),
                ),
                Err(e) => report.error("feed", format!("{url} is not a usable appcast: {e}")),
            }
        }
    }
}

fn check_distribution(
    cx: &Context<'_>,
    dist: &Path,
    expected: Option<&str>,
    pins: &Pins,
    report: &mut Report,
) {
    const AREA: &str = "sparkle";
    let resolved = cx.project.resolve(dist);
    let version = match sparkle::distribution_version(&resolved) {
        Ok(version) => version,
        Err(_) => {
            report.error(
                AREA,
                format!(
                    "macos.sparkle {} is not a Sparkle distribution; run `gpui-auto-update sparkle fetch --out {}`",
                    dist.display(),
                    dist.display()
                ),
            );
            return;
        }
    };
    if pins.get(&version).is_none() {
        report.error(
            AREA,
            format!(
                "{} contains Sparkle {version}, which is not a pinned release",
                dist.display()
            ),
        );
    } else if expected.is_some_and(|e| e != version) {
        report.error(
            AREA,
            format!(
                "{} contains Sparkle {version}, but macos.sparkle-version selects {}",
                dist.display(),
                expected.unwrap_or_default()
            ),
        );
    } else {
        report.ok(
            AREA,
            format!("{} contains Sparkle {version}", dist.display()),
        );
    }
    for tool in ["sign_update", "generate_appcast"] {
        if !resolved.join("bin").join(tool).is_file() {
            report.error(
                AREA,
                format!(
                    "{} has no bin/{tool}, which `gpui-auto-update feed sparkle` needs; fetch a complete distribution",
                    dist.display()
                ),
            );
        }
    }
}

fn check_archive(
    cx: &Context<'_>,
    archive: &Path,
    sha256: &str,
    size: u64,
    version: &str,
    report: &mut Report,
) {
    const AREA: &str = "sparkle";
    let resolved = cx.project.resolve(archive);
    let mut bytes = Vec::new();
    let read =
        std::fs::File::open(&resolved).and_then(|f| f.take(size + 1).read_to_end(&mut bytes));
    if let Err(e) = read {
        report.error(
            AREA,
            format!(
                "macos.sparkle-archive {} cannot be read: {e}",
                archive.display()
            ),
        );
        return;
    }
    let actual: String = sha2::Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual == sha256 {
        report.ok(
            AREA,
            format!(
                "{} is the official Sparkle {version} archive",
                archive.display()
            ),
        );
    } else {
        report.error(
            AREA,
            format!(
                "checksum mismatch for {}: Sparkle {version} is pinned at sha256 {sha256}, the file has {actual}; \
                 download it again with `gpui-auto-update sparkle fetch`",
                archive.display()
            ),
        );
    }
}

fn check_app(
    cx: &Context<'_>,
    config: &Config,
    app: &Path,
    feed: Option<&str>,
    sandbox: Option<SandboxMode>,
    report: &mut Report,
) {
    const AREA: &str = "macos";
    let resolved = cx.project.resolve(app);
    let info = match sparkle::app_info_strings(&resolved) {
        Ok(info) => info,
        Err(e) => {
            report.error(AREA, format!("macos.app {}: {e:#}", app.display()));
            return;
        }
    };
    let mut same = |plist_key: &str, config_key: &str, expected: Option<&str>| {
        let (Some(expected), Some(actual)) = (expected, info.get(plist_key)) else {
            return;
        };
        if expected.trim() != actual.trim() {
            report.error(
                AREA,
                format!(
                    "{} has {plist_key} {actual:?}, but {config_key} is {expected:?}",
                    app.display()
                ),
            );
        }
    };
    same("CFBundleIdentifier", "app-id", config.app_id.as_deref());
    same("SUPublicEDKey", "public-key", config.public_key.as_deref());
    same("SUFeedURL", "macos.feed-url", feed);
    same(
        "CFBundleShortVersionString",
        "the package version",
        cx.version.as_deref(),
    );

    let Some(sandbox) = sandbox else {
        return;
    };
    match sparkle::check_unsigned_app(&resolved, sandbox) {
        Err(e) => report.error(AREA, format!("{e:#}")),
        Ok(findings) => {
            for finding in findings {
                let message = format!("{}: {}", finding.area, finding.message);
                if finding.error {
                    report.error(AREA, message);
                } else {
                    report.warn(AREA, message);
                }
            }
        }
    }
    report.note(
        AREA,
        format!(
            "code signatures are not checked here; run `gpui-auto-update sparkle validate --app {}` on the signed app",
            app.display()
        ),
    );
}

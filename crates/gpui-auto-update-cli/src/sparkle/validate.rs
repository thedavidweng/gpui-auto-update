//! Validation of a packaged application: Info.plist metadata, framework
//! placement, run-path resolution, sandbox requirements, license notice, and
//! code signatures.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use super::plist::{Dictionary, Value};
use anyhow::Result;
use base64::Engine as _;
use clap::Args;

use super::SandboxMode;
use super::bundle;
use super::macho;
use super::pins::Pins;

const SANDBOX_ENTITLEMENT: &str = "com.apple.security.app-sandbox";
const NETWORK_CLIENT_ENTITLEMENT: &str = "com.apple.security.network.client";
const MACH_LOOKUP_ENTITLEMENT: &str =
    "com.apple.security.temporary-exception.mach-lookup.global-name";
/// Sparkle clamps shorter scheduled-check intervals to one hour.
const MIN_CHECK_INTERVAL_SECS: f64 = 3600.0;

#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// The application bundle (`.app`) to validate.
    #[arg(long)]
    app: PathBuf,
    /// How the app is sandboxed. Read from the signed entitlements when
    /// omitted.
    #[arg(long, value_enum)]
    sandbox: Option<SandboxMode>,
    /// Skip code-signature checks, for example before signing.
    #[arg(long)]
    no_signature_checks: bool,
    /// Require Developer ID signatures with secure timestamps (release
    /// builds).
    #[arg(long, conflicts_with = "no_signature_checks")]
    require_developer_id: bool,
    /// CFBundleVersion of the previous release; the new build version must
    /// be greater.
    #[arg(long)]
    previous_build_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Error,
    Warning,
}

#[derive(Debug)]
struct Finding {
    severity: Severity,
    area: &'static str,
    message: String,
}

#[derive(Debug, Default)]
struct Report {
    findings: Vec<Finding>,
}

impl Report {
    fn error(&mut self, area: &'static str, message: impl Into<String>) {
        self.push(Severity::Error, area, message.into());
    }

    fn warn(&mut self, area: &'static str, message: impl Into<String>) {
        self.push(Severity::Warning, area, message.into());
    }

    fn push(&mut self, severity: Severity, area: &'static str, message: String) {
        let duplicate = self
            .findings
            .iter()
            .any(|f| f.severity == severity && f.area == area && f.message == message);
        if !duplicate {
            self.findings.push(Finding {
                severity,
                area,
                message,
            });
        }
    }

    fn count(&self, severity: Severity) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == severity)
            .count()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for finding in &self.findings {
            let label = match finding.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            writeln!(f, "{label}[{}]: {}", finding.area, finding.message)?;
        }
        let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
        write!(
            f,
            "{}, {}",
            plural(self.count(Severity::Error), "error"),
            plural(self.count(Severity::Warning), "warning")
        )
    }
}

pub fn run(args: ValidateArgs) -> Result<ExitCode> {
    let info = bundle::read_dict(&args.app.join("Contents/Info.plist"))?;
    let pins = Pins::load()?;
    let mut report = Report::default();

    let entitlements = if args.no_signature_checks {
        None
    } else {
        check_signatures(&args, &mut report)?
    };
    let sandbox = resolve_sandbox(args.sandbox, entitlements.as_ref(), &mut report);
    if let Some(mode) = sandbox {
        let source = if args.sandbox.is_some() {
            "declared"
        } else {
            "from entitlements"
        };
        println!("sandbox mode: {mode} ({source})");
    }

    let framework_minimum = check_framework(&args.app, &info, sandbox, &pins, &mut report);
    check_info(
        &info,
        framework_minimum.as_ref(),
        args.previous_build_version.as_deref(),
        sandbox,
        &mut report,
    );
    if let (Some(mode), Some(entitlements)) = (sandbox, &entitlements) {
        check_sandbox_entitlements(&info, mode, entitlements, &mut report);
    }
    check_executable(&args.app, &info, &mut report);
    check_license(&args.app, &mut report);

    println!("{report}");
    Ok(if report.count(Severity::Error) == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// One finding from [`check_unsigned_app`].
pub struct AppFinding {
    pub error: bool,
    pub area: &'static str,
    pub message: String,
}

/// The `validate` checks that do not need code signatures: Info.plist
/// metadata, framework placement, run paths, and the license notice.
pub fn check_unsigned_app(app: &Path, sandbox: SandboxMode) -> Result<Vec<AppFinding>> {
    let info = bundle::read_dict(&app.join("Contents/Info.plist"))?;
    let pins = Pins::load()?;
    let mut report = Report::default();
    let framework_minimum = check_framework(app, &info, Some(sandbox), &pins, &mut report);
    check_info(
        &info,
        framework_minimum.as_ref(),
        None,
        Some(sandbox),
        &mut report,
    );
    check_executable(app, &info, &mut report);
    check_license(app, &mut report);
    Ok(report
        .findings
        .into_iter()
        .map(|f| AppFinding {
            error: f.severity == Severity::Error,
            area: f.area,
            message: f.message,
        })
        .collect())
}

/// Dot-separated non-negative integers, one to three components.
fn numeric_version(s: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = s
        .split('.')
        .map(|p| {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                None
            } else {
                p.parse().ok()
            }
        })
        .collect();
    parts.filter(|p| (1..=3).contains(&p.len()))
}

fn compare_versions(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let len = a.len().max(b.len());
    let pad = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    (0..len)
        .map(|i| pad(a, i).cmp(&pad(b, i)))
        .find(|o| o.is_ne())
        .unwrap_or(std::cmp::Ordering::Equal)
}

pub fn is_reverse_dns(id: &str) -> bool {
    id.contains('.')
        && id.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// Sparkle reads booleans through `-boolValue`, so `YES`/`NO` strings work
/// as well as real booleans.
fn as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Boolean(b) => Some(*b),
        Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "yes" | "true" => Some(true),
            "no" | "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn bool_key(dict: &Dictionary, key: &str) -> bool {
    dict.get(key).and_then(as_bool).unwrap_or(false)
}

/// A framework minimum system version together with the Sparkle version it
/// came from, for messages.
struct FrameworkMinimum {
    version: Vec<u64>,
    text: String,
    sparkle: String,
}

fn check_info(
    info: &Dictionary,
    framework_minimum: Option<&FrameworkMinimum>,
    previous_build: Option<&str>,
    sandbox: Option<SandboxMode>,
    report: &mut Report,
) {
    const AREA: &str = "info-plist";
    let mut required = |key: &str| -> Option<String> {
        match info.get(key) {
            None => {
                report.error(AREA, format!("{key} is missing"));
                None
            }
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
            Some(_) => {
                report.error(AREA, format!("{key} must be a non-empty string"));
                None
            }
        }
    };
    let id = required("CFBundleIdentifier");
    let short = required("CFBundleShortVersionString");
    let build = required("CFBundleVersion");
    let feed = required("SUFeedURL");
    let key = required("SUPublicEDKey");
    let minimum = required("LSMinimumSystemVersion");
    required("CFBundleExecutable");

    if let Some(id) = id {
        if !is_reverse_dns(&id) {
            report.error(
                AREA,
                format!(
                    "CFBundleIdentifier {id:?} is not a reverse-DNS identifier (letters, digits, hyphens, and dots)"
                ),
            );
        }
    }
    if let Some(short) = short {
        if numeric_version(&short).is_none() {
            report.error(
                AREA,
                format!(
                    "CFBundleShortVersionString {short:?} must be one to three dot-separated integers, such as 1.4.2"
                ),
            );
        }
    }
    if let Some(build) = build {
        match numeric_version(&build) {
            None => report.error(
                AREA,
                format!(
                    "CFBundleVersion {build:?} must be one to three dot-separated integers so that Sparkle can order releases"
                ),
            ),
            Some(current) => {
                if let Some(previous) = previous_build {
                    match numeric_version(previous) {
                        None => report.error(
                            AREA,
                            format!("--previous-build-version {previous:?} is not a numeric version"),
                        ),
                        Some(prev) if compare_versions(&current, &prev).is_le() => report.error(
                            AREA,
                            format!(
                                "CFBundleVersion {build} must be greater than the previous release's {previous}"
                            ),
                        ),
                        Some(_) => {}
                    }
                }
            }
        }
    }
    if let Some(feed) = feed {
        let https =
            url::Url::parse(&feed).is_ok_and(|u| u.scheme() == "https" && u.host().is_some());
        if !https {
            report.error(
                AREA,
                format!("SUFeedURL must be an absolute https URL (got {feed:?})"),
            );
        }
    }
    if let Some(key) = key {
        match base64::engine::general_purpose::STANDARD.decode(key.trim()) {
            Err(_) => report.error(AREA, "SUPublicEDKey is not valid base64"),
            Ok(raw) if raw.len() != 32 => report.error(
                AREA,
                format!(
                    "SUPublicEDKey must decode to 32 bytes, an Ed25519 public key (got {} bytes)",
                    raw.len()
                ),
            ),
            Ok(_) => {}
        }
    }
    if info.contains_key("SUPublicDSAKeyFile") || info.contains_key("SUPublicDSAKey") {
        report.warn(
            AREA,
            "legacy DSA keys are configured; Sparkle 2 verifies updates with SUPublicEDKey",
        );
    }
    if let Some(minimum) = minimum {
        match numeric_version(&minimum) {
            None => report.error(
                AREA,
                format!("LSMinimumSystemVersion {minimum:?} is not a numeric version"),
            ),
            Some(v) => {
                if let Some(fw) = framework_minimum {
                    if compare_versions(&v, &fw.version).is_lt() {
                        report.error(
                            AREA,
                            format!(
                                "LSMinimumSystemVersion {minimum} is lower than {}, the minimum of the embedded Sparkle {}",
                                fw.text, fw.sparkle
                            ),
                        );
                    }
                }
            }
        }
    }

    for key in [
        "SUEnableAutomaticChecks",
        "SUAutomaticallyUpdate",
        "SUAllowsAutomaticUpdates",
        "SUEnableInstallerLauncherService",
        "SUEnableDownloaderService",
        "SUVerifyUpdateBeforeExtraction",
        "SURequireSignedFeed",
    ] {
        if info.get(key).is_some_and(|v| as_bool(v).is_none()) {
            report.error(AREA, format!("{key} must be a boolean"));
        }
    }
    match info.get("SUScheduledCheckInterval") {
        None => {}
        Some(v) => match v
            .as_number()
        {
            None => report.error(
                AREA,
                "SUScheduledCheckInterval must be a number of seconds",
            ),
            Some(secs) if secs < MIN_CHECK_INTERVAL_SECS => report.warn(
                AREA,
                format!(
                    "SUScheduledCheckInterval {secs} is below Sparkle's one-hour minimum and will be clamped"
                ),
            ),
            Some(_) => {}
        },
    }

    match sandbox {
        Some(SandboxMode::Sandboxed) | Some(SandboxMode::SandboxedNetworkClient) => {
            if !bool_key(info, "SUEnableInstallerLauncherService") {
                report.error(
                    AREA,
                    "sandboxed apps must set SUEnableInstallerLauncherService to true",
                );
            }
            if sandbox == Some(SandboxMode::Sandboxed)
                && !bool_key(info, "SUEnableDownloaderService")
            {
                report.error(
                    AREA,
                    "sandboxed apps without the com.apple.security.network.client entitlement must set SUEnableDownloaderService to true",
                );
            }
        }
        Some(SandboxMode::NonSandboxed) | None => {}
    }
}

fn check_framework(
    app: &Path,
    info: &Dictionary,
    sandbox: Option<SandboxMode>,
    pins: &Pins,
    report: &mut Report,
) -> Option<FrameworkMinimum> {
    const AREA: &str = "framework";
    let framework = bundle::embedded_framework(app);
    if !framework.is_dir() {
        report.error(
            AREA,
            "Contents/Frameworks/Sparkle.framework is missing; run `gpui-auto-update sparkle embed`",
        );
        return None;
    }
    let version_dir = match bundle::current_version_dir(&framework) {
        Ok(dir) => dir,
        Err(e) => {
            report.error(AREA, format!("{e:#}"));
            return None;
        }
    };
    for entry in std::fs::read_dir(&framework)
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = entry.path();
        if path.is_symlink() && !path.exists() {
            report.error(
                AREA,
                format!(
                    "Sparkle.framework/{} is a dangling symlink",
                    entry.file_name().to_string_lossy()
                ),
            );
        }
    }
    for item in ["Sparkle", "Autoupdate", "Updater.app"] {
        if !version_dir.join(item).exists() {
            report.error(
                AREA,
                format!("Sparkle.framework is incomplete: {item} is missing"),
            );
        }
    }

    let xpc = version_dir.join("XPCServices");
    let installer_needed = matches!(
        sandbox,
        Some(SandboxMode::Sandboxed | SandboxMode::SandboxedNetworkClient)
    ) || bool_key(info, "SUEnableInstallerLauncherService");
    let downloader_needed =
        sandbox == Some(SandboxMode::Sandboxed) || bool_key(info, "SUEnableDownloaderService");
    if installer_needed && !xpc.join("Installer.xpc").is_dir() {
        report.error(
            AREA,
            "XPCServices/Installer.xpc is missing; sandboxed apps (and SUEnableInstallerLauncherService) need it. Embed with --sandbox sandboxed",
        );
    }
    if downloader_needed && !xpc.join("Downloader.xpc").is_dir() {
        report.error(
            AREA,
            "XPCServices/Downloader.xpc is missing; sandboxed apps without network access (and SUEnableDownloaderService) need it",
        );
    }

    let fw_info = match bundle::read_dict(&version_dir.join("Resources/Info.plist")) {
        Ok(d) => d,
        Err(e) => {
            report.error(AREA, format!("{e:#}"));
            return None;
        }
    };
    if bundle::string(&fw_info, "CFBundleIdentifier") != Some("org.sparkle-project.Sparkle") {
        report.error(
            AREA,
            "the embedded framework is not Sparkle (CFBundleIdentifier is not org.sparkle-project.Sparkle)",
        );
    }
    let sparkle = bundle::string(&fw_info, "CFBundleShortVersionString")
        .unwrap_or("unknown")
        .to_owned();
    let pin = pins.get(&sparkle);
    if pin.is_none() {
        report.warn(
            AREA,
            format!(
                "Sparkle {sparkle} is not pinned by this tool; the current pin is {}",
                pins.default
            ),
        );
    } else if sparkle != pins.default {
        report.warn(
            AREA,
            format!(
                "Sparkle {sparkle} is embedded; the current maintained pin is {}",
                pins.default
            ),
        );
    }
    let text = bundle::string(&fw_info, "LSMinimumSystemVersion")
        .map(str::to_owned)
        .or_else(|| pin.map(|p| p.minimum_system_version.clone()))?;
    Some(FrameworkMinimum {
        version: numeric_version(&text)?,
        text,
        sparkle,
    })
}

fn check_executable(app: &Path, info: &Dictionary, report: &mut Report) {
    const AREA: &str = "rpath";
    let Some(name) = bundle::string(info, "CFBundleExecutable") else {
        return;
    };
    let exe = app.join("Contents/MacOS").join(name);
    let bytes = match std::fs::read(&exe) {
        Ok(b) => b,
        Err(e) => {
            report.error(AREA, format!("cannot read {}: {e}", exe.display()));
            return;
        }
    };
    let slices = match macho::parse(&bytes) {
        Ok(s) => s,
        Err(e) => {
            report.error(
                AREA,
                format!("{} is not a usable Mach-O executable: {e:#}", exe.display()),
            );
            return;
        }
    };
    let framework = bundle::embedded_framework(app);
    let (Ok(app_root), Ok(fw_root)) = (app.canonicalize(), framework.canonicalize()) else {
        return;
    };
    let exe_dir = exe.parent().expect("executable is inside Contents/MacOS");

    let mut linked = false;
    for slice in &slices {
        for dylib in slice
            .dylibs
            .iter()
            .filter(|d| d.contains("Sparkle.framework/"))
        {
            linked = true;
            if let Err(message) = resolve(dylib, &slice.rpaths, exe_dir, &app_root, &fw_root) {
                report.error(AREA, format!("[{}] {message}", slice.arch));
            }
        }
    }
    if !linked {
        report.warn(
            AREA,
            format!(
                "{name} does not link Sparkle.framework; if it loads Sparkle at runtime, it must load Contents/Frameworks/Sparkle.framework"
            ),
        );
    }
}

fn expand_loader_path(path: &str, exe_dir: &Path) -> Option<PathBuf> {
    for prefix in ["@executable_path", "@loader_path"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            return Some(exe_dir.join(rest.trim_start_matches('/')));
        }
    }
    path.starts_with('/').then(|| PathBuf::from(path))
}

/// Mirrors dyld's search: the first run path under which the library exists
/// wins, so it must be the embedded framework.
fn resolve(
    dylib: &str,
    rpaths: &[String],
    exe_dir: &Path,
    app_root: &Path,
    fw_root: &Path,
) -> std::result::Result<(), String> {
    let found = if let Some(rest) = dylib.strip_prefix("@rpath/") {
        rpaths
            .iter()
            .filter_map(|rp| expand_loader_path(rp, exe_dir))
            .map(|base| base.join(rest))
            .find(|candidate| candidate.exists())
            .ok_or_else(|| {
                format!(
                    "{dylib} does not resolve through any LC_RPATH ({}); link the executable with -Wl,-rpath,@executable_path/../Frameworks",
                    if rpaths.is_empty() {
                        "none set".to_owned()
                    } else {
                        rpaths.join(", ")
                    }
                )
            })?
    } else if dylib.starts_with('@') {
        let candidate = expand_loader_path(dylib, exe_dir)
            .ok_or_else(|| format!("unsupported Sparkle install name {dylib}"))?;
        if !candidate.exists() {
            return Err(format!("{dylib} does not exist inside the bundle"));
        }
        candidate
    } else {
        return Err(format!(
            "Sparkle is loaded from the absolute path {dylib}; it must be loaded through @rpath from Contents/Frameworks"
        ));
    };
    let found = found
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", found.display()))?;
    if found.starts_with(fw_root) {
        Ok(())
    } else if found.starts_with(app_root) {
        Err(format!(
            "{dylib} resolves to {}, not to Contents/Frameworks/Sparkle.framework",
            found.display()
        ))
    } else {
        Err(format!(
            "{dylib} resolves to {}, outside the app bundle; put @executable_path/../Frameworks first in the run paths",
            found.display()
        ))
    }
}

fn check_license(app: &Path, report: &mut Report) {
    let notice = app.join(bundle::LICENSE_NOTICE);
    let present = std::fs::metadata(&notice).is_ok_and(|m| m.is_file() && m.len() > 0);
    if !present {
        report.error(
            "license",
            format!(
                "Sparkle's license notice is missing at {}; `sparkle embed` copies it from the distribution",
                bundle::LICENSE_NOTICE
            ),
        );
    }
}

fn resolve_sandbox(
    declared: Option<SandboxMode>,
    entitlements: Option<&Dictionary>,
    report: &mut Report,
) -> Option<SandboxMode> {
    const AREA: &str = "sandbox";
    let Some(entitlements) = entitlements else {
        if declared.is_none() {
            report.error(
                AREA,
                "cannot tell whether the app is sandboxed: pass --sandbox, or validate the signed app so its entitlements can be read",
            );
        }
        return declared;
    };
    let sandboxed = bool_key(entitlements, SANDBOX_ENTITLEMENT);
    match declared {
        Some(mode) => {
            if (mode != SandboxMode::NonSandboxed) != sandboxed {
                report.error(
                    AREA,
                    format!(
                        "--sandbox {mode} contradicts the signed {SANDBOX_ENTITLEMENT} entitlement ({sandboxed})"
                    ),
                );
            }
            Some(mode)
        }
        None if !sandboxed => Some(SandboxMode::NonSandboxed),
        None if bool_key(entitlements, NETWORK_CLIENT_ENTITLEMENT) => {
            Some(SandboxMode::SandboxedNetworkClient)
        }
        None => Some(SandboxMode::Sandboxed),
    }
}

fn check_sandbox_entitlements(
    info: &Dictionary,
    mode: SandboxMode,
    entitlements: &Dictionary,
    report: &mut Report,
) {
    const AREA: &str = "sandbox";
    if mode == SandboxMode::NonSandboxed {
        return;
    }
    if mode == SandboxMode::SandboxedNetworkClient
        && !bool_key(entitlements, NETWORK_CLIENT_ENTITLEMENT)
    {
        report.error(
            AREA,
            format!("--sandbox {mode} requires the {NETWORK_CLIENT_ENTITLEMENT} entitlement"),
        );
    }
    let Some(id) = bundle::string(info, "CFBundleIdentifier") else {
        return;
    };
    let names: Vec<&str> = entitlements
        .get(MACH_LOOKUP_ENTITLEMENT)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_string).collect())
        .unwrap_or_default();
    for suffix in ["spks", "spki"] {
        let name = format!("{id}-{suffix}");
        if !names.contains(&name.as_str()) {
            report.error(
                AREA,
                format!("the {MACH_LOOKUP_ENTITLEMENT} entitlement must include {name}"),
            );
        }
    }
}

#[cfg(target_os = "macos")]
fn check_signatures(args: &ValidateArgs, report: &mut Report) -> Result<Option<Dictionary>> {
    use super::codesign;
    const AREA: &str = "signing";
    const DISABLE_LIBRARY_VALIDATION: &str = "com.apple.security.cs.disable-library-validation";

    let app_details = codesign::details(&args.app)?;
    let Some(app_sig) = app_details else {
        report.error(
            AREA,
            format!(
                "{} is not signed; run `gpui-auto-update sparkle sign`",
                args.app.display()
            ),
        );
        return Ok(None);
    };
    let entitlements = codesign::entitlements(&args.app)?.unwrap_or_default();
    let library_validation = app_sig.enforces_library_validation()
        && !bool_key(&entitlements, DISABLE_LIBRARY_VALIDATION);
    if library_validation && app_sig.is_adhoc() {
        report.error(
            AREA,
            "the app is signed ad hoc with the hardened runtime, so library validation will refuse to load Sparkle.framework; sign ad hoc without the hardened runtime (`sparkle sign --identity -` does this) or use a Developer ID identity",
        );
    }
    if bool_key(&entitlements, "com.apple.security.get-task-allow") {
        report.warn(
            AREA,
            "the app has the get-task-allow entitlement, which notarization rejects",
        );
    }

    let mut components: Vec<(String, PathBuf)> = Vec::new();
    let framework = bundle::embedded_framework(&args.app);
    if let Ok(version_dir) = bundle::current_version_dir(&framework) {
        for item in [
            "XPCServices/Installer.xpc",
            "XPCServices/Downloader.xpc",
            "Autoupdate",
            "Updater.app",
        ] {
            let path = version_dir.join(item);
            if path.exists() {
                let label = item.rsplit('/').next().unwrap_or(item).to_owned();
                components.push((label, path));
            }
        }
        components.push((bundle::FRAMEWORK_NAME.to_owned(), framework.clone()));
    }

    let mut all_signed = true;
    let check = |report: &mut Report, label: &str, sig: &codesign::SignatureInfo, is_app: bool| {
        let adhoc_app_for_testing = is_app && sig.is_adhoc() && !args.require_developer_id;
        if !sig.has_hardened_runtime() && !adhoc_app_for_testing {
            report.error(
                AREA,
                format!(
                    "{label} is not signed with the hardened runtime (codesign --options runtime)"
                ),
            );
        }
        // Helpers run as separate processes and Sparkle checks that they share
        // the app's signer; the framework only matters under library validation.
        let team_matters = !is_app && (library_validation || label != bundle::FRAMEWORK_NAME);
        if team_matters && sig.team() != app_sig.team() {
            report.error(
                AREA,
                format!(
                    "{label} is signed by team {} but the app by team {}; library validation requires the same team",
                    sig.team(),
                    app_sig.team()
                ),
            );
        }
        if args.require_developer_id {
            if !sig.is_developer_id() {
                report.error(
                    AREA,
                    format!("{label} is not signed with a Developer ID Application identity"),
                );
            }
            if sig.timestamp.is_none() {
                report.error(
                    AREA,
                    format!("{label} has no secure timestamp (codesign --timestamp)"),
                );
            }
        }
    };
    for (label, path) in &components {
        match codesign::details(path)? {
            Some(sig) => check(report, label, &sig, false),
            None => {
                all_signed = false;
                report.error(AREA, format!("{label} is not signed"));
            }
        }
    }
    let app_label = args
        .app
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "the app".to_owned());
    check(report, &app_label, &app_sig, true);
    if app_sig.is_adhoc() && !args.require_developer_id {
        report.warn(
            AREA,
            "the app is signed ad hoc: fine for local testing, but distribution needs a Developer ID signature and notarization",
        );
    }
    if all_signed {
        if let Err(message) = codesign::verify_strict(&args.app)? {
            report.error(
                AREA,
                format!("codesign --verify --deep --strict failed: {message}"),
            );
        }
    }
    Ok(Some(entitlements))
}

#[cfg(not(target_os = "macos"))]
fn check_signatures(args: &ValidateArgs, report: &mut Report) -> Result<Option<Dictionary>> {
    let message = "code-signature checks need macOS and were skipped";
    if args.require_developer_id {
        report.error("signing", message);
    } else {
        report.warn("signing", message);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_versions_have_one_to_three_integer_components() {
        assert_eq!(numeric_version("42"), Some(vec![42]));
        assert_eq!(numeric_version("1.2.3"), Some(vec![1, 2, 3]));
        assert_eq!(numeric_version("1.2.3.4"), None);
        assert_eq!(numeric_version("1.2-beta"), None);
        assert_eq!(numeric_version("1..2"), None);
        assert_eq!(numeric_version(""), None);
        assert_eq!(numeric_version("+1"), None);
    }

    #[test]
    fn versions_compare_numerically_with_missing_components_as_zero() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions(&[10], &[9]), Greater);
        assert_eq!(compare_versions(&[1, 0], &[1]), Equal);
        assert_eq!(compare_versions(&[1, 2], &[1, 10]), Less);
    }

    #[test]
    fn bundle_identifiers_are_reverse_dns() {
        assert!(is_reverse_dns("com.example.App-Beta"));
        assert!(!is_reverse_dns("App"));
        assert!(!is_reverse_dns("com..example"));
        assert!(!is_reverse_dns("com.example.app_name"));
    }

    #[test]
    fn info_rules_report_each_problem() {
        let mut info = Dictionary::new();
        info.insert("CFBundleIdentifier".into(), "com.example.App".into());
        info.insert("CFBundleExecutable".into(), "App".into());
        info.insert("CFBundleShortVersionString".into(), "1.0".into());
        info.insert("CFBundleVersion".into(), "7".into());
        info.insert("SUFeedURL".into(), "ftp://example.com/appcast.xml".into());
        info.insert("SUPublicEDKey".into(), "AAAA".into());
        info.insert("LSMinimumSystemVersion".into(), "12".into());
        info.insert("SUScheduledCheckInterval".into(), Value::Integer(60));
        info.insert("SUEnableDownloaderService".into(), "YES".into());
        let mut report = Report::default();
        check_info(
            &info,
            None,
            Some("7"),
            Some(SandboxMode::Sandboxed),
            &mut report,
        );
        let messages: Vec<&str> = report.findings.iter().map(|f| f.message.as_str()).collect();
        assert!(
            messages.iter().any(|m| m.contains("SUFeedURL")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("32 bytes")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("greater than")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|m| m.contains("SUEnableInstallerLauncherService")),
            "{messages:?}"
        );
        assert!(
            !messages
                .iter()
                .any(|m| m.contains("SUEnableDownloaderService")),
            "YES strings count as true: {messages:?}"
        );
        assert_eq!(report.count(Severity::Warning), 1, "{messages:?}");
    }

    #[test]
    fn the_sandbox_mode_is_inferred_from_entitlements() {
        let mut report = Report::default();
        let mut ents = Dictionary::new();
        assert_eq!(
            resolve_sandbox(None, Some(&ents), &mut report),
            Some(SandboxMode::NonSandboxed)
        );
        ents.insert(SANDBOX_ENTITLEMENT.into(), true.into());
        assert_eq!(
            resolve_sandbox(None, Some(&ents), &mut report),
            Some(SandboxMode::Sandboxed)
        );
        ents.insert(NETWORK_CLIENT_ENTITLEMENT.into(), true.into());
        assert_eq!(
            resolve_sandbox(None, Some(&ents), &mut report),
            Some(SandboxMode::SandboxedNetworkClient)
        );
        assert!(report.findings.is_empty());
        resolve_sandbox(Some(SandboxMode::NonSandboxed), Some(&ents), &mut report);
        assert_eq!(report.count(Severity::Error), 1);
    }
}

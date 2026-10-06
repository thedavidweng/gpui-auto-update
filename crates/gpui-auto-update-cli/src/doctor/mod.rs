//! `gpui-auto-update doctor`: validate a project's updater integration
//! without publishing anything.
//!
//! The configuration comes from the package's
//! `[package.metadata.gpui-auto-update]` table (docs/doctor.md). Local files
//! (Sparkle distributions, built artifacts, CI workflows) are inspected in
//! place, and published feeds are fetched and parsed unless `--offline` is
//! given. Artifact downloads and signature checks belong to `verify`.

mod artifacts;
mod ci;
mod platforms;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Args;
use gpui_auto_update_core::trust::TrustedKey;
use gpui_auto_update_core::version::ReleaseVersion;

use crate::keys::KeySource;
use crate::project::{self, Config, Project};

/// Validate the updater integration of an application package.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// The application's Cargo.toml.
    #[arg(long, value_name = "PATH", default_value = "Cargo.toml")]
    manifest_path: PathBuf,
    /// Do not fetch published feeds.
    #[arg(long)]
    offline: bool,
    /// Permit http:// feed URLs (local testing only).
    #[arg(long)]
    allow_http: bool,
    /// Check that the private key in this environment variable belongs to
    /// the configured public key. The key is never printed.
    #[arg(long, value_name = "VAR", conflicts_with = "key_file")]
    key_env: Option<String>,
    /// Check that the private key in this file belongs to the configured
    /// public key.
    #[arg(long, value_name = "PATH")]
    key_file: Option<PathBuf>,
    /// Also list every check that passed.
    #[arg(long, short)]
    verbose: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Ok,
    Note,
    Warning,
    Error,
}

struct Report {
    findings: Vec<(Level, &'static str, String)>,
    verbose: bool,
}

impl Report {
    fn ok(&mut self, area: &'static str, message: impl Into<String>) {
        self.findings.push((Level::Ok, area, message.into()));
    }

    fn note(&mut self, area: &'static str, message: impl Into<String>) {
        self.findings.push((Level::Note, area, message.into()));
    }

    fn warn(&mut self, area: &'static str, message: impl Into<String>) {
        self.findings.push((Level::Warning, area, message.into()));
    }

    fn error(&mut self, area: &'static str, message: impl Into<String>) {
        self.findings.push((Level::Error, area, message.into()));
    }

    fn count(&self, level: Level) -> usize {
        self.findings.iter().filter(|f| f.0 == level).count()
    }

    fn finish(self) -> ExitCode {
        for (level, area, message) in &self.findings {
            let label = match level {
                Level::Ok if self.verbose => "ok",
                Level::Note if self.verbose => "note",
                Level::Ok | Level::Note => continue,
                Level::Warning => "warning",
                Level::Error => "error",
            };
            println!("{label}[{area}]: {message}");
        }
        let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
        let errors = self.count(Level::Error);
        println!(
            "doctor: {}, {}",
            plural(errors, "error"),
            plural(self.count(Level::Warning), "warning")
        );
        if errors == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }
}

/// What every platform check needs to know.
struct Context<'a> {
    args: &'a DoctorArgs,
    project: &'a Project,
    /// The package version when it is strict SemVer.
    version: Option<String>,
}

pub fn run(args: DoctorArgs) -> Result<ExitCode> {
    let project = Project::load(&args.manifest_path)?;
    let mut report = Report {
        findings: Vec::new(),
        verbose: args.verbose,
    };

    let config = match &project.config {
        None => {
            report.error(
                "project",
                format!(
                    "{} has no {} table; run `gpui-auto-update init` to see what to configure",
                    project.manifest.display(),
                    project::SECTION
                ),
            );
            return Ok(report.finish());
        }
        Some(Err(message)) => {
            report.error("project", message.clone());
            return Ok(report.finish());
        }
        Some(Ok(config)) => config,
    };

    let version = check_package(&project, &mut report);
    check_app_id(config, &mut report);
    let key = check_public_key(config, &mut report);
    if let Some(key) = &key {
        check_key_pair(&args, key, &mut report);
    }

    let cx = Context {
        args: &args,
        project: &project,
        version,
    };
    if config.macos.is_none() && config.windows.is_none() && config.linux.is_none() {
        report.error(
            "project",
            "no platform is configured; add a macos, windows, or linux table (see `gpui-auto-update init`)",
        );
    }
    if let Some(macos) = &config.macos {
        platforms::macos(&cx, config, macos, &mut report);
    }
    if let Some(windows) = &config.windows {
        platforms::windows(&cx, windows, &mut report);
    }
    if let Some(linux) = &config.linux {
        platforms::linux(&cx, linux, &mut report);
    }
    ci::check(&project, &mut report);
    if !args.offline {
        report.note(
            "feed",
            "artifact downloads and signatures are not checked here; run `gpui-auto-update verify` on each published feed",
        );
    }
    Ok(report.finish())
}

fn check_package(project: &Project, report: &mut Report) -> Option<String> {
    const AREA: &str = "project";
    let name = project.name.as_deref().unwrap_or("(unnamed)");
    let version = match &project.version {
        Ok(version) => match ReleaseVersion::parse(version) {
            Ok(_) => {
                report.ok(AREA, format!("package {name} {version}"));
                Some(version.clone())
            }
            Err(e) => {
                report.error(
                    AREA,
                    format!(
                        "package version {version:?} is not strict SemVer, which the updater uses to order releases: {e}"
                    ),
                );
                None
            }
        },
        Err(message) => {
            report.error(AREA, message.clone());
            None
        }
    };
    if project.depends_on_facade {
        report.ok(AREA, "depends on gpui-auto-update");
    } else {
        report.warn(
            AREA,
            format!("{name} does not depend on gpui-auto-update; add it with `cargo add gpui-auto-update`"),
        );
    }
    version
}

/// Reports a missing or placeholder value and returns the usable one.
fn required<'a>(
    value: Option<&'a str>,
    area: &'static str,
    key: &str,
    help: &str,
    report: &mut Report,
) -> Option<&'a str> {
    match value {
        None => {
            report.error(area, format!("{key} is missing; {help}"));
            None
        }
        Some(v) if project::is_placeholder(v) => {
            report.error(
                area,
                format!("{key} is still the placeholder {v:?}; {help}"),
            );
            None
        }
        Some(v) => Some(v),
    }
}

fn check_app_id(config: &Config, report: &mut Report) {
    const AREA: &str = "app-id";
    let help =
        "set it to the application's reverse-DNS identifier (on macOS, its CFBundleIdentifier)";
    let Some(id) = required(config.app_id.as_deref(), AREA, "app-id", help, report) else {
        return;
    };
    if crate::sparkle::is_reverse_dns(id) {
        report.ok(AREA, format!("app-id {id}"));
    } else {
        report.error(
            AREA,
            format!(
                "app-id {id:?} is not a reverse-DNS identifier (letters, digits, hyphens, and dots)"
            ),
        );
    }
}

fn check_public_key(config: &Config, report: &mut Report) -> Option<TrustedKey> {
    const AREA: &str = "public-key";
    let help = "create a key pair with `gpui-auto-update keys generate` (or import one) and set public-key to the printed SUPublicEDKey";
    let text = required(
        config.public_key.as_deref(),
        AREA,
        "public-key",
        help,
        report,
    )?;
    let key = match TrustedKey::from_base64(text) {
        Ok(key) => key,
        Err(e) => {
            report.error(
                AREA,
                format!("public-key is not an Ed25519 public key: {e}; {help}"),
            );
            return None;
        }
    };
    if key.is_insecure_test_key() {
        report.error(
            AREA,
            "public-key is the insecure, publicly known test key; releases must use your own key pair",
        );
        return None;
    }
    report.ok(AREA, format!("public-key {}", key.to_base64()));
    Some(key)
}

fn check_key_pair(args: &DoctorArgs, key: &TrustedKey, report: &mut Report) {
    const AREA: &str = "public-key";
    let source = match (&args.key_env, &args.key_file) {
        (Some(var), _) => KeySource::Env(var.clone()),
        (None, Some(path)) => KeySource::File(path.clone()),
        (None, None) => {
            report.note(
                AREA,
                "the signing key was not checked; pass --key-env or --key-file to confirm it matches",
            );
            return;
        }
    };
    match source.read() {
        Err(message) => report.error(AREA, message),
        Ok(private) if private.public_key() == *key => {
            report.ok(
                AREA,
                format!("the private key from {} matches", source.describe()),
            );
        }
        Ok(private) => report.error(
            AREA,
            format!(
                "the private key from {} does not match the configured public-key \
                 (its public key is {}); feeds signed with it would be rejected",
                source.describe(),
                private.public_key().to_base64()
            ),
        ),
    }
}

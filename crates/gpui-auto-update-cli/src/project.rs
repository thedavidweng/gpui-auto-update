//! The consuming application's package, as `init` and `doctor` see it: its
//! Cargo manifest and the `[package.metadata.gpui-auto-update]` table that
//! records the updater configuration (see docs/doctor.md).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The metadata table that holds the configuration.
pub const SECTION: &str = "[package.metadata.gpui-auto-update]";

const FACADE: &str = "gpui-auto-update";

/// Updater configuration as written in the manifest. Every value stays text
/// so that `doctor` can explain each problem, including placeholders that
/// `init --write` left behind.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    pub app_id: Option<String>,
    pub public_key: Option<String>,
    pub macos: Option<MacosConfig>,
    pub windows: Option<WindowsConfig>,
    pub linux: Option<LinuxConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct MacosConfig {
    pub feed_url: Option<String>,
    pub sandbox: Option<String>,
    pub sparkle_version: Option<String>,
    /// A distribution extracted by `sparkle fetch`.
    pub sparkle: Option<PathBuf>,
    /// A downloaded official Sparkle archive.
    pub sparkle_archive: Option<PathBuf>,
    /// A built application bundle.
    pub app: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct WindowsConfig {
    pub strategy: Option<String>,
    #[serde(default)]
    pub feeds: BTreeMap<String, String>,
    #[serde(default)]
    pub artifacts: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct LinuxConfig {
    pub app_name: Option<String>,
    #[serde(default)]
    pub feeds: BTreeMap<String, String>,
    #[serde(default)]
    pub artifacts: BTreeMap<String, PathBuf>,
}

pub struct Project {
    pub manifest: PathBuf,
    /// Directory of the manifest; relative configured paths start here.
    pub root: PathBuf,
    pub name: Option<String>,
    /// The package version, or why it is unavailable.
    pub version: Result<String, String>,
    pub depends_on_facade: bool,
    /// `None` when the manifest has no configuration table; `Err` when the
    /// table cannot be read.
    pub config: Option<Result<Config, String>>,
}

impl Project {
    pub fn load(manifest: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(manifest)
            .with_context(|| format!("cannot read {}", manifest.display()))?;
        let doc: toml::Table = text
            .parse()
            .with_context(|| format!("{} is not valid TOML", manifest.display()))?;
        let Some(package) = doc.get("package").and_then(toml::Value::as_table) else {
            bail!(
                "{} has no [package]; point --manifest-path at the application's package",
                manifest.display()
            );
        };
        let root = manifest.parent().map(Path::to_path_buf).unwrap_or_default();
        let root = if root.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            root
        };
        let version = match package.get("version") {
            Some(toml::Value::String(v)) => Ok(v.clone()),
            Some(toml::Value::Table(t))
                if t.get("workspace") == Some(&toml::Value::Boolean(true)) =>
            {
                workspace_version(&root)
            }
            Some(_) => Err("package.version is not a string".to_owned()),
            None => Err("package.version is not set".to_owned()),
        };
        let config = package
            .get("metadata")
            .and_then(|m| m.get(FACADE))
            .map(|section| {
                section
                    .clone()
                    .try_into::<Config>()
                    .map_err(|e| format!("{SECTION} is invalid: {}", e.to_string().trim()))
            });
        Ok(Self {
            manifest: manifest.to_path_buf(),
            root,
            name: package
                .get("name")
                .and_then(toml::Value::as_str)
                .map(str::to_owned),
            version,
            depends_on_facade: depends_on_facade(&doc),
            config,
        })
    }

    /// A configured path, relative to the package directory unless absolute.
    pub fn resolve(&self, path: &Path) -> PathBuf {
        self.root.join(path)
    }
}

/// Values `init --write` emits that a developer must replace.
pub fn is_placeholder(value: &str) -> bool {
    let value = value.trim();
    value.starts_with('<') && value.ends_with('>')
}

fn workspace_version(root: &Path) -> Result<String, String> {
    for dir in root.ancestors().skip(1) {
        let manifest = dir.join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let Ok(doc) = text.parse::<toml::Table>() else {
            continue;
        };
        if let Some(workspace) = doc.get("workspace") {
            return workspace
                .get("package")
                .and_then(|p| p.get("version"))
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!(
                        "package.version is inherited, but {} sets no workspace.package.version",
                        manifest.display()
                    )
                });
        }
    }
    Err("package.version is inherited from a workspace that was not found".to_owned())
}

fn depends_on_facade(doc: &toml::Table) -> bool {
    let mut tables: Vec<&toml::Value> = ["dependencies"]
        .iter()
        .filter_map(|k| doc.get(*k))
        .collect();
    if let Some(targets) = doc.get("target").and_then(toml::Value::as_table) {
        tables.extend(targets.values().filter_map(|t| t.get("dependencies")));
    }
    tables
        .iter()
        .filter_map(|t| t.as_table())
        .flat_map(|t| t.iter())
        .any(|(name, spec)| {
            let package = spec.get("package").and_then(toml::Value::as_str);
            package.unwrap_or(name) == FACADE
        })
}

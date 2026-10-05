//! Paths and property-list access for `.app` bundles and `Sparkle.framework`.

use std::path::{Path, PathBuf};

use super::plist::{self, Dictionary, Value};
use anyhow::{Context, Result, bail};

pub const FRAMEWORK_NAME: &str = "Sparkle.framework";

/// Where Sparkle's license notice is placed inside an application bundle.
pub const LICENSE_NOTICE: &str = "Contents/Resources/ThirdPartyNotices/Sparkle/LICENSE";

pub fn read_dict(path: &Path) -> Result<Dictionary> {
    plist::read_file(path)
}

pub fn string<'a>(dict: &'a Dictionary, key: &str) -> Option<&'a str> {
    dict.get(key).and_then(Value::as_string)
}

/// Embedded framework location inside an application bundle.
pub fn embedded_framework(app: &Path) -> PathBuf {
    app.join("Contents/Frameworks").join(FRAMEWORK_NAME)
}

/// The framework's current version directory (`Versions/B` for Sparkle 2),
/// resolved through `Versions/Current`.
pub fn current_version_dir(framework: &Path) -> Result<PathBuf> {
    let current = framework.join("Versions/Current");
    let target = std::fs::read_link(&current)
        .with_context(|| format!("{} is not a symlink", current.display()))?;
    let dir = framework.join("Versions").join(&target);
    if target.components().count() != 1 || !dir.is_dir() {
        bail!(
            "{} points to {}, which is not a version directory",
            current.display(),
            target.display()
        );
    }
    Ok(dir)
}

pub fn framework_info(framework: &Path) -> Result<Dictionary> {
    read_dict(&current_version_dir(framework)?.join("Resources/Info.plist"))
}

pub fn framework_version(framework: &Path) -> Result<String> {
    let info = framework_info(framework)?;
    string(&info, "CFBundleShortVersionString")
        .map(str::to_owned)
        .with_context(|| format!("{} has no CFBundleShortVersionString", framework.display()))
}

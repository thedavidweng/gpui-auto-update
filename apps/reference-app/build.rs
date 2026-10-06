//! Embeds a Windows version resource whose `ProductVersion` is the build's
//! `REFERENCE_APP_VERSION`.
//!
//! The Windows backend refuses an artifact whose embedded version differs
//! from its feed entry, so a portable update to this executable only works
//! when the resource carries the exact release version.

use std::env;
use std::fs;
use std::path::PathBuf;

/// Must match `build_config::DEFAULT_VERSION`.
const DEFAULT_VERSION: &str = "1.0.0";

fn main() {
    println!("cargo:rerun-if-env-changed=REFERENCE_APP_VERSION");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = env::var("REFERENCE_APP_VERSION")
        .ok()
        .map(|version| version.trim().to_owned())
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| DEFAULT_VERSION.to_owned());
    // Anything else is not a version; the app reports the bad configuration
    // when it starts, so leave the executable without a version resource.
    if !version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
    {
        println!(
            "cargo:warning=REFERENCE_APP_VERSION {version:?} is not a version; no version resource embedded"
        );
        return;
    }

    let numeric = numeric_version(&version);
    let rc = format!(
        r#"#pragma code_page(65001)
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "gpui-auto-update"
      VALUE "FileDescription", "gpui-auto-update reference application"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "reference-app"
      VALUE "OriginalFilename", "reference-app.exe"
      VALUE "ProductName", "Reference App"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let rc_path = out.join("reference-app-version.rc");
    fs::write(&rc_path, rc).expect("write the version resource script");
    embed_resource::compile_for(&rc_path, ["reference-app"], embed_resource::NONE)
        .manifest_optional()
        .expect("compile the version resource");
}

/// `major,minor,patch,0` for the fixed-size version fields, which cannot
/// hold pre-release or build metadata.
fn numeric_version(version: &str) -> String {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let mut parts: Vec<u16> = core
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .take(3)
        .collect();
    parts.resize(4, 0);
    parts
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

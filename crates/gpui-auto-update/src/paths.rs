//! Where the automatic-update preference is stored on each platform.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use gpui_auto_update_core::{ErrorKind, UpdateError};

const DIRECTORY: &str = "gpui-auto-update";
const FILE_NAME: &str = "preferences.json";

/// The per-user file in which the automatic-update preference of the
/// application `app_id` is stored by default:
///
/// | Platform | Path |
/// | --- | --- |
/// | macOS | `~/Library/Application Support/<app_id>/gpui-auto-update/preferences.json` |
/// | Windows | `%LOCALAPPDATA%\<app_id>\gpui-auto-update\preferences.json` |
/// | Linux and other Unix | `$XDG_CONFIG_HOME/<app_id>/gpui-auto-update/preferences.json`, with `XDG_CONFIG_HOME` defaulting to `~/.config` |
///
/// On macOS, applications using the Sparkle backend keep the preference in
/// Sparkle's user defaults instead; this file is used only when the library
/// owns the preference.
///
/// Fails with [`ErrorKind::Configuration`] when `app_id` is empty or could
/// escape its directory (it contains a path separator, is `.` or `..`, or
/// starts with `.`), or when the platform's base directory is unknown.
/// Reads environment variables only; never touches the filesystem.
pub fn default_preferences_path(app_id: &str) -> Result<PathBuf, UpdateError> {
    let os = if cfg!(target_os = "macos") {
        Os::Mac
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Unix
    };
    preferences_path(os, &|name| std::env::var_os(name), app_id)
}

#[derive(Clone, Copy, Debug)]
enum Os {
    Mac,
    Windows,
    Unix,
}

fn preferences_path(
    os: Os,
    var: &dyn Fn(&str) -> Option<OsString>,
    app_id: &str,
) -> Result<PathBuf, UpdateError> {
    validate_app_id(app_id)?;
    let absolute = |name: &str| {
        var(name).map(PathBuf::from).filter(|path| {
            path.is_absolute() || matches!(os, Os::Windows) && is_windows_absolute(path)
        })
    };
    let base = match os {
        Os::Mac => absolute("HOME").map(|home| home.join("Library").join("Application Support")),
        Os::Windows => absolute("LOCALAPPDATA").or_else(|| {
            absolute("USERPROFILE").map(|profile| profile.join("AppData").join("Local"))
        }),
        Os::Unix => absolute("XDG_CONFIG_HOME")
            .or_else(|| absolute("HOME").map(|home| home.join(".config"))),
    };
    let base = base.ok_or_else(|| {
        UpdateError::new(ErrorKind::Configuration).with_diagnostic(format!(
            "no per-user base directory is known for update preferences on {os:?}"
        ))
    })?;
    Ok(base.join(app_id).join(DIRECTORY).join(FILE_NAME))
}

/// `C:\...` or `\\server\...`, which a non-Windows host does not consider
/// absolute.
fn is_windows_absolute(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/'))
        || text.starts_with(r"\\")
}

fn validate_app_id(app_id: &str) -> Result<(), UpdateError> {
    let invalid = app_id.is_empty()
        || app_id.starts_with('.')
        || app_id
            .chars()
            .any(|c| matches!(c, '/' | '\\' | ':') || c.is_control());
    if invalid {
        return Err(UpdateError::new(ErrorKind::Configuration)
            .with_diagnostic(format!("invalid application identifier {app_id:?}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn macos_uses_application_support() {
        let path = preferences_path(Os::Mac, &env(&[("HOME", "/Users/ada")]), "com.example.App");
        assert_eq!(
            path.unwrap(),
            PathBuf::from(
                "/Users/ada/Library/Application Support/com.example.App/gpui-auto-update/preferences.json"
            )
        );
    }

    #[test]
    fn unix_prefers_xdg_config_home() {
        let vars = env(&[("HOME", "/home/ada"), ("XDG_CONFIG_HOME", "/xdg")]);
        assert_eq!(
            preferences_path(Os::Unix, &vars, "com.example.App").unwrap(),
            PathBuf::from("/xdg/com.example.App/gpui-auto-update/preferences.json")
        );
    }

    #[test]
    fn unix_ignores_relative_xdg_config_home() {
        let vars = env(&[("HOME", "/home/ada"), ("XDG_CONFIG_HOME", "relative")]);
        assert_eq!(
            preferences_path(Os::Unix, &vars, "app").unwrap(),
            PathBuf::from("/home/ada/.config/app/gpui-auto-update/preferences.json")
        );
    }

    #[test]
    fn windows_uses_local_app_data() {
        let vars = env(&[("LOCALAPPDATA", r"C:\Users\ada\AppData\Local")]);
        assert_eq!(
            preferences_path(Os::Windows, &vars, "com.example.App").unwrap(),
            PathBuf::from(r"C:\Users\ada\AppData\Local")
                .join("com.example.App")
                .join("gpui-auto-update")
                .join("preferences.json")
        );
    }

    #[test]
    fn windows_falls_back_to_user_profile() {
        let vars = env(&[("USERPROFILE", r"C:\Users\ada")]);
        assert_eq!(
            preferences_path(Os::Windows, &vars, "app").unwrap(),
            PathBuf::from(r"C:\Users\ada")
                .join("AppData")
                .join("Local")
                .join("app")
                .join("gpui-auto-update")
                .join("preferences.json")
        );
    }

    #[test]
    fn missing_base_directory_is_a_configuration_error() {
        let error = preferences_path(Os::Unix, &env(&[]), "app").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Configuration);
    }

    #[test]
    fn app_ids_that_could_escape_are_rejected() {
        let vars = env(&[("HOME", "/home/ada")]);
        for app_id in ["", ".", "..", ".hidden", "a/b", r"a\b", "C:x"] {
            let error = preferences_path(Os::Unix, &vars, app_id).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Configuration, "{app_id:?}");
        }
    }
}

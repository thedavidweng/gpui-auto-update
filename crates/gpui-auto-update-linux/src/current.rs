//! Detection inputs gathered from the running Linux process.

use std::path::PathBuf;

use crate::detect::{Detection, DetectionInputs, detect};
use crate::proc_status;

impl DetectionInputs {
    /// Gathers the inputs for the running process: its effective uid (from
    /// `/proc/self/status`), executable, `$HOME`, build architecture, and the
    /// real filesystem root.
    ///
    /// Values that cannot be read are filled in so that detection denies
    /// self-update: an unreadable uid is treated as root, and an unreadable
    /// executable path as unresolvable.
    pub fn current(app_name: impl Into<String>) -> Self {
        let euid = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| proc_status::effective_uid(&status))
            .unwrap_or(0);
        Self {
            app_name: app_name.into(),
            arch: std::env::consts::ARCH.to_owned(),
            euid,
            executable: std::env::current_exe().unwrap_or_default(),
            home: std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(PathBuf::from),
            root: PathBuf::from("/"),
        }
    }
}

/// Detects the capability of the running installation of `app_name`.
pub fn detect_current(app_name: impl Into<String>) -> Detection {
    detect(&DetectionInputs::current(app_name))
}

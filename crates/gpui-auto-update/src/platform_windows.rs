//! The Windows backend behind the facade's [`UpdateBackend`] contract.

use gpui_auto_update_core::{AvailableUpdate, Capability, UpdateError};
use gpui_auto_update_windows::{WindowsBackend, WindowsHandoff, WindowsUpdateConfig};

use crate::backend::{Handoff, ProgressSink, UpdateBackend};
use crate::config::UpdaterConfig;

impl UpdateBackend for WindowsBackend {
    fn capability(&self) -> Capability {
        WindowsBackend::capability(self)
    }

    fn stage(&self, _update: &AvailableUpdate, progress: &ProgressSink) -> Result<(), UpdateError> {
        WindowsBackend::stage(self, progress.coordinator())
    }

    fn install(
        &self,
        _update: &AvailableUpdate,
        _progress: &ProgressSink,
    ) -> Result<Handoff, UpdateError> {
        WindowsBackend::install(self).map(handoff)
    }

    fn relaunch(&self) -> Result<Handoff, UpdateError> {
        self.relaunch_handoff().map(handoff)
    }
}

fn handoff(handoff: WindowsHandoff) -> Handoff {
    match handoff {
        WindowsHandoff::Restart { executable } => Handoff::Restart {
            restart_path: Some(executable),
        },
        // A running installer owns relaunching, so anything else must quit
        // without restarting the old executable next to it.
        _ => Handoff::Quit,
    }
}

impl UpdaterConfig {
    /// The default Windows configuration: checks the declared
    /// architecture's signed feed and installs with the Windows backend.
    ///
    /// The backend serves as both the check source and the
    /// [`UpdateBackend`], so staging always downloads the release the last
    /// check selected. Fails with a configuration error when `config` has
    /// no feed for the selected architecture.
    pub fn windows(
        app_id: impl Into<String>,
        config: WindowsUpdateConfig,
    ) -> Result<Self, UpdateError> {
        let backend = WindowsBackend::new(config)?;
        Ok(Self::new(app_id, backend.check_source()).with_backend(backend))
    }
}

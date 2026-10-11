//! A stand-in for unsaved user work, saved before the updater restarts.

use std::path::PathBuf;

use gpui::{App, AppContext as _, Context, Entity, Subscription, Task};
use gpui_auto_update::{PrepareError, Updater};

/// A document whose only content is how many edits were made.
pub struct Document {
    path: PathBuf,
    edits: u32,
    saved_edits: u32,
}

impl Document {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            edits: 0,
            saved_edits: 0,
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn edits(&self) -> u32 {
        self.edits
    }

    pub fn is_dirty(&self) -> bool {
        self.edits != self.saved_edits
    }

    pub fn edit(&mut self, cx: &mut Context<Self>) {
        self.edits += 1;
        cx.notify();
    }

    /// Writes the document on the background executor.
    pub fn save(&mut self, cx: &mut Context<Self>) -> Task<Result<(), PrepareError>> {
        let path = self.path.clone();
        let edits = self.edits;
        let write = cx.background_spawn(async move {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, format!("edits: {edits}\n"))
        });
        cx.spawn(async move |this, cx| {
            write.await?;
            this.update(cx, |this, cx| {
                this.saved_edits = edits;
                cx.notify();
            })?;
            Ok(())
        })
    }
}

/// Saves `document` whenever `updater` is about to install or relaunch,
/// for as long as the returned subscription lives.
pub fn save_before_install(
    updater: &Entity<Updater>,
    document: &Entity<Document>,
    cx: &mut App,
) -> Subscription {
    let document = document.downgrade();
    updater.update(cx, |updater, _| {
        updater.on_prepare_to_install(move |cx| match document.upgrade() {
            Some(document) => document.update(cx, |document, cx| document.save(cx)),
            None => Task::ready(Ok(())),
        })
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use gpui::TestAppContext;
    use gpui_auto_update::core::{
        AvailableUpdate, Capability, CheckOutcome, CheckRequest, CheckSource,
        MemoryPreferenceStore, UpdateError, UpdateState,
    };
    use gpui_auto_update::{
        BuildProfile, Handoff, ProgressSink, RestartToUpdate, UpdateBackend, UpdaterConfig,
    };

    use super::*;

    struct NewRelease;

    impl CheckSource for NewRelease {
        fn check(&self, _: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
            Ok(CheckOutcome::UpdateAvailable(AvailableUpdate::new("2.0.0")))
        }
    }

    /// Records what the document file contained when installation began.
    #[derive(Clone)]
    struct RecordingBackend {
        document: PathBuf,
        seen_at_install: Arc<Mutex<Option<String>>>,
    }

    impl UpdateBackend for RecordingBackend {
        fn capability(&self) -> Capability {
            Capability::SelfManaged
        }

        fn stage(&self, _: &AvailableUpdate, _: &ProgressSink) -> Result<(), UpdateError> {
            Ok(())
        }

        fn install(&self, _: &AvailableUpdate, _: &ProgressSink) -> Result<Handoff, UpdateError> {
            *self.seen_at_install.lock().unwrap() = std::fs::read_to_string(&self.document).ok();
            Ok(Handoff::BackendOwned)
        }
    }

    #[gpui::test]
    fn unsaved_edits_are_written_before_the_update_installs(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("document.txt");
        let backend = RecordingBackend {
            document: path.clone(),
            seen_at_install: Arc::default(),
        };
        let config = UpdaterConfig::new("dev.example.document", NewRelease)
            .with_backend(backend.clone())
            .with_preferences(MemoryPreferenceStore::new())
            .with_build_profile(BuildProfile::Release);
        let updater = cx.update(|cx| gpui_auto_update::init(config, cx));
        let document = cx.new(|_| Document::new(path.clone()));
        let _save = cx.update(|cx| save_before_install(&updater, &document, cx));
        cx.run_until_parked();

        document.update(cx, |d, cx| {
            d.edit(cx);
            d.edit(cx);
        });
        assert!(document.read_with(cx, |d, _| d.is_dirty()));

        updater.update(cx, |u, cx| u.check_for_updates(cx));
        cx.run_until_parked();
        updater.update(cx, |u, cx| u.request_install(cx)).unwrap();
        cx.run_until_parked();
        assert!(matches!(
            updater.read_with(cx, |u, _| u.state()),
            UpdateState::Staged(_)
        ));
        assert!(!path.exists(), "staging does not save the document");

        cx.update(|cx| cx.dispatch_action(&RestartToUpdate));
        cx.run_until_parked();

        assert_eq!(
            backend.seen_at_install.lock().unwrap().as_deref(),
            Some("edits: 2\n")
        );
        assert!(!document.read_with(cx, |d, _| d.is_dirty()));
    }
}

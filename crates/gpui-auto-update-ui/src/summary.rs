//! What the controls show, derived from the updater alone.

use gpui::SharedString;
use gpui_auto_update::core::{Capability, ReleaseNotes, UpdateState};
use gpui_auto_update::{PreviewState, Updater};

/// An operation a control offers for the current update state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UpdateAction {
    /// Run a manual check with visible feedback.
    CheckForUpdates,
    /// Download, verify, and stage the available update.
    Download,
    /// Install the staged update and restart, or restart after an install
    /// that waits for the application to quit.
    RestartToUpdate,
    /// Dismiss the current outcome.
    Dismiss,
}

impl UpdateAction {
    /// The short visible label.
    pub fn label(self) -> &'static str {
        match self {
            Self::CheckForUpdates => "Check for Updates",
            Self::Download => "Download Update",
            Self::RestartToUpdate => "Restart to Update",
            Self::Dismiss => "Dismiss",
        }
    }

    /// A stable element id for the control that performs this action.
    pub fn element_id(self) -> &'static str {
        match self {
            Self::CheckForUpdates => "update-check",
            Self::Download => "update-download",
            Self::RestartToUpdate => "update-restart",
            Self::Dismiss => "update-dismiss",
        }
    }
}

/// How the current state should be emphasized. Controls map tones to the
/// application's colors; they never encode meaning in color alone, because
/// the headline always says the same thing in words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tone {
    /// Nothing needs the user's attention.
    Neutral,
    /// A good outcome: up to date, or an update finished.
    Positive,
    /// The user can act: an update is available or ready to install.
    Attention,
    /// Something went wrong.
    Critical,
}

/// Download or install progress.
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateProgress {
    /// Completed fraction in `0.0..=1.0`, or `None` when progress cannot be
    /// measured (an unknown download size, verification, or installation).
    pub fraction: Option<f64>,
    /// The progress in words, for display next to a bar and for assistive
    /// technology.
    pub label: SharedString,
}

/// Everything the neutral controls show, computed from an [`Updater`].
///
/// This is plain data: applications that render their own update UI can use
/// it as a starting point, or ignore it and read [`Updater::state`]
/// directly.
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateSummary {
    /// One sentence saying what the updater is doing.
    pub headline: SharedString,
    /// Supporting text: release notes, an error message, or advice.
    pub detail: Option<SharedString>,
    /// Download or install progress, while something is in progress.
    pub progress: Option<UpdateProgress>,
    /// How to emphasize the state.
    pub tone: Tone,
    /// The operations offered for the state, most important first.
    pub actions: Vec<UpdateAction>,
    /// The one action an unobtrusive "update available" affordance should
    /// offer, when the user has something to act on.
    pub attention: Option<UpdateAction>,
    /// The version of the update the state refers to, if any.
    pub version: Option<SharedString>,
    /// The automatic-updates setting, once startup finished and it can be
    /// changed (never while previewing).
    pub automatic_updates: Option<bool>,
    /// Why this build will not install updates, when it will not.
    pub install_notice: Option<SharedString>,
    /// What went wrong with the previous update attempt after the
    /// application had quit — for example a new version that did not start,
    /// after which the previous version was restored (see
    /// [`Updater::previous_update_failure`]). Never set while previewing.
    pub previous_update_failure: Option<SharedString>,
    /// The preview being shown instead of the real state, if any.
    pub preview: Option<PreviewState>,
}

impl UpdateSummary {
    /// Summarizes `updater`. `current_version`, when given, is named in
    /// the idle and up-to-date headlines.
    pub fn new(updater: &Updater, current_version: Option<&str>) -> Self {
        use UpdateAction::*;

        let mut detail = None;
        let mut progress = None;
        let mut tone = Tone::Neutral;
        let mut version = None;
        let (headline, actions): (String, Vec<UpdateAction>) = match updater.state() {
            UpdateState::Disabled { capability } => {
                let (headline, advice) = disabled(&capability);
                detail = advice;
                (headline, vec![CheckForUpdates])
            }
            UpdateState::Idle => (
                match current_version {
                    Some(current) => format!("Version {current}"),
                    None => "Updates".to_owned(),
                },
                vec![CheckForUpdates],
            ),
            UpdateState::Checking => ("Checking for updates…".to_owned(), vec![]),
            UpdateState::UpToDate => {
                tone = Tone::Positive;
                (
                    match current_version {
                        Some(current) => format!("Version {current} is up to date."),
                        None => "This app is up to date.".to_owned(),
                    },
                    vec![CheckForUpdates, Dismiss],
                )
            }
            UpdateState::Available(update) => {
                tone = Tone::Attention;
                detail = notes(update.release_notes.as_ref());
                version = Some(update.version.clone());
                (
                    format!("Version {} is available.", update.version),
                    vec![Download, Dismiss],
                )
            }
            UpdateState::Downloading {
                update,
                progress: download,
            } => {
                const MIB: f64 = 1024.0 * 1024.0;
                let downloaded = download.downloaded as f64 / MIB;
                let label = match download.total {
                    Some(total) => format!("{downloaded:.1} of {:.1} MiB", total as f64 / MIB),
                    None => format!("{downloaded:.1} MiB"),
                };
                progress = Some(UpdateProgress {
                    fraction: download.fraction(),
                    label: label.into(),
                });
                version = Some(update.version.clone());
                (format!("Downloading version {}…", update.version), vec![])
            }
            UpdateState::Verifying(update) => {
                progress = Some(indeterminate("Verifying the download"));
                version = Some(update.version.clone());
                (format!("Verifying version {}…", update.version), vec![])
            }
            UpdateState::Staged(update) => {
                tone = Tone::Attention;
                version = Some(update.version.clone());
                (
                    format!("Version {} is ready to install.", update.version),
                    vec![RestartToUpdate],
                )
            }
            UpdateState::Installing(update) => {
                progress = Some(indeterminate("Installing"));
                version = Some(update.version.clone());
                (format!("Installing version {}…", update.version), vec![])
            }
            UpdateState::WaitingForQuit(update) => {
                tone = Tone::Attention;
                version = Some(update.version.clone());
                (
                    format!("Restart to finish updating to version {}.", update.version),
                    vec![RestartToUpdate],
                )
            }
            UpdateState::Relaunching(update) => {
                progress = Some(indeterminate("Restarting"));
                version = Some(update.version.clone());
                (
                    format!("Restarting into version {}…", update.version),
                    vec![],
                )
            }
            UpdateState::RolledBack { update, error } => {
                tone = Tone::Critical;
                detail = Some(error.message().to_owned());
                version = Some(update.version.clone());
                (
                    format!("Version {} was rolled back.", update.version),
                    vec![CheckForUpdates, Dismiss],
                )
            }
            UpdateState::Completed(update) => {
                tone = Tone::Positive;
                version = Some(update.version.clone());
                (
                    format!("Updated to version {}.", update.version),
                    vec![RestartToUpdate, Dismiss],
                )
            }
            UpdateState::Failed(error) => {
                tone = Tone::Critical;
                detail = Some(error.message().to_owned());
                (
                    "The update failed.".to_owned(),
                    vec![CheckForUpdates, Dismiss],
                )
            }
            state => (format!("{state:?}"), vec![CheckForUpdates]),
        };

        let attention = [Download, RestartToUpdate]
            .into_iter()
            .find(|action| tone == Tone::Attention && actions.contains(action));

        let previewing = updater.is_preview();
        let install_notice = (!previewing
            && updater.is_self_update_supported()
            && !updater.installs_allowed())
        .then(|| "This development build checks for updates but does not install them.".into());

        Self {
            headline: headline.into(),
            detail: detail.map(Into::into),
            progress,
            tone,
            actions,
            attention,
            version: version.map(Into::into),
            automatic_updates: (updater.is_ready() && !previewing)
                .then(|| updater.automatic_checks_enabled()),
            install_notice,
            previous_update_failure: if previewing {
                None
            } else {
                updater
                    .previous_update_failure()
                    .map(|error| error.message().to_owned().into())
            },
            preview: updater.preview(),
        }
    }

    /// A self-contained description of `action` for assistive technology
    /// and tooltips, naming the version it acts on.
    pub fn accessible_label(&self, action: UpdateAction) -> SharedString {
        match (action, &self.version) {
            (UpdateAction::Download, Some(version)) => format!("Download version {version}"),
            (UpdateAction::RestartToUpdate, Some(version)) => {
                format!("Restart to update to version {version}")
            }
            (UpdateAction::Dismiss, _) => format!("Dismiss: {}", self.headline),
            (action, _) => action.label().to_owned(),
        }
        .into()
    }
}

fn indeterminate(label: &'static str) -> UpdateProgress {
    UpdateProgress {
        fraction: None,
        label: label.into(),
    }
}

fn disabled(capability: &Capability) -> (String, Option<String>) {
    match capability {
        Capability::ExternallyManaged { manager } => {
            let manager = manager.as_deref().unwrap_or("your package manager");
            (
                format!("Updates are managed by {manager}."),
                Some(format!("Use {manager} to update this app.")),
            )
        }
        Capability::Unsupported => (
            "This installation cannot update itself.".to_owned(),
            Some("Install a signed release of the app to receive updates.".to_owned()),
        ),
        Capability::TemporarilyUnavailable => {
            ("Updates are unavailable right now.".to_owned(), None)
        }
        _ => ("Updates are turned off.".to_owned(), None),
    }
}

fn notes(notes: Option<&ReleaseNotes>) -> Option<String> {
    match notes? {
        ReleaseNotes::Inline { content, .. } => Some(content.clone()),
        ReleaseNotes::Link(url) => Some(format!("Release notes: {url}")),
        _ => None,
    }
}

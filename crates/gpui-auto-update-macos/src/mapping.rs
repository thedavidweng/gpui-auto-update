//! Pure translations from Sparkle's vocabulary to the core's.

use gpui_auto_update_core::{
    AvailableUpdate, Channel, CheckOutcome, ErrorKind, ReleaseNotes, ReleaseNotesFormat,
    UpdateError, UpdateEvent, UpdateState,
};

use crate::event::{SparkleError, SparkleEvent, SparkleUpdate, UpdateStage, UserChoice};

/// The core's description of an update Sparkle found.
pub(crate) fn available_update(update: &SparkleUpdate) -> AvailableUpdate {
    let mut available = AvailableUpdate::new(update.version.clone())
        .with_critical(update.critical)
        .with_major(update.major);
    if let Some(channel) = update.channel.as_deref().filter(|c| !c.is_empty()) {
        available = available.with_channel(Channel::new(channel));
    }
    if let Some(notes) = release_notes(update) {
        available = available.with_release_notes(notes);
    }
    if let Some(published) = &update.published {
        available = available.with_published(published.clone());
    }
    available
}

fn release_notes(update: &SparkleUpdate) -> Option<ReleaseNotes> {
    if let Some(content) = update
        .release_notes
        .as_deref()
        .filter(|n| !n.trim().is_empty())
    {
        // Sparkle treats an unspecified `sparkle:format` as HTML.
        let format = match update.release_notes_format.as_deref() {
            Some("plain-text") => ReleaseNotesFormat::PlainText,
            Some("markdown") => ReleaseNotesFormat::Markdown,
            _ => ReleaseNotesFormat::Html,
        };
        return Some(ReleaseNotes::Inline {
            content: content.to_owned(),
            format,
        });
    }
    update
        .release_notes_url
        .as_ref()
        .or(update.full_release_notes_url.as_ref())
        .map(|url| ReleaseNotes::Link(url.clone()))
}

/// The structured error for a Sparkle `NSError`.
///
/// The user-facing message is the kind's generic one: Sparkle's localized
/// descriptions can contain URLs or paths, so they go to the diagnostic.
pub(crate) fn update_error(error: &SparkleError) -> UpdateError {
    let mut diagnostic = format!(
        "Sparkle error {} code {}: {}",
        error.domain, error.code, error.message
    );
    if let Some(reason) = &error.failure_reason {
        diagnostic.push_str(" (");
        diagnostic.push_str(reason);
        diagnostic.push(')');
    }
    UpdateError::new(error_kind(error)).with_diagnostic(diagnostic)
}

/// Classifies Sparkle's `SUError` codes (SUErrors.h).
fn error_kind(error: &SparkleError) -> ErrorKind {
    if error.domain != SparkleError::SPARKLE_DOMAIN {
        return if error.domain == "NSURLErrorDomain" {
            ErrorKind::FeedRetrieval
        } else {
            ErrorKind::Internal
        };
    }
    match error.code {
        // Configuration: missing or insufficient keys, insecure or invalid
        // feed URL, invalid updater, host bundle identifier or version.
        1..=7 => ErrorKind::Configuration,
        1000 => ErrorKind::FeedParsing,
        // Appcast retrieval, resuming an appcast, release notes.
        1002 | 1004 | 1006 | 1007 => ErrorKind::FeedRetrieval,
        // Running from a disk image or translocated.
        1003 | 1005 => ErrorKind::UnsupportedInstallation,
        2000 => ErrorKind::Staging,
        2001 => ErrorKind::Download,
        3000 => ErrorKind::ArchiveValidation,
        3001 | 3002 => ErrorKind::Signature,
        4004 => ErrorKind::Relaunch,
        4000..=4099 => ErrorKind::Replacement,
        _ => ErrorKind::Internal,
    }
}

/// How a pending check ends, if `event` ends it.
pub(crate) fn check_resolution(event: &SparkleEvent) -> Option<Result<CheckOutcome, UpdateError>> {
    match event {
        SparkleEvent::UpdateFound(update) => {
            Some(Ok(CheckOutcome::UpdateAvailable(available_update(update))))
        }
        SparkleEvent::NoUpdateFound(reason) => {
            tracing::debug!(?reason, "Sparkle found no applicable update");
            Some(Ok(CheckOutcome::UpToDate))
        }
        SparkleEvent::Aborted(error) | SparkleEvent::CycleFinished { error: Some(error) } => {
            Some(if error.is_no_update() {
                Ok(CheckOutcome::UpToDate)
            } else {
                Err(update_error(error))
            })
        }
        SparkleEvent::CycleFinished { error: None } => {
            Some(Err(UpdateError::new(ErrorKind::TemporarilyUnavailable)
                .with_diagnostic(
                    "the Sparkle update cycle finished without a check result",
                )))
        }
        _ => None,
    }
}

/// Where Sparkle's update session has moved the update, as far as the
/// core's state model can express it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Downloading,
    Verifying,
    Staged,
    Installing,
    WaitingForQuit,
    Relaunching,
    Failed(UpdateError),
    Dismissed,
}

/// The phase `event` moves the session to, if it is a lifecycle step.
pub(crate) fn lifecycle_phase(event: &SparkleEvent) -> Option<Phase> {
    Some(match event {
        SparkleEvent::WillDownload { .. } => Phase::Downloading,
        SparkleEvent::Downloaded { .. } | SparkleEvent::WillExtract { .. } => Phase::Verifying,
        SparkleEvent::Extracted { .. } => Phase::Staged,
        SparkleEvent::WillInstall { .. } => Phase::Installing,
        SparkleEvent::WillInstallOnQuit { .. } => Phase::WaitingForQuit,
        SparkleEvent::WillRelaunch => Phase::Relaunching,
        SparkleEvent::DownloadFailed { error, .. } => Phase::Failed(update_error(error)),
        SparkleEvent::Aborted(error) if !error.is_no_update() => Phase::Failed(update_error(error)),
        SparkleEvent::DownloadCanceled => Phase::Failed(
            UpdateError::new(ErrorKind::Download)
                .with_message("The update download was canceled.")
                .with_diagnostic("the user canceled the download in Sparkle's window"),
        ),
        SparkleEvent::UserChoice { choice, stage, .. } => match (choice, stage) {
            (UserChoice::Install, _) => return None,
            // Sparkle keeps installing a dismissed update once the app quits.
            (UserChoice::Dismiss, UpdateStage::Installing) => Phase::WaitingForQuit,
            _ => Phase::Dismissed,
        },
        _ => return None,
    })
}

/// The events that move `current` to `phase`, filling in the steps Sparkle
/// does not report separately (a silently downloaded update goes straight
/// to "install on quit"). Never moves the state backwards.
pub(crate) fn catch_up(current: &UpdateState, phase: &Phase) -> Vec<UpdateEvent> {
    match phase {
        Phase::Failed(error) => {
            return if rank_of_state(current).is_some_and(|rank| rank > 0) {
                vec![UpdateEvent::Failed(error.clone())]
            } else {
                Vec::new()
            };
        }
        Phase::Dismissed => {
            return match current {
                UpdateState::Available(_) => vec![UpdateEvent::Dismissed],
                _ => Vec::new(),
            };
        }
        _ => {}
    }
    let (Some(from), Some(to)) = (rank_of_state(current), rank_of_phase(phase)) else {
        return Vec::new();
    };
    ((from + 1)..=to)
        .filter_map(|rank| match rank {
            1 => Some(UpdateEvent::DownloadStarted { total: None }),
            2 => Some(UpdateEvent::VerificationStarted),
            3 => Some(UpdateEvent::Staged),
            4 => Some(UpdateEvent::InstallStarted),
            // Relaunching goes straight from installing.
            5 if to == 5 => Some(UpdateEvent::WaitingForQuit),
            6 => Some(UpdateEvent::Relaunching),
            _ => None,
        })
        .collect()
}

fn rank_of_state(state: &UpdateState) -> Option<u8> {
    Some(match state {
        UpdateState::Available(_) => 0,
        UpdateState::Downloading { .. } => 1,
        UpdateState::Verifying(_) => 2,
        UpdateState::Staged(_) => 3,
        UpdateState::Installing(_) => 4,
        UpdateState::WaitingForQuit(_) => 5,
        UpdateState::Relaunching(_) => 6,
        _ => return None,
    })
}

fn rank_of_phase(phase: &Phase) -> Option<u8> {
    Some(match phase {
        Phase::Downloading => 1,
        Phase::Verifying => 2,
        Phase::Staged => 3,
        Phase::Installing => 4,
        Phase::WaitingForQuit => 5,
        Phase::Relaunching => 6,
        Phase::Failed(_) | Phase::Dismissed => return None,
    })
}

//! Behavioral tests for the public update state model and its transitions.

use gpui_auto_update_core::{
    AvailableUpdate, Capability, CheckKind, CheckOutcome, CheckRequest, CheckSource,
    DownloadProgress, ErrorKind, UpdateCoordinator, UpdateError, UpdateEvent, UpdateState,
};

struct Finds(AvailableUpdate);

impl CheckSource for Finds {
    fn check(&self, _request: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        Ok(CheckOutcome::UpdateAvailable(self.0.clone()))
    }
}

fn update() -> AvailableUpdate {
    AvailableUpdate::new("1.4.0").with_major(true)
}

fn with_available_update() -> UpdateCoordinator {
    let coordinator = UpdateCoordinator::new(Finds(update()), Capability::SelfManaged);
    coordinator.check(CheckKind::Background).unwrap();
    coordinator
}

#[test]
fn an_update_goes_from_available_to_completed() {
    let coordinator = with_available_update();
    let progress = DownloadProgress {
        downloaded: 512,
        total: Some(1024),
    };

    let steps = [
        (
            UpdateEvent::DownloadStarted { total: Some(1024) },
            UpdateState::Downloading {
                update: update(),
                progress: DownloadProgress {
                    downloaded: 0,
                    total: Some(1024),
                },
            },
        ),
        (
            UpdateEvent::DownloadProgressed(progress),
            UpdateState::Downloading {
                update: update(),
                progress,
            },
        ),
        (
            UpdateEvent::VerificationStarted,
            UpdateState::Verifying(update()),
        ),
        (UpdateEvent::Staged, UpdateState::Staged(update())),
        (
            UpdateEvent::InstallStarted,
            UpdateState::Installing(update()),
        ),
        (
            UpdateEvent::WaitingForQuit,
            UpdateState::WaitingForQuit(update()),
        ),
        (UpdateEvent::Relaunching, UpdateState::Relaunching(update())),
        (UpdateEvent::Completed, UpdateState::Completed(update())),
        (UpdateEvent::Dismissed, UpdateState::Idle),
    ];
    for (event, expected) in steps {
        coordinator.apply(event).unwrap();
        assert_eq!(coordinator.state(), expected);
    }
}

#[test]
fn a_failed_relaunch_is_reported_as_rolled_back() {
    let coordinator = with_available_update();
    for event in [
        UpdateEvent::DownloadStarted { total: None },
        UpdateEvent::Staged,
        UpdateEvent::InstallStarted,
        UpdateEvent::Relaunching,
    ] {
        coordinator.apply(event).unwrap();
    }
    let error = UpdateError::new(ErrorKind::HealthConfirmation);

    coordinator
        .apply(UpdateEvent::RolledBack(error.clone()))
        .unwrap();

    assert_eq!(
        coordinator.state(),
        UpdateState::RolledBack {
            update: update(),
            error
        }
    );
    assert_eq!(
        coordinator.check(CheckKind::Manual),
        Ok(CheckOutcome::UpdateAvailable(update()))
    );
}

#[test]
fn a_failed_download_becomes_failed_and_can_be_retried_by_checking() {
    let coordinator = with_available_update();
    coordinator
        .apply(UpdateEvent::DownloadStarted { total: Some(10) })
        .unwrap();
    let error = UpdateError::new(ErrorKind::LengthMismatch);

    coordinator
        .apply(UpdateEvent::Failed(error.clone()))
        .unwrap();

    assert_eq!(coordinator.state(), UpdateState::Failed(error));
    coordinator.check(CheckKind::Manual).unwrap();
    assert_eq!(coordinator.state(), UpdateState::Available(update()));
}

#[test]
fn overlapping_operations_are_rejected_without_changing_state() {
    let coordinator = with_available_update();
    coordinator
        .apply(UpdateEvent::DownloadStarted { total: None })
        .unwrap();
    let downloading = coordinator.state();

    let error = coordinator
        .apply(UpdateEvent::DownloadStarted { total: None })
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::OperationInProgress);
    let error = coordinator.apply(UpdateEvent::Dismissed).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::OperationInProgress);
    let error = coordinator.apply(UpdateEvent::InstallStarted).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::OperationInProgress);

    assert_eq!(coordinator.state(), downloading);
}

#[test]
fn steps_that_do_not_apply_yet_are_invalid() {
    let coordinator = UpdateCoordinator::new(Finds(update()), Capability::SelfManaged);

    for event in [
        UpdateEvent::DownloadStarted { total: None },
        UpdateEvent::InstallStarted,
        UpdateEvent::Completed,
        UpdateEvent::Failed(UpdateError::new(ErrorKind::Download)),
    ] {
        let error = coordinator.apply(event).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidState);
    }
    assert_eq!(coordinator.state(), UpdateState::Idle);
}

#[test]
fn a_disabled_installation_accepts_no_update_steps() {
    let capability = Capability::ExternallyManaged { manager: None };
    let coordinator = UpdateCoordinator::new(Finds(update()), capability.clone());

    let error = coordinator
        .apply(UpdateEvent::DownloadStarted { total: None })
        .unwrap_err();

    assert_eq!(error.kind(), ErrorKind::InvalidState);
    assert_eq!(coordinator.state(), UpdateState::Disabled { capability });
}

#[test]
fn busy_states_are_the_ones_that_exclude_other_operations() {
    let u = update();
    let busy = [
        UpdateState::Checking,
        UpdateState::Downloading {
            update: u.clone(),
            progress: DownloadProgress {
                downloaded: 0,
                total: None,
            },
        },
        UpdateState::Verifying(u.clone()),
        UpdateState::Installing(u.clone()),
        UpdateState::WaitingForQuit(u.clone()),
        UpdateState::Relaunching(u.clone()),
    ];
    let settled = [
        UpdateState::Disabled {
            capability: Capability::Unsupported,
        },
        UpdateState::Idle,
        UpdateState::UpToDate,
        UpdateState::Available(u.clone()),
        UpdateState::Staged(u.clone()),
        UpdateState::Completed(u.clone()),
        UpdateState::Failed(UpdateError::new(ErrorKind::Download)),
    ];
    assert!(busy.iter().all(UpdateState::is_busy));
    assert!(!settled.iter().any(UpdateState::is_busy));
}

#[test]
fn download_progress_supports_known_and_unknown_totals() {
    let known = DownloadProgress {
        downloaded: 25,
        total: Some(100),
    };
    let unknown = DownloadProgress {
        downloaded: 25,
        total: None,
    };

    assert_eq!(known.fraction(), Some(0.25));
    assert_eq!(unknown.fraction(), None);
}

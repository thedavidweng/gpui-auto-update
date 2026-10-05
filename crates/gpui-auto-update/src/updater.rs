//! The observable GPUI updater entity.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{App, AppContext as _, AsyncApp, Context, EventEmitter, Subscription, Task};
use gpui_auto_update_core::{
    AutomaticChecks, AvailableUpdate, Capability, Channel, CheckKind, CheckOutcome, ErrorKind,
    FilePreferenceStore, MemoryPreferenceStore, PreferenceOwner, PreferenceStore,
    UpdateCoordinator, UpdateError, UpdateEvent, UpdateState,
};

use crate::backend::{Handoff, ProgressSink, UpdateBackend};
use crate::config::{BuildProfile, UpdaterConfig};
use crate::paths::default_preferences_path;
use crate::preview::PreviewState;

/// The error a prepare-to-install hook fails with.
pub type PrepareError = Box<dyn std::error::Error + Send + Sync + 'static>;

type PrepareHook = Rc<dyn Fn(&mut App) -> Task<Result<(), PrepareError>>>;

/// Something the updater reports to subscribers, in addition to notifying
/// observers whenever anything it exposes changes.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpdaterEvent {
    /// Startup finished: the capability, automatic-update preference, and
    /// channel are known.
    Ready,
    /// [`Updater::state`] changed.
    StateChanged,
    /// A check finished or was rejected. Manual checks always produce this
    /// event, so the application can always give visible feedback.
    CheckFinished {
        /// Whether the user asked for the check.
        kind: CheckKind,
        /// The outcome, or why the check failed or could not run.
        result: Result<CheckOutcome, UpdateError>,
    },
    /// The automatic-update preference was saved with this value.
    AutomaticChecksChanged(bool),
    /// The release channel was changed to this value.
    ChannelChanged(Option<Channel>),
    /// An operation other than a check failed: staging, installing,
    /// relaunching, a prepare-to-install hook, or saving a setting. The
    /// error is also available from [`Updater::last_error`].
    Failed(UpdateError),
    /// The updater is about to end the application as `Handoff` describes.
    Handoff(Handoff),
}

/// Messages from background work, applied on the foreground in order.
enum Message {
    State(UpdateState),
    Ready {
        automatic: AutomaticChecks,
        automatic_checks: bool,
        channel: Option<Channel>,
    },
    CheckFinished {
        id: Option<u64>,
        kind: CheckKind,
        result: Result<CheckOutcome, UpdateError>,
    },
    PreferenceSaved {
        enabled: bool,
        result: Result<bool, UpdateError>,
    },
    ChannelSaved {
        channel: Option<Channel>,
        result: Result<(), UpdateError>,
    },
    OperationFinished(Result<Option<Handoff>, UpdateError>),
}

enum Step {
    Install(AvailableUpdate),
    Relaunch,
}

/// The application's updater, exposed as an observable GPUI entity.
///
/// Create it with [`crate::init`], which also registers the standard actions
/// and makes it reachable through [`Updater::global`]. All network and
/// filesystem work runs on GPUI's background executor; every resulting
/// change is applied on the foreground thread, after which the updater emits
/// [`UpdaterEvent`]s and notifies observers (`cx.observe`).
///
/// # Lifetime
///
/// The updater must live as long as the application. [`crate::init`] keeps
/// it alive in a GPUI global. If you create it yourself with
/// [`Updater::new`], keep the [`gpui::Entity`] for the application's
/// lifetime: dropping it cancels queued work, discards the results of
/// background work that is already running, and stops automatic checks. A
/// dropped updater never quits or restarts the application, so an update
/// that was being staged or installed is simply abandoned and is offered
/// again by a later check.
pub struct Updater {
    coordinator: UpdateCoordinator,
    backend: Arc<dyn UpdateBackend>,
    automatic: Option<AutomaticChecks>,
    automatic_checks: bool,
    channel: Option<Channel>,
    state: UpdateState,
    preview: Option<PreviewState>,
    ready: bool,
    pending_checks: Vec<CheckKind>,
    build_profile: BuildProfile,
    allow_debug_self_update: bool,
    hooks: Rc<RefCell<BTreeMap<u64, PrepareHook>>>,
    next_id: u64,
    last_error: Option<UpdateError>,
    checks: BTreeMap<u64, Task<()>>,
    operation: Option<Task<()>>,
    preference_task: Option<Task<()>>,
    channel_task: Option<Task<()>>,
    periodic: Option<Task<()>>,
    _setup: Task<()>,
    _messages: Task<()>,
    tx: mpsc::UnboundedSender<Message>,
    _state_subscription: gpui_auto_update_core::Subscription,
}

impl EventEmitter<UpdaterEvent> for Updater {}

impl Updater {
    /// Creates the updater. Prefer [`crate::init`], which also registers the
    /// standard actions; see the type documentation about the lifetime.
    ///
    /// Startup (asking the backend for the capability, loading preferences,
    /// and the launch check) runs in the background; [`UpdaterEvent::Ready`]
    /// is emitted when it is done. Until then the state is
    /// [`UpdateState::Disabled`] with [`Capability::TemporarilyUnavailable`]
    /// unless a capability was configured, and manual checks are queued.
    pub fn new(config: UpdaterConfig, cx: &mut Context<Self>) -> Self {
        let UpdaterConfig {
            app_id,
            source,
            backend,
            capability,
            preferences,
            policy,
            clock,
            build_profile,
            allow_debug_self_update,
            preview,
        } = config;

        let (tx, mut rx) = mpsc::unbounded();
        let initial_capability = capability
            .clone()
            .unwrap_or(Capability::TemporarilyUnavailable);
        let coordinator = UpdateCoordinator::new(source, initial_capability);
        let sender = tx.clone();
        let state_subscription = coordinator.subscribe(move |state| {
            let _ = sender.unbounded_send(Message::State(state.clone()));
        });

        let messages = cx.spawn(async move |this, cx: &mut AsyncApp| {
            while let Some(message) = rx.next().await {
                if this
                    .update(cx, |this, cx| this.handle(message, cx))
                    .is_err()
                {
                    break;
                }
            }
        });

        let automatic_checks = policy.automatic_checks_by_default();
        let setup = {
            let coordinator = coordinator.clone();
            let backend = backend.clone();
            let tx = tx.clone();
            cx.background_spawn(async move {
                let capability = capability.unwrap_or_else(|| backend.capability());
                if let Err(error) = coordinator.set_capability(capability) {
                    tracing::warn!(kind_of_error = ?error.kind(), "could not apply the update capability: {error}");
                }
                let store = preferences.unwrap_or_else(|| default_store(&app_id));
                let automatic = AutomaticChecks::with_clock(coordinator, store, policy, clock);
                let _ = tx.unbounded_send(Message::Ready {
                    automatic_checks: automatic.automatic_checks_enabled(),
                    channel: backend.channel(),
                    automatic: automatic.clone(),
                });
                if let Some(result) = automatic.run_launch_check() {
                    let _ = tx.unbounded_send(Message::CheckFinished {
                        id: None,
                        kind: CheckKind::Background,
                        result,
                    });
                }
            })
        };

        Self {
            state: coordinator.state(),
            coordinator,
            backend,
            automatic: None,
            automatic_checks,
            channel: None,
            preview,
            ready: false,
            pending_checks: Vec::new(),
            build_profile,
            allow_debug_self_update,
            hooks: Rc::default(),
            next_id: 0,
            last_error: None,
            checks: BTreeMap::new(),
            operation: None,
            preference_task: None,
            channel_task: None,
            periodic: None,
            _setup: setup,
            _messages: messages,
            tx,
            _state_subscription: state_subscription,
        }
    }

    /// The updater registered by [`crate::init`], if any.
    pub fn global(cx: &App) -> Option<gpui::Entity<Self>> {
        crate::global(cx)
    }

    /// What the updater is doing, or the preview state while previewing.
    pub fn state(&self) -> UpdateState {
        match self.preview {
            Some(preview) => preview.state(),
            None => self.state.clone(),
        }
    }

    /// Whether this installation may update itself.
    pub fn capability(&self) -> Capability {
        match self.preview {
            Some(PreviewState::ExternallyManaged) => PreviewState::externally_managed(),
            Some(_) => Capability::SelfManaged,
            None => self.coordinator.capability(),
        }
    }

    /// Whether this installation supports self-update. Always `false` while
    /// previewing, because a preview never installs anything.
    pub fn is_self_update_supported(&self) -> bool {
        self.preview.is_none() && self.coordinator.capability().can_self_update()
    }

    /// Whether this build may install updates: `false` for debug builds
    /// unless [`UpdaterConfig::allow_debug_self_update`] was set, and while
    /// previewing.
    pub fn installs_allowed(&self) -> bool {
        self.preview.is_none() && self.install_guard().is_none()
    }

    /// Whether startup has finished; see [`Updater::new`].
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// Whether a check, stage, install, or relaunch is running.
    pub fn is_busy(&self) -> bool {
        self.operation.is_some() || self.state().is_busy()
    }

    /// The most recent failure of a manual check or other operation.
    pub fn last_error(&self) -> Option<&UpdateError> {
        self.last_error.as_ref()
    }

    /// Whether background checks run.
    pub fn automatic_checks_enabled(&self) -> bool {
        self.automatic_checks
    }

    /// Who stores the automatic-update preference, once startup finished.
    pub fn preference_owner(&self) -> Option<PreferenceOwner> {
        self.automatic
            .as_ref()
            .map(AutomaticChecks::preference_owner)
    }

    /// The release channel, once startup finished; `None` is the default
    /// channel.
    pub fn channel(&self) -> Option<&Channel> {
        self.channel.as_ref()
    }

    /// The preview being shown, if any.
    pub fn preview(&self) -> Option<PreviewState> {
        self.preview
    }

    /// Whether a preview is being shown instead of the real state.
    pub fn is_preview(&self) -> bool {
        self.preview.is_some()
    }

    /// The framework-independent coordinator, for advanced integrations
    /// such as backends that report progress outside [`UpdateBackend`]
    /// calls. Its methods may block; call them off the foreground thread.
    pub fn coordinator(&self) -> &UpdateCoordinator {
        &self.coordinator
    }

    /// Starts a manual check, or attaches to the check that is running.
    ///
    /// The outcome is always reported with [`UpdaterEvent::CheckFinished`]
    /// and, for a successful or failed check, also in [`Self::state`].
    pub fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        if let Some(preview) = self.preview {
            let result = preview_check(preview);
            if let Err(error) = &result {
                self.last_error = Some(error.clone());
            }
            cx.emit(UpdaterEvent::CheckFinished {
                kind: CheckKind::Manual,
                result,
            });
            cx.notify();
            return;
        }
        self.start_check(CheckKind::Manual, cx);
    }

    /// Requests installation of the current update.
    ///
    /// When an update is available it is downloaded, verified, and staged;
    /// the state then becomes [`UpdateState::Staged`] and installation waits
    /// for [`Self::restart_to_update`] (or another `request_install`). When
    /// an update is already staged this is the same as
    /// [`Self::restart_to_update`].
    ///
    /// Rejected immediately while previewing, in a debug build that does not
    /// allow self-update, while another operation runs
    /// ([`ErrorKind::OperationInProgress`]), and when no update is available
    /// ([`ErrorKind::InvalidState`]).
    pub fn request_install(&mut self, cx: &mut Context<Self>) -> Result<(), UpdateError> {
        self.ensure_can_operate()?;
        match self.coordinator.state() {
            UpdateState::Available(update) => self.stage(update, cx),
            UpdateState::Staged(_) => self.restart_to_update(cx),
            state => Err(rejection(&state)),
        }
    }

    /// Installs the staged update and ends the application as the backend
    /// requires, or relaunches after an install that waits for the
    /// application to quit.
    ///
    /// The prepare-to-install hooks run first, on the foreground, one after
    /// another; if one fails the update stays staged and
    /// [`UpdaterEvent::Failed`] reports a [`ErrorKind::QuitCoordination`]
    /// error. The backend then decides how the application ends: GPUI's
    /// restart ([`Handoff::Restart`]), a plain quit while a helper or
    /// installer relaunches ([`Handoff::Quit`]), or nothing because the
    /// backend relaunches itself ([`Handoff::BackendOwned`]).
    pub fn restart_to_update(&mut self, cx: &mut Context<Self>) -> Result<(), UpdateError> {
        self.ensure_can_operate()?;
        let step = match self.coordinator.state() {
            UpdateState::Staged(update) => Step::Install(update),
            UpdateState::WaitingForQuit(_) | UpdateState::Completed(_) => Step::Relaunch,
            state => return Err(rejection(&state)),
        };
        let hooks: Vec<PrepareHook> = self.hooks.borrow().values().cloned().collect();
        let coordinator = self.coordinator.clone();
        let backend = self.backend.clone();
        let tx = self.tx.clone();
        let executor = cx.background_executor().clone();
        self.operation = Some(cx.spawn(async move |_, cx: &mut AsyncApp| {
            if let Err(error) = run_prepare_hooks(hooks, cx).await {
                let _ = tx.unbounded_send(Message::OperationFinished(Err(error)));
                return;
            }
            executor
                .spawn(async move {
                    let result = match step {
                        Step::Install(update) => install(&coordinator, &*backend, &update),
                        Step::Relaunch => relaunch(&coordinator, &*backend),
                    };
                    let _ = tx.unbounded_send(Message::OperationFinished(result));
                })
                .await;
        }));
        cx.notify();
        Ok(())
    }

    /// Dismisses the current outcome (up to date, available, failed, rolled
    /// back, or completed) and returns to idle.
    pub fn dismiss(&mut self, cx: &mut Context<Self>) -> Result<(), UpdateError> {
        if self.preview.is_some() {
            return Err(PreviewState::rejection());
        }
        self.coordinator.apply(UpdateEvent::Dismissed)?;
        cx.notify();
        Ok(())
    }

    /// Enables or disables background checks and saves the choice in the
    /// background.
    ///
    /// [`UpdaterEvent::AutomaticChecksChanged`] reports success, after which
    /// [`Self::automatic_checks_enabled`] has the new value; a failure is
    /// reported with [`UpdaterEvent::Failed`]. Enabling may start a
    /// background check right away, as the policy allows.
    pub fn set_automatic_checks(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), UpdateError> {
        if self.preview.is_some() {
            return Err(PreviewState::rejection());
        }
        let automatic = self.automatic.clone().ok_or_else(not_ready)?;
        let tx = self.tx.clone();
        self.preference_task = Some(cx.background_spawn(async move {
            let result = automatic.set_automatic_checks(enabled);
            let _ = tx.unbounded_send(Message::PreferenceSaved { enabled, result });
        }));
        Ok(())
    }

    /// Changes the release channel through the backend, in the background.
    ///
    /// [`UpdaterEvent::ChannelChanged`] reports success; a failure, such as
    /// a backend without channels, is reported with
    /// [`UpdaterEvent::Failed`].
    pub fn set_channel(
        &mut self,
        channel: Option<Channel>,
        cx: &mut Context<Self>,
    ) -> Result<(), UpdateError> {
        if self.preview.is_some() {
            return Err(PreviewState::rejection());
        }
        let backend = self.backend.clone();
        let tx = self.tx.clone();
        self.channel_task = Some(cx.background_spawn(async move {
            let result = backend.set_channel(channel.clone());
            let _ = tx.unbounded_send(Message::ChannelSaved { channel, result });
        }));
        Ok(())
    }

    /// Registers a hook that saves application state before the updater
    /// installs an update or relaunches the application.
    ///
    /// Hooks run on the foreground in registration order and may do
    /// asynchronous work through the returned task; the updater waits for
    /// each one, with no timeout, before handing off. A hook that fails
    /// cancels the install. Hooks complement, and do not replace, the
    /// application's normal quit handling: GPUI gives `on_app_quit`
    /// handlers only a short time, so slow saves belong here.
    ///
    /// The hook stays registered until the returned [`Subscription`] is
    /// dropped.
    pub fn on_prepare_to_install(
        &mut self,
        hook: impl Fn(&mut App) -> Task<Result<(), PrepareError>> + 'static,
    ) -> Subscription {
        let id = self.next_id();
        self.hooks.borrow_mut().insert(id, Rc::new(hook));
        let hooks = Rc::downgrade(&self.hooks);
        Subscription::new(move || {
            if let Some(hooks) = hooks.upgrade() {
                hooks.borrow_mut().remove(&id);
            }
        })
    }

    /// Shows `preview` instead of the real state. While previewing nothing
    /// is checked, downloaded, or installed; see [`PreviewState`].
    pub fn enter_preview(&mut self, preview: PreviewState, cx: &mut Context<Self>) {
        if self.preview != Some(preview) {
            self.preview = Some(preview);
            cx.emit(UpdaterEvent::StateChanged);
            cx.notify();
        }
    }

    /// Stops previewing and shows the real state again.
    pub fn exit_preview(&mut self, cx: &mut Context<Self>) {
        if self.preview.take().is_some() {
            cx.emit(UpdaterEvent::StateChanged);
            cx.notify();
        }
    }

    pub(crate) fn report(&mut self, error: UpdateError, cx: &mut Context<Self>) {
        tracing::warn!(
            kind_of_error = ?error.kind(),
            diagnostic = error.diagnostic(),
            "update operation failed: {error}"
        );
        self.last_error = Some(error.clone());
        cx.emit(UpdaterEvent::Failed(error));
        cx.notify();
    }

    fn handle(&mut self, message: Message, cx: &mut Context<Self>) {
        match message {
            Message::State(state) => {
                if self.state != state {
                    self.state = state;
                    if self.preview.is_none() {
                        cx.emit(UpdaterEvent::StateChanged);
                        cx.notify();
                    }
                }
            }
            Message::Ready {
                automatic,
                automatic_checks,
                channel,
            } => {
                self.automatic = Some(automatic);
                self.automatic_checks = automatic_checks;
                self.channel = channel;
                self.ready = true;
                cx.emit(UpdaterEvent::Ready);
                cx.notify();
                self.schedule_periodic_checks(cx);
                for kind in std::mem::take(&mut self.pending_checks) {
                    self.start_check(kind, cx);
                }
            }
            Message::CheckFinished { id, kind, result } => {
                if let Some(id) = id {
                    self.checks.remove(&id);
                }
                if kind == CheckKind::Manual
                    && let Err(error) = &result
                {
                    self.last_error = Some(error.clone());
                }
                cx.emit(UpdaterEvent::CheckFinished { kind, result });
                cx.notify();
            }
            Message::PreferenceSaved { enabled, result } => {
                self.preference_task = None;
                match result {
                    Ok(check_now) => {
                        self.automatic_checks = enabled;
                        cx.emit(UpdaterEvent::AutomaticChecksChanged(enabled));
                        cx.notify();
                        if check_now {
                            self.start_check(CheckKind::Background, cx);
                        }
                        self.schedule_periodic_checks(cx);
                    }
                    Err(error) => self.report(error, cx),
                }
            }
            Message::ChannelSaved { channel, result } => {
                self.channel_task = None;
                match result {
                    Ok(()) => {
                        self.channel = channel.clone();
                        cx.emit(UpdaterEvent::ChannelChanged(channel));
                        cx.notify();
                    }
                    Err(error) => self.report(error, cx),
                }
            }
            Message::OperationFinished(result) => {
                self.operation = None;
                match result {
                    Ok(Some(handoff)) => self.hand_off(handoff, cx),
                    Ok(None) => {}
                    Err(error) => self.report(error, cx),
                }
                cx.notify();
            }
        }
    }

    fn start_check(&mut self, kind: CheckKind, cx: &mut Context<Self>) {
        if !self.ready {
            if !self.pending_checks.contains(&kind) {
                self.pending_checks.push(kind);
            }
            return;
        }
        let id = self.next_id();
        let coordinator = self.coordinator.clone();
        let tx = self.tx.clone();
        let task = cx.background_spawn(async move {
            let result = coordinator.check(kind);
            let _ = tx.unbounded_send(Message::CheckFinished {
                id: Some(id),
                kind,
                result,
            });
        });
        self.checks.insert(id, task);
    }

    fn stage(
        &mut self,
        update: AvailableUpdate,
        cx: &mut Context<Self>,
    ) -> Result<(), UpdateError> {
        self.coordinator
            .apply(UpdateEvent::DownloadStarted { total: None })?;
        let coordinator = self.coordinator.clone();
        let backend = self.backend.clone();
        let tx = self.tx.clone();
        self.operation = Some(cx.background_spawn(async move {
            let sink = ProgressSink::new(coordinator.clone());
            let result = match guarded(|| backend.stage(&update, &sink)) {
                Ok(()) => coordinator.apply(UpdateEvent::Staged).map(|()| None),
                Err(error) => {
                    let _ = coordinator.apply(UpdateEvent::Failed(error.clone()));
                    Err(error)
                }
            };
            let _ = tx.unbounded_send(Message::OperationFinished(result));
        }));
        cx.notify();
        Ok(())
    }

    fn hand_off(&mut self, handoff: Handoff, cx: &mut Context<Self>) {
        tracing::info!(?handoff, "handing off to finish the update");
        cx.emit(UpdaterEvent::Handoff(handoff.clone()));
        match handoff {
            Handoff::Restart { restart_path } => {
                if let Some(path) = restart_path {
                    cx.set_restart_path(path);
                }
                cx.restart();
            }
            Handoff::Quit => cx.quit(),
            Handoff::BackendOwned => {}
        }
    }

    fn schedule_periodic_checks(&mut self, cx: &mut Context<Self>) {
        let Some(automatic) = self.automatic.clone() else {
            return;
        };
        if automatic.policy().periodic_interval().is_none() {
            return;
        }
        // Retrying no sooner than the minimum interval keeps a
        // capability-blocked updater from spinning.
        let retry = automatic
            .policy()
            .minimum_interval()
            .max(Duration::from_secs(60));
        let executor = cx.background_executor().clone();
        let tx = self.tx.clone();
        self.periodic = Some(cx.background_spawn(async move {
            while let Some(delay) = automatic.periodic_check_delay() {
                if !delay.is_zero() {
                    executor.timer(delay).await;
                }
                match automatic.run_periodic_check() {
                    Some(result) => {
                        let _ = tx.unbounded_send(Message::CheckFinished {
                            id: None,
                            kind: CheckKind::Background,
                            result,
                        });
                    }
                    None => executor.timer(retry).await,
                }
            }
        }));
    }

    fn ensure_can_operate(&self) -> Result<(), UpdateError> {
        if self.preview.is_some() {
            return Err(PreviewState::rejection());
        }
        if let Some(error) = self.install_guard() {
            return Err(error);
        }
        if self.operation.is_some() {
            return Err(UpdateError::new(ErrorKind::OperationInProgress));
        }
        if let Some(error) = self.coordinator.capability().denial() {
            return Err(error);
        }
        Ok(())
    }

    fn install_guard(&self) -> Option<UpdateError> {
        (self.build_profile == BuildProfile::Debug && !self.allow_debug_self_update).then(|| {
            UpdateError::new(ErrorKind::Configuration).with_message(
                "Debug builds do not install updates unless self-update is explicitly allowed.",
            )
        })
    }

    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
}

impl std::fmt::Debug for Updater {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updater")
            .field("state", &self.state())
            .field("preview", &self.preview)
            .field("ready", &self.ready)
            .finish_non_exhaustive()
    }
}

async fn run_prepare_hooks(hooks: Vec<PrepareHook>, cx: &mut AsyncApp) -> Result<(), UpdateError> {
    for hook in hooks {
        let task = cx.update(|cx| hook(cx)).map_err(|_| {
            UpdateError::new(ErrorKind::QuitCoordination)
                .with_diagnostic("the application is shutting down")
        })?;
        task.await.map_err(|error| {
            UpdateError::new(ErrorKind::QuitCoordination)
                .with_message("The application could not save its state before updating.")
                .with_diagnostic(format!("prepare-to-install hook failed: {error}"))
        })?;
    }
    Ok(())
}

fn install(
    coordinator: &UpdateCoordinator,
    backend: &dyn UpdateBackend,
    update: &AvailableUpdate,
) -> Result<Option<Handoff>, UpdateError> {
    coordinator.apply(UpdateEvent::InstallStarted)?;
    let sink = ProgressSink::new(coordinator.clone());
    match guarded(|| backend.install(update, &sink)) {
        Ok(handoff) => {
            let event = match handoff {
                Handoff::Restart { .. } => Some(UpdateEvent::Relaunching),
                Handoff::Quit => Some(UpdateEvent::WaitingForQuit),
                _ => None,
            };
            if let Some(event) = event {
                // The backend may already have reported this step.
                let _ = coordinator.apply(event);
            }
            Ok(Some(handoff))
        }
        Err(error) => {
            let _ = coordinator.apply(UpdateEvent::Failed(error.clone()));
            Err(error)
        }
    }
}

fn relaunch(
    coordinator: &UpdateCoordinator,
    backend: &dyn UpdateBackend,
) -> Result<Option<Handoff>, UpdateError> {
    let handoff = guarded(|| backend.relaunch())?;
    if matches!(handoff, Handoff::Restart { .. }) {
        let _ = coordinator.apply(UpdateEvent::Relaunching);
    }
    Ok(Some(handoff))
}

/// Turns a panicking backend call into an internal error instead of
/// silently losing the operation.
fn guarded<T>(call: impl FnOnce() -> Result<T, UpdateError>) -> Result<T, UpdateError> {
    panic::catch_unwind(AssertUnwindSafe(call)).unwrap_or_else(|_| {
        tracing::error!("update backend panicked");
        Err(UpdateError::new(ErrorKind::Internal).with_diagnostic("update backend panicked"))
    })
}

fn rejection(state: &UpdateState) -> UpdateError {
    match state {
        UpdateState::Disabled { capability } => capability
            .denial()
            .unwrap_or_else(|| UpdateError::new(ErrorKind::InvalidState)),
        state if state.is_busy() => UpdateError::new(ErrorKind::OperationInProgress),
        _ => UpdateError::new(ErrorKind::InvalidState)
            .with_message("There is no update to install right now."),
    }
}

fn not_ready() -> UpdateError {
    UpdateError::new(ErrorKind::TemporarilyUnavailable)
        .with_message("The updater is still starting. Try again in a moment.")
}

fn preview_check(preview: PreviewState) -> Result<CheckOutcome, UpdateError> {
    match preview {
        PreviewState::Error => Err(PreviewState::error()),
        PreviewState::ExternallyManaged => Err(PreviewState::externally_managed()
            .denial()
            .unwrap_or_else(|| UpdateError::new(ErrorKind::ExternallyManaged))),
        _ => Ok(CheckOutcome::UpdateAvailable(PreviewState::update())),
    }
}

fn default_store(app_id: &str) -> Box<dyn PreferenceStore> {
    match default_preferences_path(app_id) {
        Ok(path) => Box::new(FilePreferenceStore::new(path)),
        Err(error) => {
            tracing::warn!(
                diagnostic = error.diagnostic(),
                "update preferences will not persist: {error}"
            );
            Box::new(MemoryPreferenceStore::new())
        }
    }
}

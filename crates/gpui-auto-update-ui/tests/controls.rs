//! Behavior of the neutral controls: keyboard navigation, visible feedback
//! for manual checks, and the unobtrusive update affordance.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use gpui::{Entity, TestAppContext};
use gpui_auto_update::core::{
    Capability, CheckOutcome, CheckPolicy, CheckRequest, CheckSource, MemoryPreferenceStore,
    UpdateError, UpdateState,
};
use gpui_auto_update::{PreviewState, Updater, UpdaterConfig};
use gpui_auto_update_ui::{UpdateAction, UpdateControls, UpdateIndicator};

/// A check source with scripted outcomes; checks run on the foreground in
/// tests, so this stays deterministic.
#[derive(Clone)]
struct ScriptedSource {
    outcomes: Arc<Mutex<VecDeque<Result<CheckOutcome, UpdateError>>>>,
}

impl ScriptedSource {
    fn push(&self, outcome: Result<CheckOutcome, UpdateError>) {
        self.outcomes.lock().unwrap().push_back(outcome);
    }
}

impl CheckSource for ScriptedSource {
    fn check(&self, _: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(CheckOutcome::UpToDate))
    }
}

fn live_updater(cx: &mut TestAppContext) -> (Entity<Updater>, ScriptedSource) {
    let source = ScriptedSource {
        outcomes: Arc::new(Mutex::new(VecDeque::new())),
    };
    let updater = cx.update(|cx| {
        gpui_auto_update::init(
            UpdaterConfig::new("dev.example.ui-controls", source.clone())
                .with_capability(Capability::SelfManaged)
                .with_preferences(MemoryPreferenceStore::new())
                .with_policy(CheckPolicy::recommended().with_check_on_launch(false)),
            cx,
        )
    });
    cx.run_until_parked();
    (updater, source)
}

fn preview_updater(cx: &mut TestAppContext, preview: PreviewState) -> Entity<Updater> {
    let updater = cx.update(|cx| {
        gpui_auto_update::init(
            UpdaterConfig::preview("dev.example.ui-controls", preview),
            cx,
        )
    });
    cx.run_until_parked();
    updater
}

#[gpui::test]
async fn keyboard_navigation_runs_a_manual_check(cx: &mut TestAppContext) {
    let (updater, source) = live_updater(cx);
    source.push(Ok(CheckOutcome::UpToDate));
    let (_, cx) = cx.add_window_view(|window, cx| {
        UpdateControls::new(updater.clone(), window, cx).with_current_version("1.0.0")
    });

    // The first control in an idle panel is "Check for Updates".
    cx.simulate_keystrokes("tab enter");
    assert_eq!(
        updater.read_with(cx, |updater, _| updater.state()),
        UpdateState::UpToDate,
        "pressing enter on the focused control starts the check"
    );
}

#[gpui::test]
async fn keyboard_toggles_the_automatic_updates_setting(cx: &mut TestAppContext) {
    let (updater, _) = live_updater(cx);
    let (_, cx) = cx.add_window_view(|window, cx| UpdateControls::new(updater.clone(), window, cx));
    assert!(updater.read_with(cx, |updater, _| updater.automatic_checks_enabled()));

    // Tab order: "Check for Updates", then the automatic-updates setting.
    cx.simulate_keystrokes("tab tab space");
    assert!(
        !updater.read_with(cx, |updater, _| updater.automatic_checks_enabled()),
        "space on the focused setting toggles it"
    );
}

#[gpui::test]
async fn a_refused_manual_check_gives_visible_feedback(cx: &mut TestAppContext) {
    let updater = preview_updater(cx, PreviewState::ExternallyManaged);
    let (controls, cx) =
        cx.add_window_view(|window, cx| UpdateControls::new(updater.clone(), window, cx));

    cx.simulate_keystrokes("tab enter");
    let feedback = controls.read_with(cx, |controls, _| controls.feedback().map(str::to_owned));
    assert_eq!(
        feedback.as_deref(),
        Some("This installation is updated by Preview package manager."),
        "a manual check always produces visible feedback"
    );
}

#[gpui::test]
async fn the_indicator_offers_the_attention_action(cx: &mut TestAppContext) {
    let updater = preview_updater(cx, PreviewState::UpdateAvailable);
    let (indicator, cx) = cx.add_window_view(|_, cx| UpdateIndicator::new(updater.clone(), cx));
    assert_eq!(
        indicator.read_with(cx, |indicator, cx| indicator.attention(cx)),
        Some(UpdateAction::Download)
    );

    let updater = preview_updater(cx, PreviewState::Downloading);
    let (indicator, cx) = cx.add_window_view(|_, cx| UpdateIndicator::new(updater.clone(), cx));
    assert_eq!(
        indicator.read_with(cx, |indicator, cx| indicator.attention(cx)),
        None,
        "nothing to act on while the update downloads"
    );
}

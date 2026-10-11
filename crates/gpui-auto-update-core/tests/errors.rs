//! Behavioral tests for structured errors and background-failure logging.

use std::error::Error as _;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use gpui_auto_update_core::{
    Capability, CheckKind, CheckOutcome, CheckRequest, CheckSource, ErrorKind, UpdateCoordinator,
    UpdateError,
};

const SECRET_PATH: &str = "/Users/alice/Library/Application Support/App/token-abc123";

#[test]
fn display_shows_only_the_user_message() {
    let io = std::io::Error::other(format!("permission denied: {SECRET_PATH}"));
    let error = UpdateError::new(ErrorKind::Staging)
        .with_diagnostic(format!("rename {SECRET_PATH} failed"))
        .with_source(io);

    let shown = error.to_string();

    assert_eq!(shown, "The update could not be prepared for installation.");
    assert_eq!(error.message(), shown);
    assert!(!shown.contains("alice"));
    assert_eq!(error.kind(), ErrorKind::Staging);
    assert!(error.diagnostic().unwrap().contains(SECRET_PATH));
    assert!(error.source().unwrap().to_string().contains(SECRET_PATH));
}

#[test]
fn custom_user_messages_replace_the_default() {
    let error = UpdateError::new(ErrorKind::Configuration)
        .with_message("The update feed address is missing.");

    assert_eq!(error.to_string(), "The update feed address is missing.");
    assert_eq!(error.kind(), ErrorKind::Configuration);
}

#[test]
fn every_error_class_has_a_distinct_generic_message() {
    let kinds = [
        ErrorKind::Configuration,
        ErrorKind::UnsupportedInstallation,
        ErrorKind::ExternallyManaged,
        ErrorKind::TemporarilyUnavailable,
        ErrorKind::OperationInProgress,
        ErrorKind::InvalidState,
        ErrorKind::FeedRetrieval,
        ErrorKind::FeedParsing,
        ErrorKind::VersionResolution,
        ErrorKind::Signature,
        ErrorKind::Download,
        ErrorKind::LengthMismatch,
        ErrorKind::ArchiveValidation,
        ErrorKind::Staging,
        ErrorKind::HelperLaunch,
        ErrorKind::QuitCoordination,
        ErrorKind::Replacement,
        ErrorKind::Relaunch,
        ErrorKind::HealthConfirmation,
        ErrorKind::Rollback,
        ErrorKind::Preferences,
        ErrorKind::Internal,
    ];
    let mut messages: Vec<&str> = kinds.iter().map(|kind| kind.default_message()).collect();
    for message in &messages {
        assert!(!message.is_empty());
        assert!(!message.contains('/') && !message.contains('\\'));
    }
    messages.sort_unstable();
    messages.dedup();
    assert_eq!(messages.len(), kinds.len());
}

#[test]
fn errors_compare_by_class_and_message() {
    let a = UpdateError::new(ErrorKind::Download).with_diagnostic("first attempt");
    let b = UpdateError::new(ErrorKind::Download).with_diagnostic("second attempt");

    assert_eq!(a, b);
    assert_ne!(a, UpdateError::new(ErrorKind::Signature));
}

/// Captures formatted tracing events so the test can assert what was logged.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<(tracing::Level, String)>>>);

struct Fields<'a>(&'a mut String);

impl tracing::field::Visit for Fields<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.0, "{}={:?} ", field.name(), value);
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = String::new();
        event.record(&mut Fields(&mut line));
        self.0
            .lock()
            .unwrap()
            .push((*event.metadata().level(), line));
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

struct Fails;

impl CheckSource for Fails {
    fn check(&self, _request: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        Err(UpdateError::new(ErrorKind::FeedRetrieval).with_diagnostic("connection refused"))
    }
}

#[test]
fn background_check_failures_are_logged_as_warnings() {
    let capture = Capture::default();
    let coordinator = UpdateCoordinator::new(Fails, Capability::SelfManaged);

    tracing::subscriber::with_default(capture.clone(), || {
        let _ = coordinator.check(CheckKind::Background);
    });

    let events = capture.0.lock().unwrap();
    let warning = events
        .iter()
        .find(|(level, _)| *level == tracing::Level::WARN)
        .map(|(_, line)| line.as_str())
        .expect("a warning was logged");
    assert!(warning.contains("FeedRetrieval"), "{warning}");
    assert!(warning.contains("connection refused"), "{warning}");
}

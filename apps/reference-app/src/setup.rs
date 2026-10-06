//! Turns the build-time configuration into an [`UpdaterConfig`].

use std::borrow::Cow;

use gpui_auto_update::UpdaterConfig;
use gpui_auto_update::core::check::{FeedCheckSource, UpdateChecker};
use gpui_auto_update::core::feed::{Arch, Os, UpdateTarget};
use gpui_auto_update::core::fetch::{FetchPolicy, HttpClient};
use gpui_auto_update::core::{
    Capability, CheckOutcome, CheckPolicy, CheckRequest, CheckSource, ErrorKind, UpdateError,
};

use crate::build_config::{BuildConfig, BuildConfigError, DEFAULT_APP_ID};

/// The updater configuration for `build`.
///
/// A build without a usable feed still gets an updater, whose checks fail
/// with a configuration error, so the error state is reachable without a
/// server. The install backend is the facade's default.
pub fn updater_config(build: &Result<BuildConfig, BuildConfigError>) -> UpdaterConfig {
    let build = match build {
        Ok(build) => build,
        Err(error) => {
            return UpdaterConfig::new(
                DEFAULT_APP_ID,
                Misconfigured(format!("This build is misconfigured: {error}.").into()),
            );
        }
    };

    let mut config = UpdaterConfig::new(build.app_id.clone(), check_source(build))
        .allow_debug_self_update(build.allow_debug_self_update);
    if let Some(manager) = &build.externally_managed {
        config = config.with_capability(Capability::ExternallyManaged {
            manager: Some(manager.clone()),
        });
    }
    if let Some(interval) = build.check_interval {
        config = config.with_policy(
            CheckPolicy::recommended()
                .with_minimum_interval(interval)
                .with_periodic_interval(Some(interval)),
        );
    }
    config
}

fn check_source(build: &BuildConfig) -> Box<dyn CheckSource> {
    let Some(feed) = &build.feed else {
        return Box::new(Misconfigured(
            "This build has no update feed. Set REFERENCE_APP_FEED_URL and REFERENCE_APP_PUBLIC_KEY when building it."
                .into(),
        ));
    };
    let (Some(os), Some(arch)) = (Os::current(), Arch::current()) else {
        return Box::new(Misconfigured(
            "Updates are not published for this platform.".into(),
        ));
    };
    let client = HttpClient::new(FetchPolicy {
        allow_insecure_http: feed.allow_insecure_http,
        ..FetchPolicy::default()
    });
    let checker = UpdateChecker::new(feed.url.clone(), UpdateTarget::new(os, arch), client);
    Box::new(FeedCheckSource::new(checker, build.version.clone()))
}

/// A check source that always fails with a configuration error.
struct Misconfigured(Cow<'static, str>);

impl CheckSource for Misconfigured {
    fn check(&self, _: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        Err(UpdateError::new(ErrorKind::Configuration).with_message(self.0.clone()))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, TestAppContext};
    use gpui_auto_update::Updater;
    use gpui_auto_update::core::{MemoryPreferenceStore, UpdateState};

    use super::*;
    use crate::build_config::BuildInputs;

    fn updater(
        cx: &mut TestAppContext,
        build: Result<BuildConfig, BuildConfigError>,
    ) -> gpui::Entity<Updater> {
        updater_with(cx, updater_config(&build))
    }

    /// As if a platform backend reported a self-managed installation, so
    /// checks reach the check source.
    fn self_managed_updater(
        cx: &mut TestAppContext,
        build: Result<BuildConfig, BuildConfigError>,
    ) -> gpui::Entity<Updater> {
        updater_with(
            cx,
            updater_config(&build).with_capability(Capability::SelfManaged),
        )
    }

    fn updater_with(cx: &mut TestAppContext, config: UpdaterConfig) -> gpui::Entity<Updater> {
        let config = config.with_preferences(MemoryPreferenceStore::new());
        let updater = cx.new(|cx| Updater::new(config, cx));
        cx.run_until_parked();
        updater
    }

    fn check(cx: &mut TestAppContext, updater: &gpui::Entity<Updater>) -> UpdateState {
        updater.update(cx, |u, cx| u.check_for_updates(cx));
        cx.run_until_parked();
        updater.read_with(cx, |u, _| u.state())
    }

    #[gpui::test]
    fn a_build_without_a_feed_fails_checks_with_a_configuration_error(cx: &mut TestAppContext) {
        let updater = self_managed_updater(cx, BuildConfig::from_inputs(&BuildInputs::default()));
        match check(cx, &updater) {
            UpdateState::Failed(error) => {
                assert_eq!(error.kind(), ErrorKind::Configuration);
                assert!(error.message().contains("REFERENCE_APP_FEED_URL"));
            }
            state => panic!("unexpected {state:?}"),
        }
    }

    #[gpui::test]
    fn a_misconfigured_build_reports_why_when_checking(cx: &mut TestAppContext) {
        let updater = self_managed_updater(cx, Err(BuildConfigError::MissingPublicKey));
        match check(cx, &updater) {
            UpdateState::Failed(error) => {
                assert_eq!(error.kind(), ErrorKind::Configuration);
                assert!(error.message().contains("REFERENCE_APP_PUBLIC_KEY"));
            }
            state => panic!("unexpected {state:?}"),
        }
    }

    #[gpui::test]
    fn an_externally_managed_build_never_updates_itself(cx: &mut TestAppContext) {
        let build = BuildConfig::from_inputs(&BuildInputs {
            externally_managed: Some("Homebrew"),
            ..BuildInputs::default()
        });
        let updater = updater(cx, build);
        let expected = UpdateState::Disabled {
            capability: Capability::ExternallyManaged {
                manager: Some("Homebrew".into()),
            },
        };
        assert_eq!(updater.read_with(cx, |u, _| u.state()), expected);
        assert_eq!(check(cx, &updater), expected);
        assert!(!updater.read_with(cx, |u, _| u.is_self_update_supported()));
    }
}

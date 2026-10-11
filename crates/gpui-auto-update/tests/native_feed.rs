//! Native-feed configuration with the platform's default backend.

#[cfg(target_os = "linux")]
mod support;

use gpui::TestAppContext;
use gpui_auto_update::core::trust::TrustedKey;
use gpui_auto_update::core::version::ReleaseVersion;
use gpui_auto_update::core::{
    Capability, CheckPolicy, ErrorKind, MemoryPreferenceStore, UpdateState,
};
use gpui_auto_update::{BuildProfile, NativeFeed, UpdaterConfig};

fn feed() -> NativeFeed {
    NativeFeed::new(
        "https://updates.invalid/appcast-linux-x86_64.xml",
        TrustedKey::insecure_test_key(),
        ReleaseVersion::parse("1.0.0").unwrap(),
    )
    .unwrap()
}

#[gpui::test]
fn installs_that_are_not_managed_report_why_they_cannot_update(cx: &mut TestAppContext) {
    // A test binary is never a managed install: on Linux the default backend
    // detects that, and elsewhere no native backend is selected yet.
    let config = UpdaterConfig::native_feed("dev.example.native", feed())
        .with_preferences(MemoryPreferenceStore::new())
        .with_policy(CheckPolicy::recommended().with_check_on_launch(false))
        .with_build_profile(BuildProfile::Release);
    let updater = cx.update(|cx| gpui_auto_update::init(config, cx));
    cx.run_until_parked();

    updater.read_with(cx, |updater, _| {
        assert!(updater.is_ready());
        assert_eq!(updater.capability(), Capability::Unsupported);
        assert_eq!(
            updater.state(),
            UpdateState::Disabled {
                capability: Capability::Unsupported
            }
        );
        assert_eq!(updater.previous_update_failure(), None);
    });
}

#[test]
fn an_invalid_feed_url_is_a_configuration_error() {
    let error = NativeFeed::new(
        "not a url",
        TrustedKey::insecure_test_key(),
        ReleaseVersion::parse("1.0.0").unwrap(),
    )
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Configuration);
}

#[cfg(target_os = "linux")]
#[gpui::test]
fn the_configured_channel_can_be_queried_and_changed(cx: &mut TestAppContext) {
    use gpui_auto_update::UpdaterEvent;
    use gpui_auto_update::core::Channel;
    use gpui_auto_update::core::feed::Channel as FeedChannel;

    let config = UpdaterConfig::native_feed(
        "dev.example.native",
        feed().with_channel(FeedChannel::new("beta").unwrap()),
    )
    .with_preferences(MemoryPreferenceStore::new())
    .with_policy(CheckPolicy::recommended().with_check_on_launch(false))
    .with_build_profile(BuildProfile::Release);
    let updater = cx.update(|cx| gpui_auto_update::init(config, cx));
    cx.run_until_parked();
    assert_eq!(
        updater.read_with(cx, |u, _| u.channel().cloned()),
        Some(Channel::new("beta"))
    );

    let recorder = support::Recorder::new(&updater, cx);
    updater.update(cx, |u, cx| u.set_channel(None, cx)).unwrap();
    cx.run_until_parked();

    assert_eq!(updater.read_with(cx, |u, _| u.channel().cloned()), None);
    assert!(
        recorder
            .events()
            .contains(&UpdaterEvent::ChannelChanged(None))
    );
}

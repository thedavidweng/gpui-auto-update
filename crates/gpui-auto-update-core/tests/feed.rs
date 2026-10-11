//! Parsing and selecting from native feeds, without any network.

mod support;

use gpui_auto_update_core::feed::{
    Arch, Channel, Feed, FeedError, FeedLimits, ItemError, Os, Selection, SystemVersion,
    UpdateTarget,
};
use gpui_auto_update_core::version::ReleaseVersion;
use support::{Item, feed, signing_key};

fn v(text: &str) -> ReleaseVersion {
    ReleaseVersion::parse(text).unwrap()
}

fn linux_x64() -> UpdateTarget {
    UpdateTarget::new(Os::Linux, Arch::X86_64)
}

fn parse(xml: &str) -> Result<Feed, FeedError> {
    Feed::parse(xml.as_bytes(), &FeedLimits::default())
}

fn three_releases() -> Vec<Item> {
    let key = signing_key(1);
    vec![
        Item::signed("1.0.0", &key, b"one"),
        Item::signed("1.2.0", &key, b"one point two"),
        Item::signed("1.1.0", &key, b"one point one"),
    ]
}

fn selected_version(selection: Selection) -> String {
    match selection {
        Selection::UpdateAvailable(update) => update.item.version.to_string(),
        Selection::UpToDate => "up to date".to_owned(),
    }
}

#[test]
fn older_version_discovers_the_newest_release() {
    let feed = parse(&feed(&three_releases())).unwrap();
    let selection = feed.select(&linux_x64(), &v("1.0.0")).unwrap();
    assert_eq!(selected_version(selection), "1.2.0");
}

#[test]
fn feed_ordering_does_not_change_the_selected_release() {
    let mut items = three_releases();
    for _ in 0..items.len() {
        items.rotate_left(1);
        let feed = parse(&feed(&items)).unwrap();
        let selection = feed.select(&linux_x64(), &v("0.9.0")).unwrap();
        assert_eq!(selected_version(selection), "1.2.0");
    }
    items.reverse();
    let feed = parse(&feed(&items)).unwrap();
    assert_eq!(
        selected_version(feed.select(&linux_x64(), &v("0.9.0")).unwrap()),
        "1.2.0"
    );
}

#[test]
fn equal_or_newer_current_version_is_up_to_date() {
    let feed = parse(&feed(&three_releases())).unwrap();
    for current in ["1.2.0", "1.2.0+local.build", "1.2.1", "2.0.0-alpha.1"] {
        let selection = feed.select(&linux_x64(), &v(current)).unwrap();
        assert!(
            matches!(selection, Selection::UpToDate),
            "{current} should be up to date"
        );
    }
}

#[test]
fn selected_update_exposes_artifact_and_release_metadata() {
    let key = signing_key(1);
    let mut item = Item::signed("2.0.0", &key, b"artifact");
    item.short_version = Some("2.0".to_owned());
    item.pub_date = Some("Mon, 05 Oct 2026 12:00:00 +0000".to_owned());
    item.release_notes_link = Some("https://example.com/notes/2.0.0.html".to_owned());
    item.description = Some("<p>New things</p>".to_owned());
    item.minimum_system_version = Some("5.15".to_owned());
    let feed = parse(&feed(&[item.clone()])).unwrap();

    let Selection::UpdateAvailable(update) = feed.select(&linux_x64(), &v("1.0.0")).unwrap() else {
        panic!("expected an update");
    };
    let it = &update.item;
    assert_eq!(it.version, v("2.0.0"));
    assert_eq!(it.display_version.as_deref(), Some("2.0"));
    assert_eq!(it.title.as_deref(), Some("Version 2.0.0"));
    assert_eq!(
        it.published.as_deref(),
        Some("Mon, 05 Oct 2026 12:00:00 +0000")
    );
    assert_eq!(
        it.release_notes_url.as_ref().map(|u| u.as_str()),
        Some("https://example.com/notes/2.0.0.html")
    );
    assert_eq!(it.description.as_deref(), Some("<p>New things</p>"));
    assert_eq!(it.channel, None);
    assert_eq!(
        it.minimum_system_version,
        Some(SystemVersion::parse("5.15").unwrap())
    );
    assert!(!update.is_critical);

    let artifact = &it.artifact;
    assert_eq!(artifact.url.as_str(), item.url);
    assert_eq!(artifact.length, 8);
    assert_eq!(artifact.os, Os::Linux);
    assert_eq!(artifact.arch, Arch::X86_64);
    assert_eq!(artifact.signature.to_base64(), item.signature.unwrap());
    assert_eq!(artifact.content_type.as_deref(), Some("application/gzip"));
}

#[test]
fn entries_for_other_platforms_are_never_selected() {
    let key = signing_key(1);
    let feed = parse(&feed(&[
        Item::signed("1.1.0", &key, b"a"),
        Item::signed("9.0.0", &key, b"b").arch("aarch64"),
        Item::signed("9.0.1", &key, b"c").os("windows"),
        Item::signed("9.0.2", &key, b"d").os("macos"),
    ]))
    .unwrap();
    let selection = feed.select(&linux_x64(), &v("1.0.0")).unwrap();
    assert_eq!(selected_version(selection), "1.1.0");

    let arm = UpdateTarget::new(Os::Linux, Arch::Aarch64);
    assert_eq!(
        selected_version(feed.select(&arm, &v("1.0.0")).unwrap()),
        "9.0.0"
    );
}

#[test]
fn feed_without_entries_for_the_target_is_an_error() {
    let key = signing_key(1);
    let feed = parse(&feed(&[Item::signed("1.1.0", &key, b"a").os("windows")])).unwrap();
    assert!(matches!(
        feed.select(&linux_x64(), &v("1.0.0")),
        Err(FeedError::NoEntriesForTarget { .. })
    ));
}

#[test]
fn empty_feed_is_up_to_date() {
    let feed = parse(&feed(&[])).unwrap();
    assert!(matches!(
        feed.select(&linux_x64(), &v("1.0.0")).unwrap(),
        Selection::UpToDate
    ));
}

#[test]
fn channel_entries_require_opting_in_to_the_channel() {
    let key = signing_key(1);
    let feed = parse(&feed(&[
        Item::signed("1.1.0", &key, b"a"),
        Item::signed("1.2.0-beta.1", &key, b"b").channel("beta"),
    ]))
    .unwrap();

    let stable = linux_x64();
    assert_eq!(
        selected_version(feed.select(&stable, &v("1.0.0")).unwrap()),
        "1.1.0"
    );

    let beta = linux_x64().with_channel(Channel::new("beta").unwrap());
    assert_eq!(
        selected_version(feed.select(&beta, &v("1.0.0")).unwrap()),
        "1.2.0-beta.1"
    );
}

#[test]
fn entries_requiring_a_newer_system_are_skipped() {
    let key = signing_key(1);
    let mut needs_new_os = Item::signed("2.0.0", &key, b"b");
    needs_new_os.minimum_system_version = Some("10.0.22000".to_owned());
    let feed = parse(&feed(&[Item::signed("1.5.0", &key, b"a"), needs_new_os])).unwrap();

    let old_os = UpdateTarget::new(Os::Windows, Arch::X86_64)
        .with_system_version(SystemVersion::parse("10.0.19045").unwrap());
    let windows_feed = parse(&support::feed(&[
        Item::signed("1.5.0", &key, b"a").os("windows"),
        {
            let mut i = Item::signed("2.0.0", &key, b"b").os("windows");
            i.minimum_system_version = Some("10.0.22000".to_owned());
            i
        },
    ]))
    .unwrap();
    assert_eq!(
        selected_version(windows_feed.select(&old_os, &v("1.0.0")).unwrap()),
        "1.5.0"
    );
    let new_os = UpdateTarget::new(Os::Windows, Arch::X86_64)
        .with_system_version(SystemVersion::parse("10.0.22631").unwrap());
    assert_eq!(
        selected_version(windows_feed.select(&new_os, &v("1.0.0")).unwrap()),
        "2.0.0"
    );
    // An unknown system version does not filter.
    assert_eq!(
        selected_version(feed.select(&linux_x64(), &v("1.0.0")).unwrap()),
        "2.0.0"
    );
}

#[test]
fn critical_update_applies_to_versions_below_its_threshold() {
    let key = signing_key(1);
    let mut always = Item::signed("2.0.0", &key, b"x");
    always.critical = Some(None);
    let feed_always = parse(&feed(&[always])).unwrap();
    let Selection::UpdateAvailable(u) = feed_always.select(&linux_x64(), &v("1.9.0")).unwrap()
    else {
        panic!()
    };
    assert!(u.is_critical);

    let mut below = Item::signed("2.0.0", &key, b"x");
    below.critical = Some(Some("1.5.0".to_owned()));
    let feed_below = parse(&feed(&[below])).unwrap();
    let Selection::UpdateAvailable(u) = feed_below.select(&linux_x64(), &v("1.4.0")).unwrap()
    else {
        panic!()
    };
    assert!(u.is_critical);
    let Selection::UpdateAvailable(u) = feed_below.select(&linux_x64(), &v("1.5.0")).unwrap()
    else {
        panic!()
    };
    assert!(!u.is_critical);
}

fn item_error(xml: &str) -> ItemError {
    match parse(xml) {
        Err(FeedError::InvalidItem { reason, .. }) => reason,
        other => panic!("expected an invalid item, got {other:?}"),
    }
}

#[test]
fn unsigned_entry_rejects_the_whole_feed() {
    let key = signing_key(1);
    let mut unsigned = Item::signed("1.3.0", &key, b"x");
    unsigned.signature = None;
    let mut items = three_releases();
    items.push(unsigned);
    assert!(matches!(
        item_error(&feed(&items)),
        ItemError::Missing("sparkle:edSignature")
    ));
}

#[test]
fn malformed_signature_rejects_the_whole_feed() {
    let key = signing_key(1);
    for bad in [
        "",
        "not-base64!!",
        "AAAA",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        let mut item = Item::signed("1.3.0", &key, b"x");
        item.signature = Some(bad.to_owned());
        assert!(
            matches!(item_error(&feed(&[item])), ItemError::Signature(_)),
            "{bad:?}"
        );
    }
}

#[test]
fn malformed_versions_reject_the_whole_feed() {
    let key = signing_key(1);
    for bad in ["../../evil", "1.0", "v1.2.3", "1.2.3/..", "..", "C:\\x", ""] {
        let mut item = Item::signed("1.0.0", &key, b"x");
        item.version = bad.to_owned();
        assert!(
            matches!(item_error(&feed(&[item])), ItemError::Version(_)),
            "{bad:?}"
        );
    }
    let mut item = Item::signed("1.0.0", &key, b"x");
    item.critical = Some(Some("../1".to_owned()));
    assert!(matches!(item_error(&feed(&[item])), ItemError::Version(_)));
}

#[test]
fn missing_or_invalid_artifact_length_rejects_the_feed() {
    let key = signing_key(1);
    for bad in [
        None,
        Some(""),
        Some("-1"),
        Some("12abc"),
        Some("0"),
        Some(" 5"),
    ] {
        let mut item = Item::signed("1.0.0", &key, b"x");
        item.length = bad.map(str::to_owned);
        assert!(
            matches!(
                item_error(&feed(&[item])),
                ItemError::Missing("length") | ItemError::InvalidLength(_)
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn oversized_artifact_rejects_the_feed() {
    let key = signing_key(1);
    let mut item = Item::signed("1.0.0", &key, b"x");
    item.length = Some("1001".to_owned());
    let limits = FeedLimits {
        max_artifact_bytes: 1000,
        ..FeedLimits::default()
    };
    let err = Feed::parse(feed(&[item]).as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(
            err,
            FeedError::InvalidItem {
                reason: ItemError::ArtifactTooLarge {
                    length: 1001,
                    limit: 1000
                },
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn oversized_feed_is_rejected_before_parsing() {
    let xml = feed(&three_releases());
    let limits = FeedLimits {
        max_feed_bytes: xml.len() as u64 - 1,
        ..FeedLimits::default()
    };
    assert!(matches!(
        Feed::parse(xml.as_bytes(), &limits),
        Err(FeedError::TooLarge { .. })
    ));
}

#[test]
fn too_many_items_are_rejected() {
    let limits = FeedLimits {
        max_items: 2,
        ..FeedLimits::default()
    };
    assert!(matches!(
        Feed::parse(feed(&three_releases()).as_bytes(), &limits),
        Err(FeedError::TooManyItems { count: 3, limit: 2 })
    ));
}

#[test]
fn missing_platform_attributes_reject_the_feed() {
    let key = signing_key(1);
    let mut no_os = Item::signed("1.0.0", &key, b"x");
    no_os.os = None;
    assert!(matches!(
        item_error(&feed(&[no_os])),
        ItemError::Missing("sparkle:os")
    ));
    let mut no_arch = Item::signed("1.0.0", &key, b"x");
    no_arch.arch = None;
    assert!(matches!(
        item_error(&feed(&[no_arch])),
        ItemError::Missing("gpui-auto-update:arch")
    ));
    assert!(matches!(
        item_error(&feed(&[Item::signed("1.0.0", &key, b"x").os("plan9")])),
        ItemError::UnknownOs(_)
    ));
    assert!(matches!(
        item_error(&feed(&[
            Item::signed("1.0.0", &key, b"x").arch("x86_64/../..")
        ])),
        ItemError::UnknownArch(_)
    ));
}

#[test]
fn non_web_artifact_urls_reject_the_feed() {
    let key = signing_key(1);
    for bad in [
        "file:///etc/passwd",
        "relative/path.tar.gz",
        "ftp://example.com/a",
    ] {
        assert!(
            matches!(
                item_error(&feed(&[Item::signed("1.0.0", &key, b"x").url(bad)])),
                ItemError::InvalidUrl { .. }
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn invalid_channel_names_reject_the_feed() {
    let key = signing_key(1);
    for bad in ["", "../beta", "has space", "a/b"] {
        assert!(
            matches!(
                item_error(&feed(&[Item::signed("1.0.0", &key, b"x").channel(bad)])),
                ItemError::InvalidChannel(_)
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn duplicate_versions_for_one_platform_reject_the_feed() {
    let key = signing_key(1);
    let xml = feed(&[
        Item::signed("1.0.0", &key, b"x"),
        Item::signed("1.0.0+rebuild", &key, b"y"),
    ]);
    assert!(matches!(
        parse(&xml),
        Err(FeedError::DuplicateVersion { .. })
    ));
}

#[test]
fn duplicate_fields_reject_the_feed() {
    let key = signing_key(1);
    let mut item = Item::signed("1.0.0", &key, b"x");
    item.extra = "      <sparkle:version>9.9.9</sparkle:version>\n".to_owned();
    assert!(matches!(
        item_error(&feed(&[item])),
        ItemError::Duplicate("sparkle:version")
    ));
    let mut item = Item::signed("1.0.0", &key, b"x");
    item.extra = "      <enclosure url=\"https://example.com/b\" length=\"1\"/>\n".to_owned();
    assert!(matches!(
        item_error(&feed(&[item])),
        ItemError::Duplicate("enclosure")
    ));
}

#[test]
fn non_rss_and_malformed_xml_are_rejected() {
    assert!(matches!(parse("<feed/>"), Err(FeedError::NotRss)));
    assert!(matches!(parse("<rss><channel>"), Err(FeedError::Xml(_))));
    assert!(matches!(
        Feed::parse(&[0xff, 0xfe, 0x00], &FeedLimits::default()),
        Err(FeedError::Xml(_))
    ));
}

#[test]
fn document_type_declarations_are_rejected() {
    let xml = r#"<?xml version="1.0"?>
<!DOCTYPE rss [<!ENTITY x "boom">]>
<rss version="2.0"><channel><title>&x;</title></channel></rss>"#;
    assert!(matches!(parse(xml), Err(FeedError::Xml(_))));
}

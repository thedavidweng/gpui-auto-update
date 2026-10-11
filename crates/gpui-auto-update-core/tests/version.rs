//! Release versions taken from feeds.

use gpui_auto_update_core::version::ReleaseVersion;

#[test]
fn newer_release_compares_greater_by_semver_precedence() {
    let old = ReleaseVersion::parse("1.9.9").unwrap();
    let new = ReleaseVersion::parse("1.10.0").unwrap();
    assert!(new.is_newer_than(&old));
    assert!(!old.is_newer_than(&new));
}

#[test]
fn prerelease_is_older_than_its_release() {
    let beta = ReleaseVersion::parse("2.0.0-beta.1").unwrap();
    let release = ReleaseVersion::parse("2.0.0").unwrap();
    assert!(release.is_newer_than(&beta));
}

#[test]
fn build_metadata_does_not_make_a_release_newer() {
    let a = ReleaseVersion::parse("1.0.0+build.1").unwrap();
    let b = ReleaseVersion::parse("1.0.0+build.2").unwrap();
    assert!(!a.is_newer_than(&b));
    assert!(!b.is_newer_than(&a));
}

#[test]
fn display_round_trips_the_original_text() {
    assert_eq!(
        ReleaseVersion::parse("3.1.4-rc.1").unwrap().to_string(),
        "3.1.4-rc.1"
    );
}

#[test]
fn malformed_versions_are_rejected() {
    for bad in [
        "",
        " 1.0.0",
        "1.0.0 ",
        "v1.0.0",
        "1.0",
        "1.0.0.0",
        "../1.0.0",
        "1.0.0/../../etc",
        "1.0.0\\..\\x",
        "1.0.0-..",
        "1.0.0\0",
        "..",
        ".",
        "C:1.0.0",
        "1.0.0-a/b",
    ] {
        assert!(
            ReleaseVersion::parse(bad).is_err(),
            "{bad:?} must not parse as a release version"
        );
    }
}

#[test]
fn overlong_versions_are_rejected() {
    let long = format!("1.0.0-{}", "a".repeat(200));
    assert!(ReleaseVersion::parse(&long).is_err());
}

#[test]
fn path_component_of_a_valid_version_is_a_single_safe_segment() {
    for text in ["0.1.0", "1.2.3-beta.4", "10.0.0+linux.x86-64"] {
        let version = ReleaseVersion::parse(text).unwrap();
        let component = version.path_component();
        assert_eq!(component, text);
        let path = std::path::Path::new(component);
        let parts: Vec<_> = path.components().collect();
        assert_eq!(parts.len(), 1, "{text} must be one path component");
        assert!(matches!(parts[0], std::path::Component::Normal(_)));
    }
}

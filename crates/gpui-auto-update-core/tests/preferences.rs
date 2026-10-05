//! Behavioral tests for automatic-update preference persistence.

use std::time::{Duration, SystemTime};

use gpui_auto_update_core::{
    ErrorKind, FilePreferenceStore, MemoryPreferenceStore, PreferenceOwner, PreferenceStore,
    UpdatePreferences,
};

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

#[test]
fn a_missing_file_has_no_stored_preferences() {
    let dir = tempfile::tempdir().unwrap();
    let store = FilePreferenceStore::new(dir.path().join("updates.json"));

    assert_eq!(store.load().unwrap(), None);
    assert_eq!(store.owner(), PreferenceOwner::Library);
}

#[test]
fn saved_preferences_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("updates.json");
    let saved = UpdatePreferences::new(false).with_last_check(Some(at(1_700_000_000)));

    FilePreferenceStore::new(&path).save(&saved).unwrap();
    let loaded = FilePreferenceStore::new(&path).load().unwrap();

    assert_eq!(loaded, Some(saved));
}

#[test]
fn saving_replaces_the_previous_preferences_without_leaving_temporary_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("updates.json");
    let store = FilePreferenceStore::new(&path);

    store.save(&UpdatePreferences::new(true)).unwrap();
    store
        .save(&UpdatePreferences::new(false).with_last_check(Some(at(42))))
        .unwrap();

    assert_eq!(
        store.load().unwrap(),
        Some(UpdatePreferences::new(false).with_last_check(Some(at(42))))
    );
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("updates.json")]);
}

#[test]
fn a_corrupt_file_is_reported_without_exposing_its_path_to_users() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("updates.json");
    std::fs::write(&path, b"{\"automatic_checks\": tru").unwrap();

    let error = FilePreferenceStore::new(&path).load().unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Preferences);
    assert!(!error.to_string().contains("updates.json"));
    assert!(error.diagnostic().unwrap().contains("updates.json"));
}

#[test]
fn a_corrupt_file_is_replaced_by_the_next_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("updates.json");
    std::fs::write(&path, b"\0\0garbage").unwrap();
    let store = FilePreferenceStore::new(&path);

    store.save(&UpdatePreferences::new(true)).unwrap();

    assert_eq!(store.load().unwrap(), Some(UpdatePreferences::new(true)));
}

#[test]
fn unknown_fields_from_newer_versions_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("updates.json");
    std::fs::write(
        &path,
        br#"{"automatic_checks": false, "future_setting": [1, 2, 3]}"#,
    )
    .unwrap();

    let loaded = FilePreferenceStore::new(&path).load().unwrap();

    assert_eq!(loaded, Some(UpdatePreferences::new(false)));
}

#[test]
fn saving_into_an_unwritable_location_fails_with_a_preferences_error() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("not-a-directory");
    std::fs::write(&blocker, b"").unwrap();
    let store = FilePreferenceStore::new(blocker.join("updates.json"));

    let error = store.save(&UpdatePreferences::new(true)).unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Preferences);
}

#[test]
fn memory_stores_share_preferences_between_clones_and_declare_their_owner() {
    let store = MemoryPreferenceStore::new();
    let clone = store.clone();

    clone.save(&UpdatePreferences::new(false)).unwrap();

    assert_eq!(store.load().unwrap(), Some(UpdatePreferences::new(false)));
    assert_eq!(store.owner(), PreferenceOwner::Library);
    assert_eq!(
        MemoryPreferenceStore::new()
            .with_owner(PreferenceOwner::Backend)
            .owner(),
        PreferenceOwner::Backend
    );
}

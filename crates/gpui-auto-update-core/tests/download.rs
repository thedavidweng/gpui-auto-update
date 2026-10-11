//! Artifact download, verification, and staging against a local HTTP server.

mod support;

use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_auto_update_core::check::{FeedCheckSource, UpdateChecker};
use gpui_auto_update_core::download::{ArtifactDownloader, DownloadError};
use gpui_auto_update_core::feed::{Arch, Feed, FeedItem, FeedLimits, Os, Selection, UpdateTarget};
use gpui_auto_update_core::fetch::{FetchError, FetchPolicy, HttpClient};
use gpui_auto_update_core::version::ReleaseVersion;
use gpui_auto_update_core::{
    AvailableUpdate, Capability, CheckKind, CheckOutcome, DownloadProgress, ErrorKind,
    ReleaseNotes, UpdateCoordinator, UpdateError, UpdateEvent, UpdateState,
};
use support::{Item, feed, signing_key, trusted_key};
use url::Url;

type Handler = dyn Fn(tiny_http::Request) + Send + Sync;

/// Serves requests on a loopback port until the test process exits.
fn serve(handler: impl Fn(tiny_http::Request) + Send + Sync + 'static) -> Url {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = server.server_addr().to_ip().unwrap();
    let handler: Arc<Handler> = Arc::new(handler);
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let handler = Arc::clone(&handler);
            std::thread::spawn(move || handler(request));
        }
    });
    Url::parse(&format!("http://{addr}/")).unwrap()
}

fn bytes(data: impl Into<Vec<u8>>) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    tiny_http::Response::from_data(data.into()).with_chunked_threshold(usize::MAX)
}

fn test_policy() -> FetchPolicy {
    FetchPolicy {
        allow_insecure_http: true,
        timeout: Duration::from_secs(5),
        ..FetchPolicy::default()
    }
}

fn v(text: &str) -> ReleaseVersion {
    ReleaseVersion::parse(text).unwrap()
}

/// The validated feed item for `item`, as a check would select it.
fn selected(item: Item) -> FeedItem {
    let xml = feed(&[item]);
    let feed = Feed::parse(xml.as_bytes(), &FeedLimits::default()).unwrap();
    match feed
        .select(&UpdateTarget::new(Os::Linux, Arch::X86_64), &v("0.1.0"))
        .unwrap()
    {
        Selection::UpdateAvailable(update) => update.item,
        Selection::UpToDate => panic!("expected an update"),
    }
}

/// An item for `version` signed over `signed`, served at `base/artifact`.
fn item_at(base: &Url, version: &str, signed: &[u8]) -> Item {
    Item::signed(version, &signing_key(7), signed).url(base.join("artifact").unwrap().as_str())
}

fn downloader() -> ArtifactDownloader {
    ArtifactDownloader::new(HttpClient::new(test_policy()), trusted_key(&signing_key(7)))
}

#[test]
fn verified_artifact_is_staged_with_its_expected_version() {
    let payload = b"release payload".to_vec();
    let served = payload.clone();
    let base = serve(move |rq| rq.respond(bytes(served.clone())).unwrap());
    let item = selected(item_at(&base, "1.5.0", &payload));
    let staging = tempfile::tempdir().unwrap();

    let staged = downloader()
        .download(&item, staging.path(), |_| Ok(()))
        .unwrap();

    assert_eq!(std::fs::read(staged.path()).unwrap(), payload);
    assert!(staged.path().starts_with(staging.path()));
    assert_eq!(staged.expected_version(), &v("1.5.0"));
    assert_eq!(staged.length(), payload.len() as u64);
}

/// Entries left in `dir`, by file name.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Counts requests and serves `body`.
fn counting(body: Vec<u8>) -> (Url, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let base = serve(move |rq| {
        counter.fetch_add(1, Ordering::SeqCst);
        rq.respond(bytes(body.clone())).unwrap();
    });
    (base, hits)
}

/// A body of `len` bytes that the server sends without `Content-Length`.
struct Chunked {
    remaining: Option<u64>,
}

impl Read for Chunked {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = match &mut self.remaining {
            Some(0) => return Ok(0),
            Some(left) => {
                let n = (*left).min(buf.len() as u64) as usize;
                *left -= n as u64;
                n
            }
            None => buf.len(),
        };
        buf[..n].fill(b'x');
        Ok(n)
    }
}

/// Serves `len` bytes of `x` chunked (no `Content-Length`); `None` serves
/// forever.
fn serve_chunked(len: Option<u64>) -> Url {
    serve(move |rq| {
        let body = Chunked { remaining: len };
        let _ = rq.respond(tiny_http::Response::new(
            tiny_http::StatusCode(200),
            vec![],
            body,
            None,
            None,
        ));
    })
}

#[test]
fn staged_path_is_a_fixed_name_in_a_fresh_directory_whatever_the_feed_url() {
    let payload = b"payload".to_vec();
    let (base, _) = counting(payload.clone());
    let url = base
        .join("dl/..%2F..%2Fevil.exe?name=../../evil.exe")
        .unwrap();
    let item = selected(Item::signed("2.0.0", &signing_key(7), &payload).url(url.as_str()));
    let staging = tempfile::tempdir().unwrap();

    let first = downloader()
        .download(&item, staging.path(), |_| Ok(()))
        .unwrap();
    let second = downloader()
        .download(&item, staging.path(), |_| Ok(()))
        .unwrap();

    for staged in [&first, &second] {
        assert_eq!(staged.path().file_name().unwrap(), "artifact");
        assert_eq!(staged.path().parent().unwrap(), staged.directory());
        assert_eq!(staged.directory().parent().unwrap(), staging.path());
        assert_eq!(entries(staged.directory()), ["artifact"]);
    }
    assert_ne!(first.directory(), second.directory());
}

#[test]
fn backend_chosen_file_name_is_used_for_the_verified_artifact() {
    let payload = b"installer".to_vec();
    let (base, _) = counting(payload.clone());
    let item = selected(item_at(&base, "2.0.0", &payload));
    let staging = tempfile::tempdir().unwrap();

    let staged = downloader()
        .with_file_name("setup-2.0.0.exe")
        .unwrap()
        .download(&item, staging.path(), |_| Ok(()))
        .unwrap();

    assert_eq!(staged.path().file_name().unwrap(), "setup-2.0.0.exe");
}

#[test]
fn file_names_that_are_not_a_single_plain_component_are_refused() {
    for name in [
        "", ".", "..", "../x", "a/b", "a\\b", ".hidden", "c:x", "a b",
    ] {
        assert!(downloader().with_file_name(name).is_err(), "{name:?}");
    }
    assert!(downloader().with_file_name(&"a".repeat(129)).is_err());
}

#[test]
fn progress_is_reported_per_chunk_against_the_declared_length() {
    let payload: Vec<u8> = (0..300_000u32).map(|i| i as u8).collect();
    let (base, _) = counting(payload.clone());
    let item = selected(item_at(&base, "2.0.0", &payload));
    let staging = tempfile::tempdir().unwrap();
    let mut events = Vec::new();

    downloader()
        .download(&item, staging.path(), |event| {
            events.push(event);
            Ok(())
        })
        .unwrap();

    let total = Some(payload.len() as u64);
    assert_eq!(
        events.first(),
        Some(&UpdateEvent::DownloadStarted { total })
    );
    assert_eq!(events.last(), Some(&UpdateEvent::VerificationStarted));
    let progress: Vec<DownloadProgress> = events[1..events.len() - 1]
        .iter()
        .map(|e| match e {
            UpdateEvent::DownloadProgressed(p) => *p,
            other => panic!("unexpected event {other:?}"),
        })
        .collect();
    assert!(progress.len() > 1, "expected several chunks: {progress:?}");
    assert!(
        progress
            .windows(2)
            .all(|w| w[0].downloaded < w[1].downloaded)
    );
    assert!(progress.iter().all(|p| p.total == total));
    assert_eq!(progress.last().unwrap().downloaded, payload.len() as u64);
}

#[test]
fn chunked_response_without_content_length_downloads_and_verifies() {
    let payload = vec![b'x'; 100_000];
    let base = serve_chunked(Some(payload.len() as u64));
    let item = selected(item_at(&base, "2.0.0", &payload));
    let staging = tempfile::tempdir().unwrap();
    let mut last = None;

    let staged = downloader()
        .download(&item, staging.path(), |event| {
            if let UpdateEvent::DownloadProgressed(p) = event {
                last = Some(p);
            }
            Ok(())
        })
        .unwrap();

    assert_eq!(std::fs::read(staged.path()).unwrap(), payload);
    assert_eq!(
        last,
        Some(DownloadProgress {
            downloaded: 100_000,
            total: Some(100_000)
        })
    );
}

#[test]
fn content_length_that_differs_from_the_declared_length_is_rejected() {
    let payload = b"twelve bytes".to_vec();
    for served in [b"twelve byte".to_vec(), b"twelve bytes!".to_vec()] {
        let (base, _) = counting(served.clone());
        let item = selected(item_at(&base, "2.0.0", &payload));
        let staging = tempfile::tempdir().unwrap();

        let err = downloader()
            .download(&item, staging.path(), |_| Ok(()))
            .unwrap_err();

        assert!(
            matches!(err, DownloadError::LengthMismatch { expected: 12, actual } if actual == served.len() as u64),
            "{err:?}"
        );
        assert_eq!(UpdateError::from(err).kind(), ErrorKind::LengthMismatch);
        assert!(entries(staging.path()).is_empty());
    }
}

#[test]
fn chunked_body_shorter_than_declared_is_rejected() {
    let payload = vec![b'x'; 1000];
    let base = serve_chunked(Some(999));
    let item = selected(item_at(&base, "2.0.0", &payload));
    let staging = tempfile::tempdir().unwrap();

    let err = downloader()
        .download(&item, staging.path(), |_| Ok(()))
        .unwrap_err();

    assert!(
        matches!(
            err,
            DownloadError::LengthMismatch {
                expected: 1000,
                actual: 999
            }
        ),
        "{err:?}"
    );
    assert!(entries(staging.path()).is_empty());
}

#[test]
fn endless_chunked_body_is_cut_off_just_past_the_declared_length() {
    let payload = vec![b'x'; 1000];
    let base = serve_chunked(None);
    let item = selected(item_at(&base, "2.0.0", &payload));
    let staging = tempfile::tempdir().unwrap();

    let err = downloader()
        .download(&item, staging.path(), |_| Ok(()))
        .unwrap_err();

    assert!(
        matches!(err, DownloadError::LengthMismatch { expected: 1000, actual } if actual > 1000),
        "{err:?}"
    );
    assert!(entries(staging.path()).is_empty());
}

#[test]
fn declared_length_above_the_size_limit_is_rejected_without_a_request() {
    let payload = vec![b'x'; 2048];
    let (base, hits) = counting(payload.clone());
    let item = selected(item_at(&base, "2.0.0", &payload));
    let staging = tempfile::tempdir().unwrap();

    let err = downloader()
        .with_max_artifact_bytes(1024)
        .download(&item, staging.path(), |_| Ok(()))
        .unwrap_err();

    assert!(
        matches!(
            err,
            DownloadError::ArtifactTooLarge {
                length: 2048,
                limit: 1024
            }
        ),
        "{err:?}"
    );
    assert_eq!(UpdateError::from(err).kind(), ErrorKind::Download);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert!(entries(staging.path()).is_empty());
}

#[test]
fn stalled_transfer_times_out() {
    let payload = vec![b'x'; 1000];
    let base = serve(|rq| {
        let mut writer = rq.into_writer();
        let _ = writer.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\nxxxx");
        let _ = writer.flush();
        std::thread::sleep(Duration::from_secs(3));
    });
    let item = selected(item_at(&base, "2.0.0", &payload));
    let staging = tempfile::tempdir().unwrap();
    let client = HttpClient::new(FetchPolicy {
        timeout: Duration::from_millis(300),
        ..test_policy()
    });

    let started = std::time::Instant::now();
    let err = ArtifactDownloader::new(client, trusted_key(&signing_key(7)))
        .download(&item, staging.path(), |_| Ok(()))
        .unwrap_err();

    assert!(
        matches!(err, DownloadError::Fetch(FetchError::Timeout)),
        "{err:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(entries(staging.path()).is_empty());
}

#[test]
fn tampered_bytes_with_a_valid_signature_are_rejected() {
    let payload = b"genuine release".to_vec();
    let (base, _) = counting(b"tampered release".to_vec()[..15].to_vec());
    let item = selected(item_at(&base, "2.0.0", &payload));
    let staging = tempfile::tempdir().unwrap();

    let err = downloader()
        .download(&item, staging.path(), |_| Ok(()))
        .unwrap_err();

    assert!(matches!(err, DownloadError::BadSignature), "{err:?}");
    assert_eq!(UpdateError::from(err).kind(), ErrorKind::Signature);
    assert!(entries(staging.path()).is_empty());
}

#[test]
fn artifact_signed_by_another_key_is_rejected() {
    let payload = b"release".to_vec();
    let (base, _) = counting(payload.clone());
    let item = selected(
        Item::signed("2.0.0", &signing_key(8), &payload)
            .url(base.join("artifact").unwrap().as_str()),
    );
    let staging = tempfile::tempdir().unwrap();

    let err = downloader()
        .download(&item, staging.path(), |_| Ok(()))
        .unwrap_err();

    assert!(matches!(err, DownloadError::BadSignature), "{err:?}");
}

#[test]
fn unverified_bytes_have_no_final_name_and_are_not_executable() {
    let payload = b"#!/bin/sh\necho hi\n".to_vec();
    let (base, _) = counting(payload.clone());
    let item = selected(item_at(&base, "2.0.0", &payload));
    let staging = tempfile::tempdir().unwrap();
    let root = staging.path().to_owned();
    let mut seen = Vec::new();

    downloader()
        .with_file_name("app.sh")
        .unwrap()
        .download(&item, staging.path(), |event| {
            if event == UpdateEvent::VerificationStarted {
                let [dir] = entries(&root).try_into().unwrap();
                let dir = root.join(dir);
                for name in entries(&dir) {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let path = dir.join(&name);
                        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
                        assert_eq!(mode & 0o177, 0, "{name}: {mode:o}");
                        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode();
                        assert_eq!(dir_mode & 0o077, 0, "{dir_mode:o}");
                    }
                    seen.push(name);
                }
            }
            Ok(())
        })
        .unwrap();

    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_ne!(seen[0], "app.sh");
    assert!(Path::new(&seen[0]).extension().is_none(), "{seen:?}");
}

#[test]
fn callback_error_interrupts_and_cleans_up() {
    let payload = vec![b'x'; 300_000];
    let (base, _) = counting(payload.clone());
    let item = selected(item_at(&base, "2.0.0", &payload));
    let staging = tempfile::tempdir().unwrap();
    let stop = UpdateError::new(ErrorKind::InvalidState);

    let err = downloader()
        .download(&item, staging.path(), |event| match event {
            UpdateEvent::DownloadProgressed(_) => Err(stop.clone()),
            _ => Ok(()),
        })
        .unwrap_err();

    assert!(
        matches!(&err, DownloadError::Interrupted(e) if *e == stop),
        "{err:?}"
    );
    assert_eq!(UpdateError::from(err), stop);
    assert!(entries(staging.path()).is_empty());
}

#[test]
fn discard_removes_the_staging_directory() {
    let payload = b"payload".to_vec();
    let (base, _) = counting(payload.clone());
    let item = selected(item_at(&base, "2.0.0", &payload));
    let staging = tempfile::tempdir().unwrap();
    let staged = downloader()
        .download(&item, staging.path(), |_| Ok(()))
        .unwrap();

    staged.discard().unwrap();

    assert!(entries(staging.path()).is_empty());
}

#[test]
fn missing_staging_root_is_created() {
    let payload = b"payload".to_vec();
    let (base, _) = counting(payload.clone());
    let item = selected(item_at(&base, "2.0.0", &payload));
    let staging = tempfile::tempdir().unwrap();
    let root = staging.path().join("a").join("b");

    let staged = downloader().download(&item, &root, |_| Ok(())).unwrap();

    assert!(staged.path().starts_with(&root));
}

/// A coordinator whose single check returns `update`.
fn coordinator_with(update: Option<AvailableUpdate>) -> UpdateCoordinator {
    struct Fixed(Option<AvailableUpdate>);
    impl gpui_auto_update_core::CheckSource for Fixed {
        fn check(
            &self,
            _: &gpui_auto_update_core::CheckRequest,
        ) -> Result<CheckOutcome, UpdateError> {
            Ok(match &self.0 {
                Some(update) => CheckOutcome::UpdateAvailable(update.clone()),
                None => CheckOutcome::UpToDate,
            })
        }
    }
    let coordinator = UpdateCoordinator::new(Fixed(update), Capability::SelfManaged);
    coordinator.check(CheckKind::Manual).unwrap();
    coordinator
}

fn record(coordinator: &UpdateCoordinator) -> (Arc<Mutex<Vec<UpdateState>>>, impl Drop) {
    let states = Arc::new(Mutex::new(Vec::new()));
    let sink = states.clone();
    let subscription = coordinator.subscribe(move |s| sink.lock().unwrap().push(s.clone()));
    (states, subscription)
}

#[test]
fn coordinator_moves_through_downloading_and_verifying_to_staged() {
    let payload = vec![b'z'; 200_000];
    let (base, _) = counting(payload.clone());
    let item = selected(item_at(&base, "2.0.0", &payload));
    let update = AvailableUpdate::new("2.0.0");
    let coordinator = coordinator_with(Some(update.clone()));
    let (states, _subscription) = record(&coordinator);
    let staging = tempfile::tempdir().unwrap();

    let staged = downloader()
        .download_and_stage(&coordinator, &item, staging.path())
        .unwrap();

    assert_eq!(staged.expected_version(), &v("2.0.0"));
    assert_eq!(coordinator.state(), UpdateState::Staged(update.clone()));
    let states = states.lock().unwrap();
    let total = Some(200_000);
    assert_eq!(
        states.first(),
        Some(&UpdateState::Downloading {
            update: update.clone(),
            progress: DownloadProgress {
                downloaded: 0,
                total
            },
        })
    );
    assert!(states.contains(&UpdateState::Downloading {
        update: update.clone(),
        progress: DownloadProgress {
            downloaded: 200_000,
            total
        },
    }));
    assert_eq!(
        &states[states.len() - 2..],
        [
            UpdateState::Verifying(update.clone()),
            UpdateState::Staged(update)
        ]
    );
}

#[test]
fn coordinator_reports_verification_failure() {
    let payload = b"genuine".to_vec();
    let (base, _) = counting(b"forgery".to_vec());
    let item = selected(item_at(&base, "2.0.0", &payload));
    let coordinator = coordinator_with(Some(AvailableUpdate::new("2.0.0")));
    let staging = tempfile::tempdir().unwrap();

    let err = downloader()
        .download_and_stage(&coordinator, &item, staging.path())
        .unwrap_err();

    assert_eq!(err.kind(), ErrorKind::Signature);
    assert_eq!(coordinator.state(), UpdateState::Failed(err));
    assert!(entries(staging.path()).is_empty());
}

#[test]
fn coordinator_without_an_available_update_downloads_nothing() {
    let payload = b"payload".to_vec();
    let (base, hits) = counting(payload.clone());
    let item = selected(item_at(&base, "2.0.0", &payload));
    let coordinator = coordinator_with(None);
    let staging = tempfile::tempdir().unwrap();

    let err = downloader()
        .download_and_stage(&coordinator, &item, staging.path())
        .unwrap_err();

    assert_eq!(err.kind(), ErrorKind::InvalidState);
    assert_eq!(coordinator.state(), UpdateState::UpToDate);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert!(entries(staging.path()).is_empty());
}

#[test]
fn feed_check_source_drives_a_check_and_download_end_to_end() {
    let payload = b"release 1.5.0".to_vec();
    let key = signing_key(7);
    let artifact_body = payload.clone();
    let feed_xml = Arc::new(Mutex::new(String::new()));
    let xml = feed_xml.clone();
    let base = serve(move |rq| {
        let body = if rq.url() == "/appcast.xml" {
            xml.lock().unwrap().clone().into_bytes()
        } else {
            artifact_body.clone()
        };
        rq.respond(bytes(body)).unwrap();
    });
    let mut item = Item::signed("1.5.0", &key, &payload)
        .url(base.join("artifact").unwrap().as_str())
        .channel("beta");
    item.short_version = Some("1.5".to_owned());
    item.release_notes_link = Some("https://example.com/notes/1.5.0.html".to_owned());
    item.pub_date = Some("Mon, 05 Oct 2026 12:00:00 +0000".to_owned());
    item.critical = Some(None);
    *feed_xml.lock().unwrap() = feed(&[item]);

    let target = UpdateTarget::new(Os::Linux, Arch::X86_64)
        .with_channel(gpui_auto_update_core::feed::Channel::new("beta").unwrap());
    let checker = UpdateChecker::new(
        base.join("appcast.xml").unwrap(),
        target,
        HttpClient::new(test_policy()),
    );
    let source = Arc::new(FeedCheckSource::new(checker, v("1.4.0")));
    let coordinator = UpdateCoordinator::new(source.clone(), Capability::SelfManaged);

    let outcome = coordinator.check(CheckKind::Manual).unwrap();

    let expected = AvailableUpdate::new("1.5")
        .with_build("1.5.0")
        .with_channel(gpui_auto_update_core::Channel::new("beta"))
        .with_release_notes(ReleaseNotes::Link(
            "https://example.com/notes/1.5.0.html".to_owned(),
        ))
        .with_published("Mon, 05 Oct 2026 12:00:00 +0000")
        .with_critical(true);
    assert_eq!(outcome, CheckOutcome::UpdateAvailable(expected.clone()));
    let selected = source.selected().unwrap();
    assert_eq!(selected.item.version, v("1.5.0"));

    let staging = tempfile::tempdir().unwrap();
    let staged = downloader()
        .download_and_stage(&coordinator, &selected.item, staging.path())
        .unwrap();

    assert_eq!(std::fs::read(staged.path()).unwrap(), payload);
    assert_eq!(staged.expected_version(), &v("1.5.0"));
    assert_eq!(coordinator.state(), UpdateState::Staged(expected));
}

#[test]
fn feed_check_source_maps_failures_and_up_to_date() {
    let key = signing_key(7);
    let xml = feed(&[Item::signed("1.5.0", &key, b"x")]);
    let base = serve(move |rq| {
        if rq.url() == "/appcast.xml" {
            rq.respond(bytes(xml.clone())).unwrap();
        } else {
            rq.respond(bytes("nope").with_status_code(404)).unwrap();
        }
    });
    let source = |path: &str, current: &str| {
        FeedCheckSource::new(
            UpdateChecker::new(
                base.join(path).unwrap(),
                UpdateTarget::new(Os::Linux, Arch::X86_64),
                HttpClient::new(test_policy()),
            ),
            v(current),
        )
    };
    let request = gpui_auto_update_core::CheckRequest::new(CheckKind::Manual);
    use gpui_auto_update_core::CheckSource as _;

    let up_to_date = source("appcast.xml", "1.5.0");
    assert_eq!(up_to_date.check(&request), Ok(CheckOutcome::UpToDate));
    assert!(up_to_date.selected().is_none());

    let err = source("missing.xml", "1.0.0").check(&request).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::FeedRetrieval);

    let wrong_platform = FeedCheckSource::new(
        UpdateChecker::new(
            base.join("appcast.xml").unwrap(),
            UpdateTarget::new(Os::Windows, Arch::X86_64),
            HttpClient::new(test_policy()),
        ),
        v("1.0.0"),
    );
    let err = wrong_platform.check(&request).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Configuration);
}

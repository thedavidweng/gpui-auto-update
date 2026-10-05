//! Update checks against a local deterministic HTTP server.

mod support;

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use gpui_auto_update_core::check::{CheckError, UpdateChecker};
use gpui_auto_update_core::feed::{Arch, FeedError, FeedLimits, Os, Selection, UpdateTarget};
use gpui_auto_update_core::fetch::{FetchError, FetchPolicy, HttpClient};
use gpui_auto_update_core::version::ReleaseVersion;
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

fn header(text: &str) -> tiny_http::Header {
    text.parse().unwrap()
}

fn bytes(data: impl Into<Vec<u8>>) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    tiny_http::Response::from_data(data.into()).with_chunked_threshold(usize::MAX)
}

/// Loopback tests use plain HTTP; production policy refuses it.
fn test_policy() -> FetchPolicy {
    FetchPolicy {
        allow_insecure_http: true,
        timeout: Duration::from_secs(5),
        ..FetchPolicy::default()
    }
}

fn checker(feed_url: Url) -> UpdateChecker {
    UpdateChecker::new(
        feed_url,
        UpdateTarget::new(Os::Linux, Arch::X86_64),
        HttpClient::new(test_policy()),
    )
}

fn v(text: &str) -> ReleaseVersion {
    ReleaseVersion::parse(text).unwrap()
}

fn signed_feed() -> String {
    let key = signing_key(9);
    feed(&[
        Item::signed("1.4.0", &key, b"old"),
        Item::signed("1.5.0", &key, b"new"),
    ])
}

#[test]
fn old_version_discovers_newer_signed_update_over_http() {
    let xml = signed_feed();
    let base = serve(move |rq| {
        assert_eq!(rq.url(), "/appcast-linux-x86_64.xml");
        rq.respond(bytes(xml.clone())).unwrap();
    });
    let outcome = checker(base.join("appcast-linux-x86_64.xml").unwrap())
        .check(&v("1.4.0"))
        .unwrap();
    let Selection::UpdateAvailable(update) = outcome else {
        panic!("expected an update, got {outcome:?}");
    };
    assert_eq!(update.item.version, v("1.5.0"));
}

#[test]
fn current_or_newer_version_is_up_to_date_over_http() {
    let xml = signed_feed();
    let base = serve(move |rq| rq.respond(bytes(xml.clone())).unwrap());
    for current in ["1.5.0", "2.0.0"] {
        let outcome = checker(base.clone()).check(&v(current)).unwrap();
        assert!(matches!(outcome, Selection::UpToDate), "{current}");
    }
}

#[test]
fn discovered_artifact_signature_verifies_with_the_trusted_key() {
    let key = signing_key(9);
    let artifact = b"release payload".to_vec();
    let xml = feed(&[Item::signed("2.0.0", &key, &artifact)]);
    let base = serve(move |rq| rq.respond(bytes(xml.clone())).unwrap());
    let Selection::UpdateAvailable(update) = checker(base).check(&v("1.0.0")).unwrap() else {
        panic!("expected an update");
    };
    let a = &update.item.artifact;
    trusted_key(&key)
        .verify_artifact(&a.signature, a.length, artifact.as_slice())
        .unwrap();
}

#[test]
fn unsigned_release_fails_the_check() {
    let key = signing_key(9);
    let mut unsigned = Item::signed("2.0.0", &key, b"x");
    unsigned.signature = None;
    let xml = feed(&[Item::signed("1.0.0", &key, b"y"), unsigned]);
    let base = serve(move |rq| rq.respond(bytes(xml.clone())).unwrap());
    let err = checker(base).check(&v("1.0.0")).unwrap_err();
    assert!(
        matches!(
            err,
            CheckError::Feed(FeedError::InvalidItem { index: 1, .. })
        ),
        "{err:?}"
    );
}

#[test]
fn oversized_feed_with_content_length_is_rejected() {
    let base = serve(|rq| rq.respond(bytes(vec![b' '; 4096])).unwrap());
    let limits = FeedLimits {
        max_feed_bytes: 1024,
        ..FeedLimits::default()
    };
    let err = checker(base)
        .with_limits(limits)
        .check(&v("1.0.0"))
        .unwrap_err();
    assert!(
        matches!(err, CheckError::Fetch(FetchError::TooLarge { limit: 1024 })),
        "{err:?}"
    );
}

#[test]
fn oversized_chunked_feed_is_rejected() {
    let base = serve(|rq| {
        let body = std::io::Cursor::new(vec![b' '; 64 * 1024]);
        let response = tiny_http::Response::new(200.into(), vec![], body, None, None)
            .with_chunked_threshold(0);
        rq.respond(response).unwrap();
    });
    let limits = FeedLimits {
        max_feed_bytes: 1024,
        ..FeedLimits::default()
    };
    let err = checker(base)
        .with_limits(limits)
        .check(&v("1.0.0"))
        .unwrap_err();
    assert!(
        matches!(err, CheckError::Fetch(FetchError::TooLarge { limit: 1024 })),
        "{err:?}"
    );
}

#[test]
fn slow_server_times_out() {
    let base = serve(|rq| {
        std::thread::sleep(Duration::from_secs(3));
        let _ = rq.respond(bytes("late"));
    });
    let client = HttpClient::new(FetchPolicy {
        timeout: Duration::from_millis(300),
        ..test_policy()
    });
    let started = std::time::Instant::now();
    let err = client.get_bytes(&base, 1024).unwrap_err();
    assert!(matches!(err, FetchError::Timeout), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn http_error_status_fails_the_check() {
    let base = serve(|rq| rq.respond(bytes("nope").with_status_code(404)).unwrap());
    let err = checker(base).check(&v("1.0.0")).unwrap_err();
    assert!(
        matches!(err, CheckError::Fetch(FetchError::Status(404))),
        "{err:?}"
    );
}

#[test]
fn production_policy_refuses_plain_http() {
    let client = HttpClient::new(FetchPolicy::default());
    let err = client
        .get_bytes(&Url::parse("http://127.0.0.1:9/feed.xml").unwrap(), 1024)
        .unwrap_err();
    assert!(matches!(err, FetchError::InsecureUrl(_)), "{err:?}");
}

#[test]
fn redirects_within_the_limit_are_followed() {
    let xml = signed_feed();
    let base = serve(move |rq| match rq.url() {
        "/latest.xml" => rq
            .respond(
                bytes("")
                    .with_status_code(302)
                    .with_header(header("Location: /releases/appcast.xml")),
            )
            .unwrap(),
        "/releases/appcast.xml" => rq.respond(bytes(xml.clone())).unwrap(),
        other => panic!("unexpected request {other}"),
    });
    let outcome = checker(base.join("latest.xml").unwrap())
        .check(&v("1.0.0"))
        .unwrap();
    assert!(matches!(outcome, Selection::UpdateAvailable(_)));
}

#[test]
fn redirect_loops_are_bounded() {
    let base = serve(|rq| {
        rq.respond(
            bytes("")
                .with_status_code(301)
                .with_header(header("Location: /loop")),
        )
        .unwrap()
    });
    let client = HttpClient::new(FetchPolicy {
        max_redirects: 3,
        ..test_policy()
    });
    let err = client.get_bytes(&base, 1024).unwrap_err();
    assert!(matches!(err, FetchError::TooManyRedirects), "{err:?}");
}

#[test]
fn redirects_are_refused_when_disabled() {
    let base = serve(|rq| {
        rq.respond(
            bytes("")
                .with_status_code(302)
                .with_header(header("Location: /elsewhere")),
        )
        .unwrap()
    });
    let client = HttpClient::new(FetchPolicy {
        max_redirects: 0,
        ..test_policy()
    });
    assert!(matches!(
        client.get_bytes(&base, 1024),
        Err(FetchError::TooManyRedirects)
    ));
}

#[test]
fn redirects_to_non_web_schemes_are_refused() {
    let base = serve(|rq| {
        rq.respond(
            bytes("")
                .with_status_code(302)
                .with_header(header("Location: file:///etc/passwd")),
        )
        .unwrap()
    });
    let err = HttpClient::new(test_policy())
        .get_bytes(&base, 1024)
        .unwrap_err();
    assert!(matches!(err, FetchError::InsecureUrl(_)), "{err:?}");
}

#[test]
fn redirect_without_location_is_an_error() {
    let base = serve(|rq| rq.respond(bytes("").with_status_code(302)).unwrap());
    let err = HttpClient::new(test_policy())
        .get_bytes(&base, 1024)
        .unwrap_err();
    assert!(matches!(err, FetchError::InvalidRedirect), "{err:?}");
}

#[test]
fn streamed_download_reports_length_and_stops_at_the_limit() {
    let base = serve(|rq| rq.respond(bytes(vec![7u8; 10_000])).unwrap());
    let client = HttpClient::new(test_policy());

    let mut download = client.open(&base, 20_000).unwrap();
    assert_eq!(download.content_length(), Some(10_000));
    let mut body = Vec::new();
    download.read_to_end(&mut body).unwrap();
    assert_eq!(body.len(), 10_000);

    let err = client.open(&base, 5_000).unwrap_err();
    assert!(
        matches!(err, FetchError::TooLarge { limit: 5_000 }),
        "{err:?}"
    );
}

#[test]
fn streamed_chunked_download_is_cut_off_at_the_limit() {
    let base = serve(|rq| {
        let body = std::io::Cursor::new(vec![1u8; 10_000]);
        let response = tiny_http::Response::new(200.into(), vec![], body, None, None)
            .with_chunked_threshold(0);
        rq.respond(response).unwrap();
    });
    let mut download = HttpClient::new(test_policy()).open(&base, 5_000).unwrap();
    assert_eq!(download.content_length(), None);
    let mut body = Vec::new();
    assert!(download.read_to_end(&mut body).is_err());
    assert!(body.len() <= 5_000);
}

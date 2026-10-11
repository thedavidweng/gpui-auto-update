//! Bounded, blocking HTTP(S) fetching for feeds and artifacts.
//!
//! Every request has a connect timeout and an overall timeout, every body
//! has an explicit size limit, and redirects are followed by this module
//! rather than by the HTTP library so that each hop is checked: the hop count
//! is bounded, only `https` (and, when explicitly allowed, `http`) targets
//! are followed, and an `https` request is never redirected to `http`.
//!
//! Calls block the current thread; run them on a background executor.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use url::Url;

/// Transport rules for [`HttpClient`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchPolicy {
    /// Time allowed to establish a connection.
    pub connect_timeout: Duration,
    /// Time allowed for each request, from start until the body has been
    /// fully read. Applies per hop when redirects are followed.
    pub timeout: Duration,
    /// Redirects followed before failing. `0` refuses all redirects.
    pub max_redirects: u32,
    /// Permit plain `http` URLs. Intended only for loopback test servers;
    /// Ed25519 verification still applies, but metadata is unprotected in
    /// transit. Even when set, an `https` URL never redirects to `http`.
    pub allow_insecure_http: bool,
}

impl Default for FetchPolicy {
    /// 10 s connect, 30 s per request, up to 5 redirects, `https` only.
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            timeout: Duration::from_secs(30),
            max_redirects: 5,
            allow_insecure_http: false,
        }
    }
}

/// Why a fetch failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    /// The URL (or a redirect target) uses a scheme the policy does not
    /// allow. Carries the scheme.
    #[error("refusing to fetch a {0:?} URL")]
    InsecureUrl(String),
    /// More redirects than [`FetchPolicy::max_redirects`].
    #[error("too many redirects")]
    TooManyRedirects,
    /// A redirect had no usable `Location`.
    #[error("redirect has a missing or invalid Location")]
    InvalidRedirect,
    /// The server answered with a non-success status.
    #[error("server responded with HTTP status {0}")]
    Status(u16),
    /// The body is larger than the allowed size.
    #[error("response is larger than the {limit}-byte limit")]
    TooLarge {
        /// The limit that was exceeded.
        limit: u64,
    },
    /// The connection or request timed out.
    #[error("request timed out")]
    Timeout,
    /// Any other network, TLS, or protocol failure.
    #[error("network error: {0}")]
    Transport(String),
}

/// A blocking HTTP client that enforces a [`FetchPolicy`].
#[derive(Debug, Clone)]
pub struct HttpClient {
    agent: ureq::Agent,
    policy: FetchPolicy,
}

/// A response body being streamed. Reading fails with an I/O error once more
/// bytes than the requested limit arrive.
pub struct Download {
    reader: ureq::BodyReader<'static>,
    content_length: Option<u64>,
    url: Url,
    limit: u64,
    remaining: u64,
}

impl Download {
    /// The `Content-Length` the server declared, if any.
    pub fn content_length(&self) -> Option<u64> {
        self.content_length
    }

    /// The final URL after redirects.
    pub fn url(&self) -> &Url {
        &self.url
    }
}

impl Read for Download {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Reading at most one byte past the limit tells a body of exactly
        // `limit` bytes (followed by end of stream) from a longer one.
        let max = usize::try_from(self.remaining.saturating_add(1))
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let n = self.reader.read(&mut buf[..max])?;
        if n as u64 > self.remaining {
            self.remaining = 0;
            return Err(ureq::Error::BodyExceedsLimit(self.limit).into_io());
        }
        self.remaining -= n as u64;
        Ok(n)
    }
}

impl std::fmt::Debug for Download {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Download")
            .field("url", &self.url.as_str())
            .field("content_length", &self.content_length)
            .finish_non_exhaustive()
    }
}

impl HttpClient {
    /// Builds a client for `policy`.
    pub fn new(policy: FetchPolicy) -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .timeout_connect(Some(policy.connect_timeout))
            .timeout_global(Some(policy.timeout))
            // Redirects are followed by `send` so every hop is validated.
            .max_redirects(0)
            .http_status_as_error(false)
            .https_only(!policy.allow_insecure_http)
            .user_agent(concat!("gpui-auto-update/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self { agent, policy }
    }

    /// The policy this client enforces.
    pub fn policy(&self) -> &FetchPolicy {
        &self.policy
    }

    /// Fetches a whole body of at most `max_bytes` bytes. A body of exactly
    /// `max_bytes` bytes is accepted.
    pub fn get_bytes(&self, url: &Url, max_bytes: u64) -> Result<Vec<u8>, FetchError> {
        let mut body = Vec::new();
        self.open(url, max_bytes)?
            .read_to_end(&mut body)
            .map_err(|e| read_error(e, max_bytes))?;
        Ok(body)
    }

    /// Starts a streamed download of at most `max_bytes` bytes. Reading fails
    /// once the body exceeds `max_bytes`; a body of exactly `max_bytes`
    /// bytes reads to completion.
    pub fn open(&self, url: &Url, max_bytes: u64) -> Result<Download, FetchError> {
        let (response, url) = self.send(url, max_bytes)?;
        let body = response.into_body();
        let content_length = body.content_length();
        // ureq's own limit fails a body whose length equals it, so the exact
        // bound is enforced by `Download::read`.
        let reader = body
            .into_with_config()
            .limit(max_bytes.saturating_add(1))
            .reader();
        Ok(Download {
            reader,
            content_length,
            url,
            limit: max_bytes,
            remaining: max_bytes,
        })
    }

    fn send(
        &self,
        url: &Url,
        max_bytes: u64,
    ) -> Result<(ureq::http::Response<ureq::Body>, Url), FetchError> {
        let mut current = url.clone();
        self.check_scheme(&current)?;
        let mut redirects = 0;
        loop {
            let response = self
                .agent
                .get(current.as_str())
                .call()
                .map_err(|e| map_error(e, max_bytes))?;
            let status = response.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                if redirects >= self.policy.max_redirects {
                    return Err(FetchError::TooManyRedirects);
                }
                redirects += 1;
                let next = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|loc| current.join(loc).ok())
                    .ok_or(FetchError::InvalidRedirect)?;
                self.check_scheme(&next)?;
                if current.scheme() == "https" && next.scheme() != "https" {
                    return Err(FetchError::InsecureUrl(next.scheme().to_owned()));
                }
                current = next;
                continue;
            }
            if status != 200 {
                return Err(FetchError::Status(status));
            }
            if response
                .body()
                .content_length()
                .is_some_and(|n| n > max_bytes)
            {
                return Err(FetchError::TooLarge { limit: max_bytes });
            }
            return Ok((response, current));
        }
    }

    fn check_scheme(&self, url: &Url) -> Result<(), FetchError> {
        match url.scheme() {
            "https" => Ok(()),
            "http" if self.policy.allow_insecure_http => Ok(()),
            other => Err(FetchError::InsecureUrl(other.to_owned())),
        }
    }
}

/// Classifies an I/O error returned while reading a [`Download`] opened with
/// a `limit`-byte bound.
pub(crate) fn read_error(error: std::io::Error, limit: u64) -> FetchError {
    if error.kind() == std::io::ErrorKind::TimedOut {
        return FetchError::Timeout;
    }
    // ureq reports body failures (timeouts, the size limit) as its own error
    // wrapped in an `io::Error`.
    if error
        .get_ref()
        .is_some_and(|inner| inner.is::<ureq::Error>())
    {
        if let Some(Ok(inner)) = error.into_inner().map(|e| e.downcast::<ureq::Error>()) {
            return map_error(*inner, limit);
        }
        return FetchError::Transport("unreadable response body".to_owned());
    }
    FetchError::Transport(error.to_string())
}

fn map_error(error: ureq::Error, limit: u64) -> FetchError {
    match error {
        ureq::Error::Timeout(_) => FetchError::Timeout,
        ureq::Error::BodyExceedsLimit(_) => FetchError::TooLarge { limit },
        ureq::Error::RequireHttpsOnly(_) => FetchError::InsecureUrl("http".to_owned()),
        ureq::Error::Io(e) if e.kind() == std::io::ErrorKind::TimedOut => FetchError::Timeout,
        other => FetchError::Transport(other.to_string()),
    }
}

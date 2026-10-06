//! Update configuration fixed at compile time.
//!
//! End-to-end tests build the same sources twice, as version N and N+1, so
//! every value that distinguishes the two builds (version, feed, trusted key)
//! comes from environment variables read by `option_env!` while compiling:
//!
//! | Variable | Meaning | Default |
//! | --- | --- | --- |
//! | `REFERENCE_APP_ID` | Reverse-DNS application identifier | `dev.gpui-auto-update.reference-app` |
//! | `REFERENCE_APP_VERSION` | Version of this build (strict semver) | `1.0.0` |
//! | `REFERENCE_APP_FEED_URL` | Native update feed | none (checks fail with a configuration error) |
//! | `REFERENCE_APP_PUBLIC_KEY` | Base64 Ed25519 public key; required with a feed | none |
//! | `REFERENCE_APP_ALLOW_INSECURE_HTTP` | Allow an `http` feed on a loopback host | `false` |
//! | `REFERENCE_APP_ALLOW_DEBUG_SELF_UPDATE` | Let a debug build install updates | `false` |
//! | `REFERENCE_APP_EXTERNALLY_MANAGED` | Package manager name; marks the install externally managed | none |
//! | `REFERENCE_APP_CHECK_INTERVAL_SECS` | Periodic automatic check interval | the library default |
//! | `REFERENCE_APP_E2E_REPORT` | Absolute path; run unattended and append what happens to it (see `unattended`) | none |
//! | `REFERENCE_APP_E2E_FAIL_TO_START` | Exit with an error before the main window opens, as a broken release | `false` |
//!
//! Empty values count as unset. Nothing is read at run time.

use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

use gpui_auto_update::core::trust::TrustedKey;
use gpui_auto_update::core::version::ReleaseVersion;
use url::{Host, Url};

/// The application identifier when none is configured.
pub const DEFAULT_APP_ID: &str = "dev.gpui-auto-update.reference-app";

/// The version when none is configured.
pub const DEFAULT_VERSION: &str = "1.0.0";

/// The raw, unvalidated build-time values.
#[derive(Clone, Copy, Debug, Default)]
pub struct BuildInputs<'a> {
    pub app_id: Option<&'a str>,
    pub version: Option<&'a str>,
    pub feed_url: Option<&'a str>,
    pub public_key: Option<&'a str>,
    pub allow_insecure_http: Option<&'a str>,
    pub allow_debug_self_update: Option<&'a str>,
    pub externally_managed: Option<&'a str>,
    pub check_interval_secs: Option<&'a str>,
    pub e2e_report: Option<&'a str>,
    pub e2e_fail_to_start: Option<&'a str>,
}

impl BuildInputs<'static> {
    /// The values this binary was compiled with.
    pub const fn compiled() -> Self {
        Self {
            app_id: option_env!("REFERENCE_APP_ID"),
            version: option_env!("REFERENCE_APP_VERSION"),
            feed_url: option_env!("REFERENCE_APP_FEED_URL"),
            public_key: option_env!("REFERENCE_APP_PUBLIC_KEY"),
            allow_insecure_http: option_env!("REFERENCE_APP_ALLOW_INSECURE_HTTP"),
            allow_debug_self_update: option_env!("REFERENCE_APP_ALLOW_DEBUG_SELF_UPDATE"),
            externally_managed: option_env!("REFERENCE_APP_EXTERNALLY_MANAGED"),
            check_interval_secs: option_env!("REFERENCE_APP_CHECK_INTERVAL_SECS"),
            e2e_report: option_env!("REFERENCE_APP_E2E_REPORT"),
            e2e_fail_to_start: option_env!("REFERENCE_APP_E2E_FAIL_TO_START"),
        }
    }
}

/// Validated build-time configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildConfig {
    pub app_id: String,
    pub version: ReleaseVersion,
    pub feed: Option<FeedConfig>,
    pub allow_debug_self_update: bool,
    /// The package manager that owns this installation, when it is
    /// externally managed.
    pub externally_managed: Option<String>,
    pub check_interval: Option<Duration>,
    /// Where an unattended end-to-end build reports what happens.
    pub e2e_report: Option<PathBuf>,
    /// Whether this build is a broken release that exits before its main
    /// window opens.
    pub e2e_fail_to_start: bool,
}

/// Where updates come from and who must have signed them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedConfig {
    pub url: Url,
    pub public_key: TrustedKey,
    pub allow_insecure_http: bool,
}

/// Why the build-time configuration is unusable.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BuildConfigError {
    #[error("REFERENCE_APP_VERSION is invalid: {0}")]
    Version(String),
    #[error("REFERENCE_APP_FEED_URL is not a URL: {0}")]
    FeedUrl(String),
    #[error(
        "REFERENCE_APP_FEED_URL must use https, or http on a loopback host with REFERENCE_APP_ALLOW_INSECURE_HTTP"
    )]
    InsecureFeed,
    #[error("REFERENCE_APP_PUBLIC_KEY is required when a feed is configured")]
    MissingPublicKey,
    #[error("REFERENCE_APP_PUBLIC_KEY is invalid: {0}")]
    PublicKey(String),
    #[error("{name} must be a boolean (1/0, true/false, yes/no), not {value:?}")]
    Flag { name: &'static str, value: String },
    #[error("REFERENCE_APP_CHECK_INTERVAL_SECS must be a positive number of seconds, not {0:?}")]
    CheckInterval(String),
    #[error("REFERENCE_APP_E2E_REPORT must be an absolute path, not {0:?}")]
    E2eReport(String),
}

impl BuildConfig {
    /// The configuration this binary was compiled with.
    pub fn compiled() -> Result<Self, BuildConfigError> {
        Self::from_inputs(&BuildInputs::compiled())
    }

    /// Validates `inputs`.
    pub fn from_inputs(inputs: &BuildInputs<'_>) -> Result<Self, BuildConfigError> {
        let version = ReleaseVersion::parse(set(inputs.version).unwrap_or(DEFAULT_VERSION))
            .map_err(|error| BuildConfigError::Version(error.to_string()))?;
        let allow_insecure_http = flag(
            "REFERENCE_APP_ALLOW_INSECURE_HTTP",
            inputs.allow_insecure_http,
        )?;
        let allow_debug_self_update = flag(
            "REFERENCE_APP_ALLOW_DEBUG_SELF_UPDATE",
            inputs.allow_debug_self_update,
        )?;
        let feed = match set(inputs.feed_url) {
            None => None,
            Some(text) => Some(feed(text, set(inputs.public_key), allow_insecure_http)?),
        };
        let check_interval = set(inputs.check_interval_secs)
            .map(|text| match text.parse::<u64>() {
                Ok(secs) if secs > 0 => Ok(Duration::from_secs(secs)),
                _ => Err(BuildConfigError::CheckInterval(text.to_owned())),
            })
            .transpose()?;
        let e2e_report = set(inputs.e2e_report)
            .map(|text| {
                let path = PathBuf::from(text);
                if path.is_absolute() {
                    Ok(path)
                } else {
                    Err(BuildConfigError::E2eReport(text.to_owned()))
                }
            })
            .transpose()?;
        let e2e_fail_to_start = flag("REFERENCE_APP_E2E_FAIL_TO_START", inputs.e2e_fail_to_start)?;
        Ok(Self {
            app_id: set(inputs.app_id).unwrap_or(DEFAULT_APP_ID).to_owned(),
            version,
            feed,
            allow_debug_self_update,
            externally_managed: set(inputs.externally_managed).map(str::to_owned),
            check_interval,
            e2e_report,
            e2e_fail_to_start,
        })
    }
}

fn set(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn flag(name: &'static str, value: Option<&str>) -> Result<bool, BuildConfigError> {
    match set(value).map(str::to_ascii_lowercase).as_deref() {
        None | Some("0" | "false" | "no") => Ok(false),
        Some("1" | "true" | "yes") => Ok(true),
        Some(other) => Err(BuildConfigError::Flag {
            name,
            value: other.to_owned(),
        }),
    }
}

fn feed(
    url: &str,
    public_key: Option<&str>,
    allow_insecure_http: bool,
) -> Result<FeedConfig, BuildConfigError> {
    let url = Url::parse(url).map_err(|error| BuildConfigError::FeedUrl(error.to_string()))?;
    let secure = match url.scheme() {
        "https" => true,
        "http" => allow_insecure_http && is_loopback(&url),
        _ => false,
    };
    if !secure {
        return Err(BuildConfigError::InsecureFeed);
    }
    let public_key = TrustedKey::from_base64(public_key.ok_or(BuildConfigError::MissingPublicKey)?)
        .map_err(|error| BuildConfigError::PublicKey(error.to_string()))?;
    Ok(FeedConfig {
        url,
        public_key,
        allow_insecure_http,
    })
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => IpAddr::V4(ip).is_loopback(),
        Some(Host::Ipv6(ip)) => IpAddr::V6(ip).is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Base64 of the public half of the core crate's insecure test key.
    fn key() -> String {
        TrustedKey::insecure_test_key().to_base64()
    }

    #[test]
    fn unset_inputs_give_an_unconfigured_1_0_0_build() {
        let config = BuildConfig::from_inputs(&BuildInputs::default()).unwrap();
        assert_eq!(config.app_id, "dev.gpui-auto-update.reference-app");
        assert_eq!(config.version.as_str(), "1.0.0");
        assert_eq!(config.feed, None);
        assert!(!config.allow_debug_self_update);
        assert_eq!(config.externally_managed, None);
        assert_eq!(config.check_interval, None);
    }

    #[test]
    fn empty_inputs_count_as_unset() {
        let inputs = BuildInputs {
            version: Some(""),
            feed_url: Some("  "),
            allow_insecure_http: Some(""),
            ..BuildInputs::default()
        };
        let config = BuildConfig::from_inputs(&inputs).unwrap();
        assert_eq!(config.version.as_str(), "1.0.0");
        assert_eq!(config.feed, None);
    }

    #[test]
    fn version_n_plus_one_build() {
        let key = key();
        let inputs = BuildInputs {
            app_id: Some("dev.example.e2e"),
            version: Some("1.0.1"),
            feed_url: Some("https://updates.example.com/feed.xml"),
            public_key: Some(&key),
            allow_debug_self_update: Some("1"),
            externally_managed: Some("Homebrew"),
            check_interval_secs: Some("60"),
            ..BuildInputs::default()
        };
        let config = BuildConfig::from_inputs(&inputs).unwrap();
        assert_eq!(config.app_id, "dev.example.e2e");
        assert_eq!(config.version.as_str(), "1.0.1");
        let feed = config.feed.unwrap();
        assert_eq!(feed.url.as_str(), "https://updates.example.com/feed.xml");
        assert!(feed.public_key.is_insecure_test_key());
        assert!(!feed.allow_insecure_http);
        assert!(config.allow_debug_self_update);
        assert_eq!(config.externally_managed.as_deref(), Some("Homebrew"));
        assert_eq!(config.check_interval, Some(Duration::from_secs(60)));
    }

    #[test]
    fn end_to_end_builds_run_unattended_and_can_be_broken() {
        let inputs = BuildInputs {
            e2e_report: Some("/tmp/e2e/report.log"),
            e2e_fail_to_start: Some("yes"),
            ..BuildInputs::default()
        };
        let config = BuildConfig::from_inputs(&inputs).unwrap();
        assert_eq!(
            config.e2e_report.as_deref(),
            Some(std::path::Path::new("/tmp/e2e/report.log"))
        );
        assert!(config.e2e_fail_to_start);

        let plain = BuildConfig::from_inputs(&BuildInputs::default()).unwrap();
        assert_eq!(plain.e2e_report, None);
        assert!(!plain.e2e_fail_to_start);
    }

    #[test]
    fn a_relative_end_to_end_report_path_is_rejected() {
        let inputs = BuildInputs {
            e2e_report: Some("report.log"),
            ..BuildInputs::default()
        };
        assert!(matches!(
            BuildConfig::from_inputs(&inputs),
            Err(BuildConfigError::E2eReport(_))
        ));
    }

    #[test]
    fn invalid_version_is_rejected() {
        let inputs = BuildInputs {
            version: Some("v1.0"),
            ..BuildInputs::default()
        };
        assert!(matches!(
            BuildConfig::from_inputs(&inputs),
            Err(BuildConfigError::Version(_))
        ));
    }

    #[test]
    fn a_feed_requires_a_valid_public_key() {
        let missing = BuildInputs {
            feed_url: Some("https://updates.example.com/feed.xml"),
            ..BuildInputs::default()
        };
        assert_eq!(
            BuildConfig::from_inputs(&missing),
            Err(BuildConfigError::MissingPublicKey)
        );
        let invalid = BuildInputs {
            public_key: Some("not base64!"),
            ..missing
        };
        assert!(matches!(
            BuildConfig::from_inputs(&invalid),
            Err(BuildConfigError::PublicKey(_))
        ));
    }

    #[test]
    fn plain_http_is_only_allowed_on_loopback_when_opted_in() {
        let key = key();
        let with = |url: &'static str, insecure: Option<&'static str>| {
            let key = key.clone();
            move || {
                BuildConfig::from_inputs(&BuildInputs {
                    feed_url: Some(url),
                    public_key: Some(&key),
                    allow_insecure_http: insecure,
                    ..BuildInputs::default()
                })
                .map(|config| config.feed.unwrap().allow_insecure_http)
            }
        };
        assert_eq!(
            with("http://127.0.0.1:8080/feed.xml", None)(),
            Err(BuildConfigError::InsecureFeed)
        );
        assert_eq!(
            with("http://127.0.0.1:8080/feed.xml", Some("true"))(),
            Ok(true)
        );
        assert_eq!(
            with("http://localhost:8080/feed.xml", Some("yes"))(),
            Ok(true)
        );
        assert_eq!(with("http://[::1]:8080/feed.xml", Some("1"))(), Ok(true));
        assert_eq!(
            with("http://updates.example.com/feed.xml", Some("1"))(),
            Err(BuildConfigError::InsecureFeed)
        );
        assert_eq!(
            with("file:///tmp/feed.xml", Some("1"))(),
            Err(BuildConfigError::InsecureFeed)
        );
    }

    #[test]
    fn malformed_flags_and_intervals_are_rejected() {
        let bad_flag = BuildInputs {
            allow_debug_self_update: Some("maybe"),
            ..BuildInputs::default()
        };
        assert!(matches!(
            BuildConfig::from_inputs(&bad_flag),
            Err(BuildConfigError::Flag {
                name: "REFERENCE_APP_ALLOW_DEBUG_SELF_UPDATE",
                ..
            })
        ));
        for interval in ["0", "-5", "soon"] {
            let inputs = BuildInputs {
                check_interval_secs: Some(interval),
                ..BuildInputs::default()
            };
            assert!(matches!(
                BuildConfig::from_inputs(&inputs),
                Err(BuildConfigError::CheckInterval(_))
            ));
        }
    }
}

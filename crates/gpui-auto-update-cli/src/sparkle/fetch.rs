//! Acquiring an official Sparkle distribution archive and verifying it
//! against a pinned or explicitly declared SHA-256 before extraction.

use std::fs;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Args;
use sha2::Digest as _;

use super::bundle;
use super::pins::Pins;

/// Upper bound for archives that are not pinned (pinned ones are capped at
/// their recorded size).
const MAX_UNPINNED_ARCHIVE: u64 = 256 * 1024 * 1024;
const MAX_UNPACKED: u64 = 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;

#[derive(Debug, Args)]
pub struct FetchArgs {
    /// Pinned Sparkle version to fetch. Defaults to the tool's current pin.
    #[arg(long)]
    version: Option<String>,
    /// Download from this URL (for example a mirror) instead of the official
    /// release URL. The archive must still match the expected checksum.
    #[arg(long, conflicts_with = "archive")]
    url: Option<String>,
    /// Use an archive that is already on disk instead of downloading.
    #[arg(long)]
    archive: Option<PathBuf>,
    /// Expected SHA-256 (hex) of a distribution that this tool does not pin.
    /// Requires --url or --archive.
    #[arg(long)]
    sha256: Option<String>,
    /// Directory to extract the distribution into. It must not exist or be
    /// empty.
    #[arg(long)]
    out: PathBuf,
}

struct Expected {
    version: Option<String>,
    sha256: String,
    max_size: u64,
}

pub fn run(args: FetchArgs) -> Result<ExitCode> {
    ensure_new_or_empty(&args.out)?;
    let pins = Pins::load()?;

    let (expected, default_url) = match &args.sha256 {
        Some(declared) => {
            if args.url.is_none() && args.archive.is_none() {
                bail!("--sha256 declares a custom distribution and needs --url or --archive");
            }
            let sha256 = normalize_sha256(declared)?;
            if let Some(pin) = args.version.as_deref().and_then(|v| pins.get(v)) {
                if pin.sha256 != sha256 {
                    bail!(
                        "--sha256 {sha256} conflicts with the pinned checksum {} for Sparkle {}",
                        pin.sha256,
                        pin.version
                    );
                }
            }
            let expected = Expected {
                version: args.version.clone(),
                sha256,
                max_size: MAX_UNPINNED_ARCHIVE,
            };
            (expected, None)
        }
        None => {
            let pin = pins.resolve(args.version.as_deref())?;
            let expected = Expected {
                version: Some(pin.version.clone()),
                sha256: pin.sha256.clone(),
                max_size: pin.size,
            };
            (expected, Some(pin.url.clone()))
        }
    };

    let bytes = match (&args.archive, args.url.as_ref().or(default_url.as_ref())) {
        (Some(path), _) => read_limited(path, expected.max_size)?,
        (None, Some(url)) => download(url, expected.max_size)?,
        (None, None) => unreachable!("a pinned release always has a URL"),
    };

    let actual = hex(&sha2::Sha256::digest(&bytes));
    if actual != expected.sha256 {
        bail!(
            "checksum mismatch for the Sparkle archive: expected sha256 {}, got {actual}; nothing was extracted",
            expected.sha256
        );
    }

    let parent = absolute_parent(&args.out)?;
    fs::create_dir_all(&parent).with_context(|| format!("cannot create {}", parent.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".sparkle-fetch-")
        .tempdir_in(&parent)
        .context("cannot create a staging directory")?;
    extract(&bytes, staging.path())?;
    let version = check_layout(staging.path(), expected.version.as_deref())?;

    if args.out.exists() {
        fs::remove_dir(&args.out)
            .with_context(|| format!("cannot replace {}", args.out.display()))?;
    }
    fs::rename(staging.path(), &args.out)
        .with_context(|| format!("cannot move the distribution to {}", args.out.display()))?;
    // The staging path no longer exists; dropping the guard is a no-op.
    drop(staging);

    println!(
        "Sparkle {version} (sha256:{actual}) extracted to {}",
        args.out.display()
    );
    Ok(ExitCode::SUCCESS)
}

fn ensure_new_or_empty(out: &Path) -> Result<()> {
    match fs::read_dir(out) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                bail!("output directory {} is not empty", out.display());
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("cannot inspect {}", out.display())),
    }
}

fn absolute_parent(out: &Path) -> Result<PathBuf> {
    let abs = std::path::absolute(out)?;
    abs.parent()
        .map(Path::to_path_buf)
        .with_context(|| format!("{} has no parent directory", out.display()))
}

fn normalize_sha256(s: &str) -> Result<String> {
    let s = s.trim().to_ascii_lowercase();
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("--sha256 must be 64 hexadecimal characters");
    }
    Ok(s)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn read_limited(path: &Path, max: u64) -> Result<Vec<u8>> {
    let file = fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        bail!("{} is larger than the expected {max} bytes", path.display());
    }
    Ok(bytes)
}

/// Scheme and host of an absolute URL, without a URL-parsing dependency.
pub(super) fn scheme_and_host(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        bracketed.split(']').next()?
    } else {
        host_port.split(':').next()?
    };
    (!host.is_empty()).then_some((scheme, host))
}

fn download(url: &str, max: u64) -> Result<Vec<u8>> {
    let (scheme, host) = scheme_and_host(url).with_context(|| format!("invalid URL {url}"))?;
    let https = match scheme {
        "https" => true,
        "http" if matches!(host, "127.0.0.1" | "localhost" | "::1") => false,
        _ => bail!("refusing to download {url}: Sparkle archives must be fetched over https"),
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_global(Some(Duration::from_secs(600)))
        // GitHub release assets redirect to a CDN; https_only keeps every hop
        // on TLS and the checksum pins the content regardless of the host.
        .max_redirects(if https { 5 } else { 0 })
        .https_only(https)
        .http_status_as_error(false)
        .user_agent(concat!("gpui-auto-update-cli/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    eprintln!("downloading {url}");
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("cannot download {url}"))?;
    let status = response.status().as_u16();
    if status != 200 {
        bail!("downloading {url} failed with HTTP status {status}");
    }
    let too_large = || anyhow::anyhow!("{url} is larger than the expected {max} bytes");
    // ureq rejects a body whose length reaches the limit, so allow one extra
    // byte and enforce the exact bound below.
    let bytes = response
        .body_mut()
        .with_config()
        .limit(max + 1)
        .read_to_vec()
        .map_err(|e| match e {
            ureq::Error::BodyExceedsLimit(_) => too_large(),
            e => anyhow::Error::new(e).context(format!("cannot download {url}")),
        })?;
    if bytes.len() as u64 > max {
        return Err(too_large());
    }
    Ok(bytes)
}

/// Relative path made only of normal components, or `None` if the path is
/// absolute or climbs out of the extraction root.
fn safe_relative(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

/// Whether a relative symlink placed at `link` stays inside the root.
fn symlink_stays_inside(link: &Path, target: &Path) -> bool {
    let mut depth = link.components().count().saturating_sub(1);
    for component in target.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => match depth.checked_sub(1) {
                Some(d) => depth = d,
                None => return false,
            },
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

fn extract(bytes: &[u8], root: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(liblzma::read::XzDecoder::new(bytes));
    archive.set_overwrite(false);
    let mut count = 0usize;
    let mut unpacked = 0u64;
    for entry in archive
        .entries()
        .context("the archive is not a valid .tar.xz")?
    {
        let mut entry = entry.context("the archive is corrupt")?;
        count += 1;
        if count > MAX_ENTRIES {
            bail!("the archive has more than {MAX_ENTRIES} entries");
        }
        let path = entry.path()?.into_owned();
        let rel = safe_relative(&path)
            .with_context(|| format!("the archive contains an unsafe path: {}", path.display()))?;
        match entry.header().entry_type() {
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                unpacked += entry.size();
                if unpacked > MAX_UNPACKED {
                    bail!("the archive unpacks to more than {MAX_UNPACKED} bytes");
                }
            }
            tar::EntryType::Directory => {}
            tar::EntryType::Symlink => {
                let target = entry
                    .link_name()?
                    .with_context(|| format!("symlink {} has no target", path.display()))?;
                if !symlink_stays_inside(&rel, &target) {
                    bail!(
                        "the archive contains a symlink that points outside it: {} -> {}",
                        path.display(),
                        target.display()
                    );
                }
            }
            other => bail!(
                "the archive contains an unsupported entry type {other:?}: {}",
                path.display()
            ),
        }
        if rel.as_os_str().is_empty() {
            continue;
        }
        if !entry
            .unpack_in(root)
            .with_context(|| format!("cannot extract {}", path.display()))?
        {
            bail!("the archive contains an unsafe path: {}", path.display());
        }
    }
    Ok(())
}

fn check_layout(root: &Path, expected_version: Option<&str>) -> Result<String> {
    let license = root.join("LICENSE");
    if !license.is_file() {
        bail!(
            "the archive has no LICENSE file; Sparkle's license notice must be redistributed with the framework"
        );
    }
    let framework = root.join(bundle::FRAMEWORK_NAME);
    if !framework.is_dir() {
        bail!("the archive has no {}", bundle::FRAMEWORK_NAME);
    }
    let version = bundle::framework_version(&framework)?;
    if let Some(expected) = expected_version {
        if version != expected {
            bail!("the archive contains Sparkle {version}, but {expected} was requested");
        }
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_extracted_from_urls() {
        assert_eq!(
            scheme_and_host("https://github.com/a/b"),
            Some(("https", "github.com"))
        );
        assert_eq!(
            scheme_and_host("http://127.0.0.1:8080/x"),
            Some(("http", "127.0.0.1"))
        );
        assert_eq!(scheme_and_host("http://[::1]:80/x"), Some(("http", "::1")));
        assert_eq!(
            scheme_and_host("http://localhost@evil.example/x"),
            Some(("http", "evil.example"))
        );
        assert_eq!(scheme_and_host("not a url"), None);
    }

    #[test]
    fn symlinks_are_checked_relative_to_their_directory() {
        let link = Path::new("Sparkle.framework/Versions/Current");
        assert!(symlink_stays_inside(link, Path::new("B")));
        let top = Path::new("Sparkle.framework/Sparkle");
        assert!(symlink_stays_inside(
            top,
            Path::new("Versions/Current/Sparkle")
        ));
        assert!(symlink_stays_inside(top, Path::new("../LICENSE")));
        assert!(!symlink_stays_inside(top, Path::new("../../etc")));
        assert!(!symlink_stays_inside(top, Path::new("/etc/passwd")));
    }
}

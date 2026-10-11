//! `feed native`: sign a Windows or Linux artifact and add it to that
//! platform's native feed.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_auto_update_core::feed::{Arch, Feed, FeedItem, FeedLimits, NATIVE_NS, Os, SPARKLE_NS};
use gpui_auto_update_core::version::ReleaseVersion;
use url::Url;

use super::TrustPolicy;
use super::args::{Failure, ParsedArgs, Spec, usage};

const SPEC: Spec = Spec {
    values: &[
        "--os",
        "--arch",
        "--version",
        "--artifact",
        "--download-url-prefix",
        "--url",
        "--output",
        "--feed",
        "--feed-title",
        "--title",
        "--display-version",
        "--pub-date",
        "--channel",
        "--minimum-system-version",
        "--critical-below",
        "--release-notes-url",
        "--full-release-notes-url",
        "--description-file",
        "--type",
        "--public-key",
        "--bridge-from-public-key",
    ],
    switches: &["--critical", "--allow-http", "--allow-test-key"],
};

/// One release entry, as written to the feed.
struct Entry {
    version: ReleaseVersion,
    display_version: Option<String>,
    title: Option<String>,
    published: Option<String>,
    channel: Option<String>,
    minimum_system_version: Option<String>,
    critical: Option<Option<String>>,
    release_notes_url: Option<String>,
    full_release_notes_url: Option<String>,
    description: Option<String>,
    url: String,
    length: u64,
    signature: String,
    os: Os,
    arch: Arch,
    content_type: Option<String>,
}

impl From<&FeedItem> for Entry {
    fn from(item: &FeedItem) -> Self {
        Self {
            version: item.version.clone(),
            display_version: item.display_version.clone(),
            title: item.title.clone(),
            published: item.published.clone(),
            channel: item.channel.as_ref().map(ToString::to_string),
            minimum_system_version: item
                .minimum_system_version
                .as_ref()
                .map(ToString::to_string),
            critical: item
                .critical
                .as_ref()
                .map(|below| below.as_ref().map(ToString::to_string)),
            release_notes_url: item.release_notes_url.as_ref().map(ToString::to_string),
            full_release_notes_url: item
                .full_release_notes_url
                .as_ref()
                .map(ToString::to_string),
            description: item.description.clone(),
            url: item.artifact.url.to_string(),
            length: item.artifact.length,
            signature: item.artifact.signature.to_base64(),
            os: item.artifact.os,
            arch: item.artifact.arch,
            content_type: item.artifact.content_type.clone(),
        }
    }
}

pub fn run(args: impl Iterator<Item = String>) -> Result<(), Failure> {
    let args = ParsedArgs::parse(args, &SPEC)?;
    let os = match args.required("--os")? {
        "windows" => Os::Windows,
        "linux" => Os::Linux,
        "macos" => {
            return usage(
                "macOS updates use Sparkle's appcast; generate it with `feed sparkle` instead",
            );
        }
        _ => return usage("--os must be `windows` or `linux`"),
    };
    let Some(arch) = Arch::parse(args.required("--arch")?) else {
        return usage(
            "--arch must be `x86_64` or `aarch64` (aliases such as amd64 or arm64 are not accepted)",
        );
    };
    let version = ReleaseVersion::parse(args.required("--version")?)
        .map_err(|e| Failure::Usage(format!("--version: {e}")))?;
    let artifact_path = args
        .path("--artifact")?
        .ok_or_else(|| Failure::Usage("--artifact is required".into()))?;
    let output = args
        .path("--output")?
        .ok_or_else(|| Failure::Usage("--output is required".into()))?;
    let trust = TrustPolicy::from_args(&args)?;
    let Some(source) = args.single_source()? else {
        return usage(
            "a key source is required: --key-stdin, --key-env <VAR>, or --key-file <path>",
        );
    };
    let limits = FeedLimits::default();

    let (mut entries, feed_title) = match args.path("--feed")? {
        Some(path) => load_existing(&path, os, arch, &limits)?,
        None => (Vec::new(), None),
    };
    if let Some(existing) = entries
        .iter()
        .find(|e| e.version.cmp_precedence(&version).is_eq())
    {
        return Err(Failure::Failed(format!(
            "the feed already lists version {} for {os}/{arch}; versioned artifacts are \
             immutable, so publish a new version instead",
            existing.version
        )));
    }

    let url = artifact_url(&args, &artifact_path, &version)?;
    if let Some(existing) = entries.iter().find(|e| e.url == url.as_str()) {
        return Err(Failure::Failed(format!(
            "{url} is already the artifact of version {}; every version needs its own \
             immutable artifact URL",
            existing.version
        )));
    }

    let bytes = read_artifact(&artifact_path, limits.max_artifact_bytes)?;
    let key = source.read()?;
    let signer = key.public_key();
    trust.check(&signer)?;
    let signature = key.sign(&bytes);
    drop(key);
    signer
        .verify_artifact(&signature, bytes.len() as u64, bytes.as_slice())
        .map_err(|e| format!("the new signature failed its own verification: {e}"))?;

    let description = args
        .path("--description-file")?
        .map(|p| {
            fs::read_to_string(&p)
                .map_err(|e| Failure::Failed(format!("cannot read {}: {e}", p.display())))
        })
        .transpose()?;
    let critical = if let Some(below) = args.value("--critical-below")? {
        Some(Some(below.to_owned()))
    } else if args.switch("--critical") {
        Some(None)
    } else {
        None
    };
    let content_type = args
        .value("--type")?
        .map(str::to_owned)
        .unwrap_or_else(|| default_content_type(&artifact_path).to_owned());
    let display_version = args.value("--display-version")?.map(str::to_owned);
    let title = args
        .value("--title")?
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "Version {}",
                display_version.as_deref().unwrap_or(version.as_str())
            )
        });
    entries.push(Entry {
        version: version.clone(),
        display_version,
        title: Some(title),
        published: Some(
            args.value("--pub-date")?
                .map(str::to_owned)
                .unwrap_or_else(now_rfc822),
        ),
        channel: args.value("--channel")?.map(str::to_owned),
        minimum_system_version: args.value("--minimum-system-version")?.map(str::to_owned),
        critical,
        release_notes_url: args.value("--release-notes-url")?.map(str::to_owned),
        full_release_notes_url: args.value("--full-release-notes-url")?.map(str::to_owned),
        description,
        url: url.to_string(),
        length: bytes.len() as u64,
        signature: signature.to_base64(),
        os,
        arch,
        content_type: Some(content_type),
    });
    entries.sort_by(|a, b| b.version.cmp_precedence(&a.version));

    let title = args
        .value("--feed-title")?
        .map(str::to_owned)
        .or(feed_title);
    let document = render(title.as_deref(), &entries);
    // Everything an installed copy will check is re-validated by the same
    // parser the updater uses; an entry it would reject is never written.
    let parsed = Feed::parse(document.as_bytes(), &limits)
        .map_err(|e| format!("refusing to write a feed the updater would reject: {e}"))?;
    if parsed.items().len() != entries.len() {
        return Err(Failure::Failed(
            "refusing to write a feed whose entries do not round-trip".into(),
        ));
    }
    write_atomically(&output, document.as_bytes())?;

    eprintln!(
        "Signed {} ({} bytes) for {os}/{arch} with key {}.",
        artifact_path.display(),
        bytes.len(),
        signer.to_base64()
    );
    println!(
        "{}: version {version} for {os}/{arch}, {} entr{}",
        output.display(),
        entries.len(),
        if entries.len() == 1 { "y" } else { "ies" }
    );
    Ok(())
}

/// Reads an existing feed for the same platform, rejecting it as a whole if
/// the updater would (for example because an entry is unsigned).
fn load_existing(
    path: &Path,
    os: Os,
    arch: Arch,
    limits: &FeedLimits,
) -> Result<(Vec<Entry>, Option<String>), Failure> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            return Err(Failure::Failed(format!(
                "cannot read {}: {e}",
                path.display()
            )));
        }
    };
    let feed = Feed::parse(&bytes, limits)
        .map_err(|e| format!("existing feed {} is invalid: {e}", path.display()))?;
    if let Some(other) = feed
        .items()
        .iter()
        .find(|i| i.artifact.os != os || i.artifact.arch != arch)
    {
        return Err(Failure::Failed(format!(
            "existing feed {} has an entry for {}/{} (version {}); publish one feed per \
             operating system and architecture",
            path.display(),
            other.artifact.os,
            other.artifact.arch,
            other.version
        )));
    }
    Ok((
        feed.items().iter().map(Entry::from).collect(),
        channel_title(&bytes),
    ))
}

fn channel_title(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let doc = roxmltree::Document::parse(text).ok()?;
    let channel = doc
        .root_element()
        .children()
        .find(|n| n.has_tag_name("channel"))?;
    let title = channel.children().find(|n| n.has_tag_name("title"))?;
    title.text().map(str::to_owned)
}

fn artifact_url(
    args: &ParsedArgs,
    artifact: &Path,
    version: &ReleaseVersion,
) -> Result<Url, Failure> {
    let url = match (args.value("--url")?, args.value("--download-url-prefix")?) {
        (Some(url), None) => {
            Url::parse(url).map_err(|e| Failure::Usage(format!("--url is not a URL: {e}")))?
        }
        (None, Some(prefix)) => {
            let mut base = Url::parse(prefix)
                .map_err(|e| Failure::Usage(format!("--download-url-prefix is not a URL: {e}")))?;
            if !base.path().ends_with('/') {
                let path = format!("{}/", base.path());
                base.set_path(&path);
            }
            let name = artifact
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| Failure::Usage("--artifact has no UTF-8 file name".into()))?;
            let mut url = base;
            url.path_segments_mut()
                .map_err(|()| Failure::Usage("--download-url-prefix cannot be a base".into()))?
                .pop_if_empty()
                .push(name);
            url
        }
        (Some(_), Some(_)) => return usage("give only one of --url or --download-url-prefix"),
        (None, None) => {
            return usage(
                "the artifact URL is required: --download-url-prefix <url> or --url <url>",
            );
        }
    };
    match url.scheme() {
        "https" => {}
        "http" if args.switch("--allow-http") => {
            eprintln!(
                "warning: http:// artifact URL; installed copies only download https by default"
            );
        }
        _ => {
            return usage(
                "artifact URLs must use https (pass --allow-http only for local testing)",
            );
        }
    }
    if url.query().is_some() || url.fragment().is_some() {
        return usage("artifact URLs must not have a query or fragment");
    }
    if !url.path().contains(version.as_str()) {
        return Err(Failure::Failed(format!(
            "artifact URL {url} does not contain the version {version}; use an immutable, \
             versioned URL so that a published artifact is never replaced"
        )));
    }
    Ok(url)
}

fn read_artifact(path: &Path, max: u64) -> Result<Vec<u8>, Failure> {
    let fail = |e: std::io::Error| Failure::Failed(format!("cannot read {}: {e}", path.display()));
    let meta = fs::metadata(path).map_err(fail)?;
    if !meta.is_file() {
        return Err(Failure::Failed(format!("{} is not a file", path.display())));
    }
    if meta.len() == 0 {
        return Err(Failure::Failed(format!("{} is empty", path.display())));
    }
    if meta.len() > max {
        return Err(Failure::Failed(format!(
            "{} is {} bytes, more than the updater's {max}-byte artifact limit",
            path.display(),
            meta.len()
        )));
    }
    let bytes = fs::read(path).map_err(fail)?;
    if bytes.len() as u64 != meta.len() {
        return Err(Failure::Failed(format!(
            "{} changed while it was being read",
            path.display()
        )));
    }
    Ok(bytes)
}

fn default_content_type(artifact: &Path) -> &'static str {
    let name = artifact
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        "application/gzip"
    } else if name.ends_with(".tar.xz") {
        "application/x-xz"
    } else if name.ends_with(".zip") {
        "application/zip"
    } else if name.ends_with(".msi") {
        "application/x-msi"
    } else {
        "application/octet-stream"
    }
}

fn render(title: Option<&str>, entries: &[Entry]) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    let _ = writeln!(
        s,
        "<rss version=\"2.0\" xmlns:sparkle=\"{SPARKLE_NS}\" xmlns:gpui-auto-update=\"{NATIVE_NS}\">"
    );
    s.push_str("  <channel>\n");
    if let Some(title) = title {
        element(&mut s, 4, "title", title);
    }
    for e in entries {
        s.push_str("    <item>\n");
        let field = |s: &mut String, name: &str, value: &Option<String>| {
            if let Some(v) = value {
                element(s, 6, name, v);
            }
        };
        field(&mut s, "title", &e.title);
        field(&mut s, "pubDate", &e.published);
        element(&mut s, 6, "sparkle:version", e.version.as_str());
        field(&mut s, "sparkle:shortVersionString", &e.display_version);
        field(&mut s, "sparkle:channel", &e.channel);
        field(
            &mut s,
            "sparkle:minimumSystemVersion",
            &e.minimum_system_version,
        );
        match &e.critical {
            None => {}
            Some(None) => s.push_str("      <sparkle:criticalUpdate/>\n"),
            Some(Some(below)) => {
                let _ = writeln!(
                    s,
                    "      <sparkle:criticalUpdate sparkle:version=\"{}\"/>",
                    escape(below)
                );
            }
        }
        field(&mut s, "sparkle:releaseNotesLink", &e.release_notes_url);
        field(
            &mut s,
            "sparkle:fullReleaseNotesLink",
            &e.full_release_notes_url,
        );
        field(&mut s, "description", &e.description);
        s.push_str("      <enclosure");
        let attr = |s: &mut String, name: &str, value: &str| {
            let _ = write!(s, "\n          {name}=\"{}\"", escape(value));
        };
        attr(&mut s, "url", &e.url);
        attr(&mut s, "length", &e.length.to_string());
        if let Some(t) = &e.content_type {
            attr(&mut s, "type", t);
        }
        attr(&mut s, "sparkle:os", e.os.as_str());
        attr(&mut s, "gpui-auto-update:arch", e.arch.as_str());
        attr(&mut s, "sparkle:edSignature", &e.signature);
        s.push_str("/>\n    </item>\n");
    }
    s.push_str("  </channel>\n</rss>\n");
    s
}

fn element(s: &mut String, indent: usize, name: &str, text: &str) {
    let _ = writeln!(s, "{:indent$}<{name}>{}</{name}>", "", escape(text));
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
    out
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    let fail = |e: std::io::Error| Failure::Failed(format!("cannot write {}: {e}", path.display()));
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    fs::create_dir_all(&dir).map_err(fail)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".feed-")
        .suffix(".xml")
        .tempfile_in(&dir)
        .map_err(fail)?;
    tmp.write_all(bytes).map_err(fail)?;
    tmp.as_file().sync_all().map_err(fail)?;
    tmp.persist(path).map_err(|e| fail(e.error))?;
    Ok(())
}

fn now_rfc822() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    rfc822(secs)
}

/// Formats a Unix time as an RSS (RFC 822) date in UTC.
fn rfc822(secs: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = secs / 86_400;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm) for the proleptic
    // Gregorian calendar, with day 0 = 1970-01-01.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{}, {day:02} {} {year} {:02}:{:02}:{:02} +0000",
        DAYS[(days % 7) as usize],
        MONTHS[(month - 1) as usize],
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::rfc822;

    #[test]
    fn rss_dates_are_rfc822_in_utc() {
        assert_eq!(rfc822(0), "Thu, 01 Jan 1970 00:00:00 +0000");
        // 2026-10-05T12:00:00Z, the date in docs/feed-format.md.
        assert_eq!(rfc822(1_791_201_600), "Mon, 05 Oct 2026 12:00:00 +0000");
        // 2000-02-29T23:59:59Z, a leap day.
        assert_eq!(rfc822(951_868_799), "Tue, 29 Feb 2000 23:59:59 +0000");
    }
}

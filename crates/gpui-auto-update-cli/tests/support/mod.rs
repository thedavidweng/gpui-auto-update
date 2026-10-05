//! Fixtures shared by the Sparkle packaging tests: a fake Sparkle
//! distribution laid out like the official archive, a minimal `.app` bundle,
//! and a loopback HTTP server. Nothing here touches the internet.

#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gpui-auto-update"))
        .args(args)
        .output()
        .expect("failed to run gpui-auto-update")
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Serves `body` to every request on a loopback port and returns its URL.
pub fn serve(body: Vec<u8>, path: &str) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let _ = request.respond(tiny_http::Response::from_data(body.clone()));
        }
    });
    format!("http://127.0.0.1:{port}/{path}")
}

pub fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[cfg(unix)]
pub fn symlink(target: &str, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

pub const SPARKLE_LICENSE: &str =
    "Copyright (c) 2006-2013 Andy Matuschak.\nPermission is hereby granted...\n";

/// `key => value` pairs rendered as an XML property list. Values are raw
/// plist XML fragments such as `<string>x</string>` or `<true/>`.
pub fn plist_xml(entries: &[(&str, String)]) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n",
    );
    for (k, v) in entries {
        s.push_str(&format!("  <key>{k}</key>\n  {v}\n"));
    }
    s.push_str("</dict>\n</plist>\n");
    s
}

pub fn string(v: &str) -> String {
    format!("<string>{v}</string>")
}

/// Compiled fixture binaries, built once per test process (macOS only).
#[cfg(target_os = "macos")]
mod native {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::OnceLock;

    fn cc(args: &[&str], source: &str) {
        let mut child = Command::new("cc")
            .args(["-x", "c", "-", "-mmacosx-version-min=12.0"])
            .args(args)
            .stdin(Stdio::piped())
            .spawn()
            .expect("cc is required for macOS fixture binaries");
        use std::io::Write as _;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(source.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success(), "cc {args:?} failed");
    }

    pub struct Binaries {
        _dir: tempfile::TempDir,
        pub sparkle_dylib: PathBuf,
        pub executable: PathBuf,
    }

    pub fn binaries() -> &'static Binaries {
        static BINARIES: OnceLock<Binaries> = OnceLock::new();
        BINARIES.get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let sparkle_dylib = dir.path().join("Sparkle");
            cc(
                &[
                    "-dynamiclib",
                    "-install_name",
                    "@rpath/Sparkle.framework/Versions/B/Sparkle",
                    "-o",
                    sparkle_dylib.to_str().unwrap(),
                ],
                "int sparkle_fixture(void) { return 0; }\n",
            );
            let executable = dir.path().join("tool");
            cc(
                &["-o", executable.to_str().unwrap()],
                "int main(void) { return 0; }\n",
            );
            Binaries {
                _dir: dir,
                sparkle_dylib,
                executable,
            }
        })
    }

    /// Links an app executable against the framework found in `framework_dir`.
    pub fn app_executable(out: &Path, framework_dir: &Path, rpath: Option<&str>) {
        let mut args = vec![
            "-o".to_owned(),
            out.to_str().unwrap().to_owned(),
            "-F".to_owned(),
            framework_dir.to_str().unwrap().to_owned(),
            "-framework".to_owned(),
            "Sparkle".to_owned(),
        ];
        if let Some(rpath) = rpath {
            args.push(format!("-Wl,-rpath,{rpath}"));
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        cc(
            &args,
            "extern int sparkle_fixture(void);\nint main(void) { return sparkle_fixture(); }\n",
        );
    }
}

#[cfg(target_os = "macos")]
pub use native::binaries;

/// An app whose executable links the real `Sparkle.framework` in
/// `framework_dir` and exits 0 only if Sparkle's updater class loaded.
#[cfg(target_os = "macos")]
pub fn app_linking_real_sparkle(root: &Path, framework_dir: &Path) -> PathBuf {
    use std::io::Write as _;
    use std::process::Stdio;
    let app = root.join("Fixture.app");
    write(&app.join("Contents/Info.plist"), &plist_xml(&valid_info()));
    let exe = app.join("Contents/MacOS/Fixture");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    let mut child = Command::new("cc")
        .args([
            "-fobjc-arc",
            "-x",
            "objective-c",
            "-",
            "-mmacosx-version-min=12.0",
            "-o",
        ])
        .arg(&exe)
        .args(["-framework", "Foundation", "-F"])
        .arg(framework_dir)
        .args([
            "-framework",
            "Sparkle",
            "-Wl,-rpath,@executable_path/../Frameworks",
        ])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            b"#import <Foundation/Foundation.h>\nint main(void) { return NSClassFromString(@\"SPUStandardUpdaterController\") ? 0 : 3; }\n",
        )
        .unwrap();
    assert!(child.wait().unwrap().success());
    app
}

fn executable(path: &Path, dylib: bool) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    #[cfg(target_os = "macos")]
    {
        let b = binaries();
        let src = if dylib {
            &b.sparkle_dylib
        } else {
            &b.executable
        };
        fs::copy(src, path).unwrap();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = dylib;
        fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

fn helper_bundle(contents: &Path, id: &str, exe: &str, package_type: &str) {
    write(
        &contents.join("Info.plist"),
        &plist_xml(&[
            ("CFBundleIdentifier", string(id)),
            ("CFBundleExecutable", string(exe)),
            ("CFBundlePackageType", string(package_type)),
        ]),
    );
    executable(&contents.join("MacOS").join(exe), false);
}

/// Creates `<root>/Sparkle-<version>/` laid out like the official archive and
/// returns its path.
#[cfg(unix)]
pub fn fixture_distribution(root: &Path, version: &str) -> PathBuf {
    let dist = root.join(format!("Sparkle-{version}"));
    let fw = dist.join("Sparkle.framework");
    let b = fw.join("Versions/B");
    executable(&b.join("Sparkle"), true);
    executable(&b.join("Autoupdate"), false);
    write(
        &b.join("Resources/Info.plist"),
        &plist_xml(&[
            ("CFBundleIdentifier", string("org.sparkle-project.Sparkle")),
            ("CFBundleExecutable", string("Sparkle")),
            ("CFBundlePackageType", string("FMWK")),
            ("CFBundleShortVersionString", string(version)),
            ("CFBundleVersion", string("2064")),
            ("LSMinimumSystemVersion", string("12.0")),
        ]),
    );
    write(&b.join("Headers/SPUUpdater.h"), "// fixture\n");
    helper_bundle(
        &b.join("Updater.app/Contents"),
        "org.sparkle-project.Sparkle.Updater",
        "Updater",
        "APPL",
    );
    helper_bundle(
        &b.join("XPCServices/Installer.xpc/Contents"),
        "org.sparkle-project.InstallerLauncher",
        "Installer",
        "XPC!",
    );
    helper_bundle(
        &b.join("XPCServices/Downloader.xpc/Contents"),
        "org.sparkle-project.DownloaderService",
        "Downloader",
        "XPC!",
    );
    symlink("B", &fw.join("Versions/Current"));
    for name in [
        "Autoupdate",
        "Headers",
        "Resources",
        "Sparkle",
        "Updater.app",
        "XPCServices",
    ] {
        symlink(&format!("Versions/Current/{name}"), &fw.join(name));
    }
    write(&dist.join("LICENSE"), SPARKLE_LICENSE);
    write(&dist.join("CHANGELOG"), "fixture\n");
    write(&dist.join("bin/sign_update"), "fixture\n");
    dist
}

/// A `.tar.xz` of the contents of `dir`, with symlinks preserved.
pub fn tar_xz(dir: &Path) -> Vec<u8> {
    let encoder = liblzma::write::XzEncoder::new(Vec::new(), 6);
    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);
    builder.append_dir_all(".", dir).unwrap();
    let mut encoder = builder.into_inner().unwrap();
    encoder.flush().unwrap();
    encoder.finish().unwrap()
}

/// A `.tar.xz` holding one hand-crafted entry, for hostile-archive tests.
pub fn tar_xz_with_raw_entry(name: &[u8], kind: tar::EntryType, link: Option<&str>) -> Vec<u8> {
    let mut header = tar::Header::new_old();
    header.as_old_mut().name[..name.len()].copy_from_slice(name);
    header.set_entry_type(kind);
    header.set_mode(0o644);
    header.set_size(0);
    if let Some(link) = link {
        header.as_old_mut().linkname[..link.len()].copy_from_slice(link.as_bytes());
    }
    header.set_cksum();
    let encoder = liblzma::write::XzEncoder::new(Vec::new(), 6);
    let mut builder = tar::Builder::new(encoder);
    builder.append(&header, std::io::empty()).unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}

pub const PUBLIC_ED_KEY: &str = "pfIShU4dEXqPd5ObYNfDBiQWcXozk7estwzTnF9BamQ=";

/// Info.plist entries that satisfy every Sparkle metadata check.
pub fn valid_info() -> Vec<(&'static str, String)> {
    vec![
        ("CFBundleIdentifier", string("com.example.Fixture")),
        ("CFBundleExecutable", string("Fixture")),
        ("CFBundlePackageType", string("APPL")),
        ("CFBundleShortVersionString", string("1.2.3")),
        ("CFBundleVersion", string("42")),
        ("LSMinimumSystemVersion", string("12.0")),
        (
            "SUFeedURL",
            string("https://updates.example.com/appcast.xml"),
        ),
        ("SUPublicEDKey", string(PUBLIC_ED_KEY)),
    ]
}

pub struct AppSpec<'a> {
    pub info: Vec<(&'a str, String)>,
    /// Link the executable against `Sparkle.framework` (macOS only).
    pub link_sparkle: bool,
    pub rpath: Option<&'a str>,
}

impl Default for AppSpec<'_> {
    fn default() -> Self {
        Self {
            info: valid_info(),
            link_sparkle: true,
            rpath: Some("@executable_path/../Frameworks"),
        }
    }
}

/// Creates `<root>/Fixture.app`. `dist` is the fixture distribution the
/// executable links against.
pub fn fixture_app(root: &Path, dist: &Path, spec: &AppSpec<'_>) -> PathBuf {
    let app = root.join("Fixture.app");
    let contents = app.join("Contents");
    write(&contents.join("Info.plist"), &plist_xml(&spec.info));
    let exe = contents.join("MacOS/Fixture");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    #[cfg(target_os = "macos")]
    if spec.link_sparkle {
        native::app_executable(&exe, dist, spec.rpath);
    } else {
        executable(&exe, false);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = dist;
        executable(&exe, false);
    }
    app
}

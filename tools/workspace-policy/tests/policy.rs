//! Layering and publication rules for the workspace (see ADR 0001).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// Crates that must build without GPUI so that GPUI API churn stays inside the
/// facade and UI crates.
const FRAMEWORK_INDEPENDENT: &[&str] = &[
    "gpui-auto-update-core",
    "gpui-auto-update-macos",
    "gpui-auto-update-windows",
    "gpui-auto-update-linux",
    "gpui-auto-update-cli",
];

const PUBLISHED: &[&str] = &[
    "gpui-auto-update-core",
    "gpui-auto-update-macos",
    "gpui-auto-update-windows",
    "gpui-auto-update-linux",
    "gpui-auto-update",
    "gpui-auto-update-ui",
    "gpui-auto-update-cli",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("tools/workspace-policy is two levels below the workspace root")
        .to_path_buf()
}

fn cargo() -> Command {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.current_dir(workspace_root());
    cmd
}

fn run(mut cmd: Command) -> String {
    let output = cmd.output().expect("failed to spawn cargo");
    assert!(
        output.status.success(),
        "{cmd:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("cargo output is UTF-8")
}

fn workspace_packages() -> Vec<Value> {
    let mut cmd = cargo();
    cmd.args(["metadata", "--format-version", "1", "--no-deps", "--locked"]);
    let metadata: Value = serde_json::from_str(&run(cmd)).expect("cargo metadata is JSON");
    metadata["packages"]
        .as_array()
        .expect("packages array")
        .clone()
}

fn package<'a>(packages: &'a [Value], name: &str) -> &'a Value {
    packages
        .iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("workspace has no package named {name}"))
}

/// Names of every crate reachable through normal and build dependencies on
/// any target, which is what a downstream consumer would compile.
fn dependency_closure(package: &str) -> Vec<String> {
    let mut cmd = cargo();
    cmd.args([
        "tree",
        "--locked",
        "-p",
        package,
        "--target",
        "all",
        "-e",
        "normal,build",
        "--prefix",
        "none",
        "--format",
        "{p}",
    ]);
    run(cmd)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn framework_independent_crates_do_not_depend_on_gpui() {
    for krate in FRAMEWORK_INDEPENDENT {
        let offenders: Vec<_> = dependency_closure(krate)
            .into_iter()
            .filter(|dep| dep == "gpui" || dep.starts_with("gpui_") || dep == "gpui-macros")
            .collect();
        assert!(
            offenders.is_empty(),
            "{krate} must not depend on GPUI, but pulls in {offenders:?}"
        );
    }
}

#[test]
fn facade_depends_on_official_gpui_and_core() {
    let deps = dependency_closure("gpui-auto-update");
    assert!(
        deps.iter().any(|d| d == "gpui"),
        "facade must use official gpui"
    );
    assert!(
        deps.iter().any(|d| d == "gpui-auto-update-core"),
        "facade must be built on the core crate"
    );
    assert!(
        !deps.iter().any(|d| d == "gpui-pre"),
        "unofficial gpui forks must not be used"
    );
}

#[test]
fn published_crates_declare_crates_io_metadata() {
    let packages = workspace_packages();
    for name in PUBLISHED {
        let p = package(&packages, name);
        assert_eq!(p["license"], "MIT OR Apache-2.0", "{name}: license");
        for field in [
            "description",
            "repository",
            "homepage",
            "documentation",
            "readme",
        ] {
            assert!(
                p[field].as_str().is_some_and(|v| !v.is_empty()),
                "{name}: missing `{field}`"
            );
        }
        for field in ["keywords", "categories"] {
            assert!(
                p[field].as_array().is_some_and(|v| !v.is_empty()),
                "{name}: missing `{field}`"
            );
        }
        let manifest = PathBuf::from(p["manifest_path"].as_str().unwrap());
        let readme = manifest
            .parent()
            .unwrap()
            .join(p["readme"].as_str().unwrap());
        assert!(readme.is_file(), "{name}: readme {readme:?} does not exist");
        assert_ne!(
            p["publish"],
            serde_json::json!([]),
            "{name}: must be publishable"
        );
    }
}

#[test]
fn non_library_workspace_members_are_not_published() {
    let packages = workspace_packages();
    for p in &packages {
        let name = p["name"].as_str().unwrap();
        if !PUBLISHED.contains(&name) {
            assert_eq!(
                p["publish"],
                serde_json::json!([]),
                "{name} is not a published crate and must set `publish = false`"
            );
        }
    }
}

#[test]
fn repository_ships_dual_license_texts() {
    let root = workspace_root();
    for file in ["LICENSE-MIT", "LICENSE-APACHE"] {
        assert!(root.join(file).is_file(), "missing {file}");
    }
}

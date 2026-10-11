//! Common release-workflow mistakes in GitHub Actions workflows.
//!
//! The checks are line-based on purpose: they look for patterns that are
//! wrong wherever they appear, without needing a YAML model of each job.

use std::path::{Path, PathBuf};

use super::Report;
use crate::project::Project;

const AREA: &str = "ci";

/// Workflow directories that belong to the project: the package's own and
/// those of its ancestors up to the repository root. Without a repository
/// root only the package directory is searched, so unrelated workflows
/// elsewhere on the machine are never read.
fn workflow_dirs(root: &Path) -> Vec<PathBuf> {
    let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    let mut dirs = Vec::new();
    for dir in root.ancestors() {
        dirs.push(dir.join(".github/workflows"));
        if dir.join(".git").exists() {
            return dirs;
        }
    }
    vec![root.join(".github/workflows")]
}

pub(super) fn check(project: &Project, report: &mut Report) {
    let mut files: Vec<PathBuf> = workflow_dirs(&project.root)
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "yml" || e == "yaml"))
        .collect();
    files.sort();
    if files.is_empty() {
        report.note(AREA, "no GitHub Actions workflows found");
        return;
    }

    let mut publishes = false;
    let mut verifies = false;
    let before = report.findings.len();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let name = display_name(file);
        for (index, line) in text.lines().enumerate() {
            let at = format!("{name}:{}", index + 1);
            let tool = line.contains("gpui-auto-update ");
            publishes |= tool && line.contains(" feed ");
            verifies |= tool && line.contains(" verify ");
            if tool && line.contains("${{ secrets.") {
                report.error(
                    AREA,
                    format!(
                        "{at}: a secret is expanded into a gpui-auto-update command line, where it can leak into logs and process lists; \
                         put it in the step's `env:` and pass --key-env <VAR>"
                    ),
                );
            }
            if line.contains("--allow-test-key") {
                report.error(
                    AREA,
                    format!("{at}: --allow-test-key accepts the publicly known test key; release workflows must use the real key"),
                );
            }
            if tool && line.contains("--allow-http") {
                report.warn(
                    AREA,
                    format!("{at}: --allow-http permits unencrypted URLs; use it only for local test servers"),
                );
            }
        }
    }
    if publishes && !verifies {
        report.warn(
            AREA,
            "the workflows publish feeds with `gpui-auto-update feed` but never run `gpui-auto-update verify` on the result",
        );
    }
    if report.findings.len() == before {
        report.ok(AREA, format!("{} workflow file(s) checked", files.len()));
    }
}

fn display_name(file: &Path) -> String {
    let parts: Vec<_> = file.components().rev().take(3).collect();
    parts
        .iter()
        .rev()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

//! Safety invariants of the reusable release workflow templates in
//! `release/` and the hosting recipes in `docs/hosting/`.
//!
//! These templates are what consuming applications copy into their own
//! repositories, so their publish-order and secret-handling guarantees are
//! part of the project's public surface: immutable artifacts before mutable
//! feeds, every feed verified before it is published, existing versioned
//! artifacts never silently overwritten, and private keys only ever through
//! secure CI inputs.

use std::path::{Path, PathBuf};

use serde_yaml::Value;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("tools/workspace-policy has a workspace root")
        .to_path_buf()
}

fn workflow(name: &str) -> Value {
    let path = workspace_root().join("release/github-actions").join(name);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    serde_yaml::from_str(&text)
        .unwrap_or_else(|err| panic!("{} must be valid YAML: {err}", path.display()))
}

fn jobs(workflow: &Value) -> &Value {
    workflow.get("jobs").expect("workflow has jobs")
}

fn job<'a>(workflow: &'a Value, name: &str) -> &'a Value {
    jobs(workflow)
        .get(name)
        .unwrap_or_else(|| panic!("workflow has a `{name}` job"))
}

fn needs(workflow: &Value, name: &str) -> Vec<String> {
    match job(workflow, name).get("needs") {
        None => Vec::new(),
        Some(Value::String(one)) => vec![one.clone()],
        Some(Value::Sequence(many)) => many
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .expect("needs entries are strings")
                    .to_owned()
            })
            .collect(),
        Some(other) => panic!("`needs` of `{name}` is a string or list, got {other:?}"),
    }
}

/// Every `run` block of every job, with the step name for diagnostics.
fn run_blocks(workflow: &Value) -> Vec<(String, String)> {
    let mut blocks = Vec::new();
    for (job_name, job) in jobs(workflow).as_mapping().expect("jobs is a mapping") {
        let job_name = job_name.as_str().unwrap_or("<unnamed>").to_owned();
        if let Some(steps) = job.get("steps").and_then(Value::as_sequence) {
            for step in steps {
                if let Some(run) = step.get("run").and_then(Value::as_str) {
                    let step_name = step
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("<unnamed>");
                    blocks.push((format!("{job_name}/{step_name}"), run.to_owned()));
                }
            }
        }
    }
    blocks
}

fn on_section(workflow: &Value) -> &Value {
    // YAML 1.2 parses the `on` key as the string "on".
    workflow.get("on").expect("workflow has an `on` section")
}

#[test]
fn release_template_triggers_on_tags_and_manual_dispatch_only() {
    let release = workflow("release.yml");
    let on = on_section(&release);
    let tags = on
        .get("push")
        .and_then(|push| push.get("tags"))
        .and_then(Value::as_sequence)
        .expect("release.yml triggers on pushed tags");
    assert!(
        tags.iter().any(|tag| tag.as_str() == Some("v*")),
        "release.yml triggers on version tags `v*`, got {tags:?}"
    );
    let dispatch = on
        .get("workflow_dispatch")
        .expect("release.yml supports manual dispatch");
    dispatch
        .get("inputs")
        .and_then(|inputs| inputs.get("version"))
        .expect("manual dispatch takes an explicit `version` input");
    assert!(
        on.get("pull_request").is_none() && on.get("pull_request_target").is_none(),
        "a release workflow must not run on pull requests, where forks could \
         reach its secrets"
    );
}

#[test]
fn release_template_publishes_immutable_artifacts_before_mutable_feeds() {
    let release = workflow("release.yml");
    // The full chain: version -> build -> feeds (generate signed feeds) ->
    // publish-artifacts (immutable, never overwritten) -> verify-published
    // (CLI `verify` against the live artifact URLs) -> publish-feeds
    // (mutable, last) -> github-release (human-facing, optional).
    assert!(needs(&release, "build").contains(&"version".to_owned()));
    assert!(needs(&release, "feeds").contains(&"build".to_owned()));
    assert!(needs(&release, "publish-artifacts").contains(&"feeds".to_owned()));
    assert!(
        needs(&release, "verify-published").contains(&"publish-artifacts".to_owned()),
        "feeds are verified against published artifacts before the feeds go live"
    );
    let feed_needs = needs(&release, "publish-feeds");
    assert!(
        feed_needs.contains(&"publish-artifacts".to_owned())
            && feed_needs.contains(&"verify-published".to_owned()),
        "mutable feeds are published last, only after immutable artifacts are \
         uploaded and verified, got {feed_needs:?}"
    );
    assert!(needs(&release, "github-release").contains(&"publish-feeds".to_owned()));
}

#[test]
fn release_template_refuses_concurrent_and_overwriting_releases() {
    let release = workflow("release.yml");
    let concurrency = release
        .get("concurrency")
        .expect("release.yml serializes releases through a concurrency group");
    assert_eq!(
        concurrency
            .get("cancel-in-progress")
            .and_then(Value::as_bool),
        Some(false),
        "a pending release must never be cancelled by a newer one"
    );
    let publish = serde_yaml::to_string(job(&release, "publish-artifacts")).unwrap();
    assert!(
        publish.contains("immutable"),
        "versioned artifacts are uploaded with immutable cache semantics"
    );
    assert!(
        publish.contains("already published") || publish.contains("refuse"),
        "the template refuses to overwrite an already-published version"
    );
    let feeds = serde_yaml::to_string(job(&release, "publish-feeds")).unwrap();
    assert!(
        feeds.contains("max-age=60") || feeds.contains("no-cache"),
        "mutable feeds are published with a short cache lifetime"
    );
}

#[test]
fn release_templates_keep_secrets_out_of_run_blocks() {
    for name in ["release.yml", "verify-feeds.yml"] {
        let template = workflow(name);
        for (step, run) in run_blocks(&template) {
            assert!(
                !run.contains("${{ secrets."),
                "{name}: step {step} expands a secret inside a run block; \
                 secrets must reach steps only through `env` or action inputs"
            );
        }
    }
}

#[test]
fn verify_feeds_template_is_a_secret_free_reusable_workflow() {
    let verify = workflow("verify-feeds.yml");
    let call = on_section(&verify)
        .get("workflow_call")
        .expect("verify-feeds.yml is a reusable workflow_call");
    let inputs = call.get("inputs").expect("workflow_call declares inputs");
    for required in ["feed-url-prefix", "public-key", "version"] {
        inputs
            .get(required)
            .unwrap_or_else(|| panic!("verify-feeds.yml declares a `{required}` input"));
    }
    assert!(
        call.get("secrets").is_none(),
        "verification needs only the public key, so the reusable workflow \
         takes no secrets"
    );
    let runs_verify = run_blocks(&verify)
        .iter()
        .any(|(_, run)| run.contains("gpui-auto-update verify"));
    assert!(
        runs_verify,
        "verify-feeds.yml runs `gpui-auto-update verify`"
    );
}

#[test]
fn hosting_recipes_cover_every_documented_host() {
    for recipe in [
        "github-releases.md",
        "cloudflare-r2.md",
        "s3-compatible.md",
        "static-hosting.md",
    ] {
        let path = workspace_root().join("docs/hosting").join(recipe);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        assert!(
            text.contains("Cache-Control"),
            "{recipe}: gives concrete cache-lifetime guidance"
        );
        assert!(
            text.contains("immutable"),
            "{recipe}: marks versioned artifacts as immutable"
        );
        assert!(
            text.contains("overwrite"),
            "{recipe}: refuses to overwrite existing versioned artifacts"
        );
    }
}

#[test]
fn release_readme_documents_the_publish_order() {
    let path = workspace_root().join("release/README.md");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    let artifacts = text
        .find("immutable artifacts")
        .expect("release/README.md covers immutable artifacts");
    let feeds = text
        .find("mutable feeds")
        .expect("release/README.md covers mutable feeds");
    assert!(
        artifacts < feeds,
        "release/README.md documents immutable artifacts before mutable feeds"
    );
    for phrase in ["verify", "workflow_dispatch", "tag"] {
        assert!(
            text.to_ascii_lowercase().contains(phrase),
            "release/README.md mentions {phrase}"
        );
    }
}

//! Integration tests for the CLI. Every invocation runs with HOME pointed at
//! a temp sandbox — tests must never touch real provider config.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/.agent-profile")
}

/// A command with HOME (and config-dir env) sandboxed into `home`.
fn cmd(home: &Path) -> Command {
    let mut c = Command::cargo_bin("agent-profile").unwrap();
    c.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME");
    c
}

/// Scaffold a `.agent-profile/` workspace from (relative path, contents) pairs.
fn workspace(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, contents) in files {
        let path = tmp.path().join(".agent-profile").join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    tmp
}

const MINIMAL_PROFILE: &str = "\
apiVersion: agent-profile/v1
name: minimal
description: Minimal profile.
role: minimal
targets: [claude-subagent]
";

// ---------------------------------------------------------------- S1-T1

#[test]
fn help_lists_all_subcommands_from_design() {
    let tmp = TempDir::new().unwrap();
    let assert = cmd(tmp.path()).arg("--help").assert().success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for sub in [
        "list",
        "show",
        "validate",
        "doctor",
        "render",
        "diff",
        "apply",
        "teardown",
        "current",
        "completions",
    ] {
        assert!(out.contains(sub), "--help missing subcommand `{sub}`");
    }
}

#[test]
fn unimplemented_commands_exit_1() {
    let tmp = TempDir::new().unwrap();
    // completions lands in S5.
    cmd(tmp.path())
        .args(["completions", "zsh"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not implemented"));
}

#[test]
fn render_has_no_dry_run_flag() {
    let tmp = TempDir::new().unwrap();
    let assert = cmd(tmp.path())
        .args(["render", "--help"])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        !out.contains("--dry-run"),
        "render must not grow a --dry-run flag"
    );
}

#[test]
fn apply_requires_session_id() {
    let tmp = TempDir::new().unwrap();
    cmd(tmp.path())
        .args(["apply", "--role", "qa", "--target", "claude-teammate"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--session-id"));
}

#[test]
fn teardown_requires_session_id_or_all() {
    let tmp = TempDir::new().unwrap();
    cmd(tmp.path()).arg("teardown").assert().failure();
}

// ---------------------------------------------------------------- S1-T5 / S1-T7

#[test]
fn validate_examples_exits_0() {
    let tmp = TempDir::new().unwrap();
    cmd(tmp.path())
        .args(["validate", "--dir"])
        .arg(examples_dir())
        .assert()
        .code(0)
        .stdout(predicate::str::contains(
            "✓ reviewer: schema OK, 2 capability blocks resolved, no secret literals",
        ));
}

#[test]
fn validate_single_role_from_examples() {
    let tmp = TempDir::new().unwrap();
    cmd(tmp.path())
        .args(["validate", "--role", "implementer", "--dir"])
        .arg(examples_dir())
        .assert()
        .code(0)
        .stdout(predicate::str::contains("1 capability block resolved"));
}

#[test]
fn validate_json_envelope() {
    let tmp = TempDir::new().unwrap();
    let assert = cmd(tmp.path())
        .args(["validate", "--json", "--dir"])
        .arg(examples_dir())
        .assert()
        .code(0);
    let json: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON");
    assert_eq!(json["schemaVersion"], 1);
    assert_eq!(json["ok"], true);
    assert!(json["results"].as_array().unwrap().len() >= 3);
}

#[test]
fn validate_bad_stem_exits_2_with_v003() {
    let tmp = workspace(&[(
        "profiles/wrongname.yaml",
        &MINIMAL_PROFILE.replace("name: minimal", "name: other"),
    )]);
    cmd(tmp.path())
        .args(["validate", "--role", "wrongname", "--dir"])
        .arg(tmp.path().join(".agent-profile"))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("V003"))
        .stdout(predicate::str::contains("profiles/wrongname.yaml"));
}

#[test]
fn validate_missing_block_exits_2_with_v005() {
    let profile = format!("{MINIMAL_PROFILE}include: [does-not-exist]\n");
    let tmp = workspace(&[("profiles/minimal.yaml", &profile)]);
    cmd(tmp.path())
        .args(["validate", "--dir"])
        .arg(tmp.path().join(".agent-profile"))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("V005"))
        .stdout(predicate::str::contains("does-not-exist"));
}

#[test]
fn validate_nested_include_exits_2_with_v006() {
    let profile = format!("{MINIMAL_PROFILE}include: [nested]\n");
    let tmp = workspace(&[
        ("profiles/minimal.yaml", &profile),
        (
            "capabilities/nested.yaml",
            "apiVersion: agent-profile/v1\nkind: capability\nname: nested\ninclude: [other]\n",
        ),
    ]);
    cmd(tmp.path())
        .args(["validate", "--role", "minimal", "--dir"])
        .arg(tmp.path().join(".agent-profile"))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("V006"))
        .stdout(predicate::str::contains("capabilities/nested.yaml"));
}

#[test]
fn validate_secret_literal_exits_2() {
    let profile = format!(
        "{MINIMAL_PROFILE}mcpServers:\n  gh:\n    command: github-mcp\n    env:\n      GITHUB_TOKEN: ghp_1234567890abcdef\n"
    );
    let tmp = workspace(&[("profiles/minimal.yaml", &profile)]);
    cmd(tmp.path())
        .args(["validate", "--dir"])
        .arg(tmp.path().join(".agent-profile"))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("V012"))
        .stdout(predicate::str::contains("mcpServers.gh.env.GITHUB_TOKEN"));
}

#[test]
fn validate_allow_literal_silences_finding() {
    let profile = format!(
        "{MINIMAL_PROFILE}mcpServers:\n  gh:\n    command: github-mcp\n    env:\n      GITHUB_TOKEN: not-a-secret # agent-profile: allow-literal\n"
    );
    let tmp = workspace(&[("profiles/minimal.yaml", &profile)]);
    cmd(tmp.path())
        .args(["validate", "--dir"])
        .arg(tmp.path().join(".agent-profile"))
        .assert()
        .code(0);
}

#[test]
fn validate_unknown_key_exits_2_naming_file_and_key() {
    let profile = format!("{MINIMAL_PROFILE}permissions: full\n");
    let tmp = workspace(&[("profiles/minimal.yaml", &profile)]);
    cmd(tmp.path())
        .args(["validate", "--dir"])
        .arg(tmp.path().join(".agent-profile"))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("V001"))
        .stdout(predicate::str::contains("unknown field `permissions`"))
        .stdout(predicate::str::contains("profiles/minimal.yaml"));
}

#[test]
fn validate_bad_api_version_exits_2_with_v002() {
    let profile = MINIMAL_PROFILE.replace("agent-profile/v1", "agent-profile/v2");
    let tmp = workspace(&[("profiles/minimal.yaml", &profile)]);
    cmd(tmp.path())
        .args(["validate", "--dir"])
        .arg(tmp.path().join(".agent-profile"))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("V002"))
        .stdout(predicate::str::contains("apiVersion"));
}

#[test]
fn validate_extra_capability_key_exits_2_naming_file_and_key() {
    let tmp = workspace(&[(
        "capabilities/bad.yaml",
        "apiVersion: agent-profile/v1\nkind: capability\nname: bad\ndescription: not allowed here\n",
    )]);
    cmd(tmp.path())
        .args(["validate", "--dir"])
        .arg(tmp.path().join(".agent-profile"))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("unknown field `description`"))
        .stdout(predicate::str::contains("capabilities/bad.yaml"));
}

#[test]
fn validate_bad_capability_kind_exits_2_with_v007() {
    let tmp = workspace(&[(
        "capabilities/badkind.yaml",
        "apiVersion: agent-profile/v1\nkind: profile\nname: badkind\n",
    )]);
    cmd(tmp.path())
        .args(["validate", "--dir"])
        .arg(tmp.path().join(".agent-profile"))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("V007"));
}

#[test]
fn validate_unknown_role_exits_2() {
    let tmp = TempDir::new().unwrap();
    cmd(tmp.path())
        .args(["validate", "--role", "ghost", "--dir"])
        .arg(examples_dir())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("ghost"));
}

#[test]
fn missing_workspace_exits_2() {
    let tmp = TempDir::new().unwrap();
    cmd(tmp.path())
        .current_dir(tmp.path())
        .arg("list")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(".agent-profile"));
}

// ---------------------------------------------------------------- S1-T6

#[test]
fn list_json_is_valid_with_schema_version_1() {
    let tmp = TempDir::new().unwrap();
    let assert = cmd(tmp.path())
        .args(["list", "--json", "--dir"])
        .arg(examples_dir())
        .assert()
        .code(0);
    let json: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON");
    assert_eq!(json["schemaVersion"], 1);
    let profiles: Vec<&str> = json["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(profiles, ["implementer", "qa", "reviewer"]);
    let caps: Vec<&str> = json["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        caps,
        [
            "mcp-github-readonly",
            "models-high-reasoning",
            "perms-readonly"
        ]
    );
}

#[test]
fn show_raw_prints_profile_file() {
    let tmp = TempDir::new().unwrap();
    cmd(tmp.path())
        .args(["show", "--role", "qa", "--dir"])
        .arg(examples_dir())
        .assert()
        .code(0)
        .stdout(predicate::str::contains("name: qa"))
        .stdout(predicate::str::contains("permissionMode: plan"));
}

#[test]
fn show_resolved_reviewer_has_provenance_comments() {
    let tmp = TempDir::new().unwrap();
    cmd(tmp.path())
        .args(["show", "--role", "reviewer", "--resolved", "--dir"])
        .arg(examples_dir())
        .assert()
        .code(0)
        .stdout(predicate::str::contains("# from: profiles/reviewer.yaml"))
        .stdout(predicate::str::contains("permissionMode: readonly"))
        .stdout(predicate::str::contains("github-readonly"));
}

#[test]
fn show_resolved_provenance_covers_blocks_and_profile() {
    // Profile overrides one key and inherits others, so the resolved view must
    // attribute keys to both included blocks and the profile itself.
    let tmp = workspace(&[
        (
            "profiles/worker.yaml",
            "apiVersion: agent-profile/v1\nname: worker\ndescription: Worker.\nrole: worker\n\
             targets: [codex-agent]\ninclude: [base-perms, base-model]\nmodel:\n  effort: low\n",
        ),
        (
            "capabilities/base-perms.yaml",
            "apiVersion: agent-profile/v1\nkind: capability\nname: base-perms\n\
             permissionMode: readonly\nskills: [verify]\n",
        ),
        (
            "capabilities/base-model.yaml",
            "apiVersion: agent-profile/v1\nkind: capability\nname: base-model\n\
             model:\n  claude: claude-fable-5\n  effort: high\n",
        ),
    ]);
    let assert = cmd(tmp.path())
        .args(["show", "--role", "worker", "--resolved", "--dir"])
        .arg(tmp.path().join(".agent-profile"))
        .assert()
        .code(0)
        .stdout(predicate::str::contains(
            "permissionMode: readonly    # from: capabilities/base-perms.yaml",
        ))
        .stdout(predicate::str::contains(
            "claude: claude-fable-5    # from: capabilities/base-model.yaml",
        ))
        .stdout(predicate::str::contains(
            "effort: low    # from: profiles/worker.yaml",
        ));

    // Same data via --json: provenance map matches.
    let _ = assert;
    let json_assert = cmd(tmp.path())
        .args(["show", "--role", "worker", "--resolved", "--json", "--dir"])
        .arg(tmp.path().join(".agent-profile"))
        .assert()
        .code(0);
    let json: serde_json::Value = serde_json::from_slice(&json_assert.get_output().stdout).unwrap();
    assert_eq!(json["schemaVersion"], 1);
    assert_eq!(
        json["provenance"]["model.claude"],
        "capabilities/base-model.yaml"
    );
    assert_eq!(json["provenance"]["model.effort"], "profiles/worker.yaml");
    assert_eq!(
        json["provenance"]["permissionMode"],
        "capabilities/base-perms.yaml"
    );
    assert_eq!(json["resolved"]["model"]["effort"], "low");
    assert_eq!(json["resolved"]["skills"][0], "verify");
}

#[test]
fn show_unknown_role_exits_2() {
    let tmp = TempDir::new().unwrap();
    cmd(tmp.path())
        .args(["show", "--role", "ghost", "--dir"])
        .arg(examples_dir())
        .assert()
        .code(2);
}

// ---------------------------------------------------------------- S2 render

/// A self-contained codex-targeted workspace used by render/diff tests.
const WORKER_PROFILE: &str = "\
apiVersion: agent-profile/v1
name: worker
description: Codex worker.
role: worker
targets: [codex-agent]
model:
  codex: gpt-5.5
  effort: medium
permissionMode: acceptEdits
tools:
  allow: [\"Bash(cargo test:*)\", Read]
  deny: [\"Bash(git push:*)\"]
mcpServers:
  github:
    command: github-mcp
    env:
      GITHUB_TOKEN: ${env:GITHUB_PAT_RW}
context: [fragments/worker-notes.md]
";

fn worker_workspace() -> TempDir {
    workspace(&[
        ("profiles/worker.yaml", WORKER_PROFILE),
        ("fragments/worker-notes.md", "Be a good worker.\n"),
    ])
}

/// Sorted (relative-path, contents) snapshot of a directory tree.
fn snapshot(root: &Path) -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().display().to_string();
                out.push((rel, fs::read_to_string(&path).unwrap_or_default()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

#[test]
fn render_implementer_codex_matches_design_walkthrough() {
    let home = TempDir::new().unwrap();
    let assert = cmd(home.path())
        .args([
            "render",
            "--role",
            "implementer",
            "--target",
            "codex-agent",
            "--dir",
        ])
        .arg(examples_dir())
        .assert()
        .code(0);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    // §4.6 shape: file list, ops, key names.
    assert!(
        out.starts_with("PLAN (dry-run — nothing written):"),
        "{out}"
    );
    assert!(out.contains("create"), "{out}");
    assert!(out.contains(".codex/agents/implementer.toml"), "{out}");
    assert!(out.contains("merge-keys"), "{out}");
    assert!(out.contains("~/.codex/config.toml"), "{out}");
    assert!(out.contains("+ [mcp_servers.github]"), "{out}");
    assert!(out.contains("key-level merge via toml_edit"), "{out}");
    assert!(out.contains("append"), "{out}");
    assert!(out.contains("AGENTS.md"), "{out}");
    assert!(
        out.contains("marked block (3 lines, @include stub)"),
        "{out}"
    );
    // Inexpressible fields are surfaced, never silently dropped.
    assert!(out.contains("skipped tools.allow: `Read`"), "{out}");
    assert!(out.contains("skipped skills:"), "{out}");
    // Dry-run footer from §4.1.
    assert!(
        out.contains(
            "Run `agent-profile render … --out <dir>` to inspect files, or `apply` to write."
        ),
        "{out}"
    );
}

#[test]
fn render_rejects_target_not_in_profile_targets() {
    let home = TempDir::new().unwrap();
    // qa targets only [claude-subagent, claude-teammate].
    cmd(home.path())
        .args(["render", "--role", "qa", "--target", "codex-agent", "--dir"])
        .arg(examples_dir())
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "does not list target `codex-agent`",
        ));
}

#[test]
fn render_unknown_target_exits_2() {
    let home = TempDir::new().unwrap();
    cmd(home.path())
        .args(["render", "--role", "qa", "--target", "cursor", "--dir"])
        .arg(examples_dir())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown target `cursor`"));
}

#[test]
fn render_json_matches_design_contract() {
    let home = TempDir::new().unwrap();
    let assert = cmd(home.path())
        .args([
            "render",
            "--json",
            "--role",
            "implementer",
            "--target",
            "codex-agent",
            "--dir",
        ])
        .arg(examples_dir())
        .assert()
        .code(0);
    let json: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON");
    assert_eq!(json["schemaVersion"], 1);
    assert_eq!(json["role"], "implementer");
    assert_eq!(json["target"], "codex-agent");
    let actions = json["actions"].as_array().unwrap();
    assert_eq!(actions[0]["op"], "create");
    assert_eq!(actions[0]["path"], ".codex/agents/implementer.toml");
    assert!(
        actions[0]["content"]
            .as_str()
            .unwrap()
            .contains("name = \"implementer\"")
    );
    assert_eq!(actions[1]["op"], "merge-keys");
    assert_eq!(actions[1]["path"], "~/.codex/config.toml");
    assert_eq!(actions[1]["keys"][0], "mcp_servers.github");
    assert_eq!(actions[2]["op"], "append-block");
    assert_eq!(actions[2]["path"], "AGENTS.md");
    assert!(
        json["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["field"] == "skills")
    );
}

#[test]
fn render_out_writes_sandbox_dir_and_touches_nothing_else() {
    let home = TempDir::new().unwrap();
    let ws = worker_workspace();
    let out = TempDir::new().unwrap();

    let ws_before = snapshot(ws.path());
    let home_before = snapshot(home.path());

    cmd(home.path())
        .args([
            "render",
            "--role",
            "worker",
            "--target",
            "codex-agent",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .arg("--out")
        .arg(out.path())
        .assert()
        .code(0)
        .stdout(predicate::str::contains("provider files untouched"));

    // Workspace and sandbox HOME are byte-identical — only --out was written.
    assert_eq!(snapshot(ws.path()), ws_before);
    assert_eq!(snapshot(home.path()), home_before);

    let rendered = snapshot(out.path());
    let paths: Vec<&str> = rendered.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        [
            ".codex/agents/worker.toml",
            ".codex/config.toml",
            "AGENTS.md"
        ]
    );
    let agent = &rendered[0].1;
    assert!(agent.starts_with("# generated by agent-profile v"));
    assert!(agent.contains("name = \"worker\""));
    let config_fragment = &rendered[1].1;
    assert!(config_fragment.contains("[mcp_servers.github]"));
    assert!(config_fragment.contains("GITHUB_TOKEN = \"${env:GITHUB_PAT_RW}\""));
    let agents_md = &rendered[2].1;
    assert!(agents_md.contains("<!-- agent-profile:begin role=worker -->"));
    assert!(agents_md.contains("@.agent-profile/fragments/worker-notes.md"));
}

// ---------------------------------------------------------------- S2 diff

#[test]
fn diff_against_empty_repo_exits_3_showing_creates() {
    let home = TempDir::new().unwrap();
    let ws = worker_workspace();
    let assert = cmd(home.path())
        .args([
            "diff",
            "--role",
            "worker",
            "--target",
            "codex-agent",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(3);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("--- .codex/agents/worker.toml"), "{out}");
    assert!(out.contains("+++ rendered"), "{out}");
    assert!(out.contains("+name = \"worker\""), "{out}");
    assert!(out.contains("--- ~/.codex/config.toml"), "{out}");
    assert!(out.contains("+[mcp_servers.github]"), "{out}");
    assert!(out.contains("--- AGENTS.md"), "{out}");
}

#[test]
fn diff_after_placing_identical_rendered_output_exits_0() {
    let home = TempDir::new().unwrap();
    let ws = worker_workspace();
    let out = TempDir::new().unwrap();

    cmd(home.path())
        .args([
            "render",
            "--role",
            "worker",
            "--target",
            "codex-agent",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .arg("--out")
        .arg(out.path())
        .assert()
        .code(0);

    // Manually place the rendered output where the plan expects it.
    let copy = |from: &str, to: PathBuf| {
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(out.path().join(from), to).unwrap();
    };
    copy(
        ".codex/agents/worker.toml",
        ws.path().join(".codex/agents/worker.toml"),
    );
    copy(".codex/config.toml", home.path().join(".codex/config.toml"));
    copy("AGENTS.md", ws.path().join("AGENTS.md"));

    cmd(home.path())
        .args([
            "diff",
            "--role",
            "worker",
            "--target",
            "codex-agent",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(0)
        .stdout(predicate::str::is_empty());
}

#[test]
fn diff_merge_keys_shows_only_added_toml_keys() {
    let home = TempDir::new().unwrap();
    let ws = worker_workspace();
    let existing = "\
# personal codex config
model = \"gpt-5.5\"

[marketplaces.official]
source_type = \"git\"

[projects.\"/x\"]
trust_level = \"trusted\"
";
    fs::create_dir_all(home.path().join(".codex")).unwrap();
    fs::write(home.path().join(".codex/config.toml"), existing).unwrap();

    let assert = cmd(home.path())
        .args([
            "diff",
            "--role",
            "worker",
            "--target",
            "codex-agent",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(3);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    // §4.6: only the added mcp_servers keys appear — no removals, no churn
    // of the comment/marketplaces/projects content Codex owns.
    assert!(out.contains("+[mcp_servers.github]"), "{out}");
    assert!(out.contains("+command = \"github-mcp\""), "{out}");
    assert!(
        out.contains("+GITHUB_TOKEN = \"${env:GITHUB_PAT_RW}\""),
        "{out}"
    );
    let removals: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with('-') && !l.starts_with("---"))
        .collect();
    assert!(removals.is_empty(), "no removals expected: {removals:?}");
}

#[test]
fn diff_json_lists_diffs_per_path() {
    let home = TempDir::new().unwrap();
    let ws = worker_workspace();
    let assert = cmd(home.path())
        .args([
            "diff",
            "--json",
            "--role",
            "worker",
            "--target",
            "codex-agent",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(3);
    let json: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON");
    assert_eq!(json["schemaVersion"], 1);
    let diffs = json["diffs"].as_array().unwrap();
    let paths: Vec<&str> = diffs.iter().map(|d| d["path"].as_str().unwrap()).collect();
    assert_eq!(
        paths,
        [
            ".codex/agents/worker.toml",
            "~/.codex/config.toml",
            "AGENTS.md"
        ]
    );
    assert!(
        diffs[1]["diff"]
            .as_str()
            .unwrap()
            .contains("+[mcp_servers.github]")
    );
}

#[test]
fn diff_mcp_key_collision_is_drift_exit_3() {
    let home = TempDir::new().unwrap();
    let ws = worker_workspace();
    fs::create_dir_all(home.path().join(".codex")).unwrap();
    fs::write(
        home.path().join(".codex/config.toml"),
        "[mcp_servers.github]\ncommand = \"someone-elses-mcp\"\n",
    )
    .unwrap();
    cmd(home.path())
        .args([
            "diff",
            "--role",
            "worker",
            "--target",
            "codex-agent",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "merge collision at `mcp_servers.github",
        ))
        .stderr(predicate::str::contains("not created by agent-profile"));
}

// ---------------------------------------------------------------- S3 claude render

/// A teammate-targeted workspace used by claude render/diff tests.
const MATE_PROFILE: &str = "\
apiVersion: agent-profile/v1
name: mate
description: Teammate worker.
role: mate
targets: [claude-teammate, claude-subagent]
model:
  claude: claude-fable-5
permissionMode: acceptEdits
tools:
  allow: [Read, \"Bash(cargo test:*)\"]
  deny: [\"Bash(git push:*)\"]
mcpServers:
  github:
    command: github-mcp
    env:
      GITHUB_TOKEN: ${env:GITHUB_PAT_RW}
skills: [verify]
context: [fragments/mate-notes.md]
";

fn mate_workspace() -> TempDir {
    workspace(&[
        ("profiles/mate.yaml", MATE_PROFILE),
        ("fragments/mate-notes.md", "Be a good mate.\n"),
    ])
}

/// §4.1 walkthrough shape for the subagent target.
#[test]
fn render_reviewer_subagent_matches_4_1_walkthrough() {
    let home = TempDir::new().unwrap();
    let assert = cmd(home.path())
        .args([
            "render",
            "--role",
            "reviewer",
            "--target",
            "claude-subagent",
            "--dir",
        ])
        .arg(examples_dir())
        .assert()
        .code(0);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        out.starts_with("PLAN (dry-run — nothing written):"),
        "{out}"
    );
    assert!(out.contains("create"), "{out}");
    assert!(out.contains(".claude/agents/reviewer.md"), "{out}");
    assert!(
        out.contains("skipped mcpServers: claude-subagent cannot express per-subagent MCP servers"),
        "{out}"
    );
    assert!(
        out.contains("→ use target claude-teammate, or add servers to .mcp.json yourself"),
        "{out}"
    );
    assert!(
        out.contains(
            "Run `agent-profile render … --out <dir>` to inspect files, or `apply` to write."
        ),
        "{out}"
    );
}

#[test]
fn render_reviewer_teammate_json_matches_contract() {
    let home = TempDir::new().unwrap();
    let assert = cmd(home.path())
        .args([
            "render",
            "--json",
            "--role",
            "reviewer",
            "--target",
            "claude-teammate",
            "--dir",
        ])
        .arg(examples_dir())
        .assert()
        .code(0);
    let json: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON");
    assert_eq!(json["schemaVersion"], 1);
    assert_eq!(json["target"], "claude-teammate");
    let actions = json["actions"].as_array().unwrap();
    let summary: Vec<(&str, &str)> = actions
        .iter()
        .map(|a| (a["op"].as_str().unwrap(), a["path"].as_str().unwrap()))
        .collect();
    // Exactly the §3.4 state-example action kinds, in surface order.
    assert_eq!(
        summary,
        [
            ("create", ".claude/agents/reviewer.md"),
            ("merge-keys", ".mcp.json"),
            ("merge-keys", ".claude/settings.local.json"),
            ("symlink", ".claude/skills/code-review"),
            ("symlink", ".claude/skills/verification-before-completion"),
            ("append-block", "CLAUDE.md"),
        ]
    );
    assert_eq!(actions[1]["keys"][1], "mcpServers.github-readonly");
    assert_eq!(actions[2]["keys"][0], "permissions.allow[+4]");
    assert_eq!(actions[2]["keys"][2], "permissions.defaultMode");
    assert_eq!(actions[3]["linkTarget"], "~/.agents/skills/code-review");
    assert_eq!(actions[5]["marker"], "role=reviewer");
    // The honest §3.2.3 limitation note rides along.
    assert!(
        json["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains("project-wide")),
        "{json}"
    );
}

#[test]
fn render_teammate_human_includes_limitation_note() {
    let home = TempDir::new().unwrap();
    let ws = mate_workspace();
    cmd(home.path())
        .args([
            "render",
            "--role",
            "mate",
            "--target",
            "claude-teammate",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(0)
        .stdout(predicate::str::contains(
            "note: settings.local.json permissions are project-wide, not per-teammate",
        ))
        .stdout(predicate::str::contains("symlink"))
        .stdout(predicate::str::contains("-> ~/.agents/skills/verify"));
}

// ---------------------------------------------------------------- S3 claude diff

#[test]
fn diff_teammate_roundtrip_exits_0_after_placing_rendered_output() {
    let home = TempDir::new().unwrap();
    let ws = mate_workspace();
    let out = TempDir::new().unwrap();

    cmd(home.path())
        .args([
            "render",
            "--role",
            "mate",
            "--target",
            "claude-teammate",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .arg("--out")
        .arg(out.path())
        .assert()
        .code(0);

    // Empty repo first: every action shows as a difference.
    cmd(home.path())
        .args([
            "diff",
            "--role",
            "mate",
            "--target",
            "claude-teammate",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(3);

    // Place the rendered output where the plan expects it.
    for rel in [
        ".claude/agents/mate.md",
        ".mcp.json",
        ".claude/settings.local.json",
        "CLAUDE.md",
    ] {
        let to = ws.path().join(rel);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(out.path().join(rel), to).unwrap();
    }
    let link = ws.path().join(".claude/skills/verify");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("~/.agents/skills/verify", &link).unwrap();

    cmd(home.path())
        .args([
            "diff",
            "--role",
            "mate",
            "--target",
            "claude-teammate",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(0)
        .stdout(predicate::str::is_empty());
}

#[test]
fn diff_mcp_json_preserves_foreign_keys_and_appends_only() {
    let home = TempDir::new().unwrap();
    let ws = mate_workspace();
    // Existing .mcp.json with a server we don't own, pretty-printed.
    fs::write(
        ws.path().join(".mcp.json"),
        "{\n  \"mcpServers\": {\n    \"sketch\": {\n      \"type\": \"http\",\n      \"url\": \"http://localhost:31126/mcp\"\n    }\n  }\n}\n",
    )
    .unwrap();

    let assert = cmd(home.path())
        .args([
            "diff",
            "--role",
            "mate",
            "--target",
            "claude-teammate",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(3);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let mcp_diff: String = out
        .split("--- ")
        .find(|s| s.starts_with(".mcp.json"))
        .expect("diff for .mcp.json")
        .to_string();
    assert!(mcp_diff.contains("+    \"github\""), "{mcp_diff}");
    // The foreign `sketch` server is never removed.
    let removals: Vec<&str> = mcp_diff
        .lines()
        .filter(|l| l.starts_with('-') && !l.starts_with("---"))
        .collect();
    assert!(
        removals.iter().all(|l| !l.contains("sketch")),
        "foreign keys must survive: {removals:?}"
    );
}

#[test]
fn diff_mcp_json_collision_is_drift_exit_3() {
    let home = TempDir::new().unwrap();
    let ws = mate_workspace();
    fs::write(
        ws.path().join(".mcp.json"),
        r#"{ "mcpServers": { "github": { "command": "someone-elses-mcp" } } }"#,
    )
    .unwrap();
    cmd(home.path())
        .args([
            "diff",
            "--role",
            "mate",
            "--target",
            "claude-teammate",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "merge collision at `mcpServers.github.command`",
        ))
        .stderr(predicate::str::contains("not created by agent-profile"));
}

#[test]
fn diff_settings_appends_only_missing_entries() {
    let home = TempDir::new().unwrap();
    let ws = mate_workspace();
    // `Read` already allowed; `defaultMode` matches → only the missing
    // entries appear as additions.
    fs::write(
        ws.path().join(".claude/settings.local.json"),
        "{\n  \"permissions\": {\n    \"allow\": [\n      \"Read\"\n    ],\n    \"defaultMode\": \"acceptEdits\"\n  }\n}\n",
    )
    .ok();
    fs::create_dir_all(ws.path().join(".claude")).unwrap();
    fs::write(
        ws.path().join(".claude/settings.local.json"),
        "{\n  \"permissions\": {\n    \"allow\": [\n      \"Read\"\n    ],\n    \"defaultMode\": \"acceptEdits\"\n  }\n}\n",
    )
    .unwrap();

    let assert = cmd(home.path())
        .args([
            "diff",
            "--role",
            "mate",
            "--target",
            "claude-teammate",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(3);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let settings_diff: String = out
        .split("--- ")
        .find(|s| s.starts_with(".claude/settings.local.json"))
        .expect("diff for settings.local.json")
        .to_string();
    assert!(
        settings_diff.contains("Bash(cargo test:*)"),
        "{settings_diff}"
    );
    assert!(settings_diff.contains("\"deny\""), "{settings_diff}");
    // No *content* is removed — every removed line reappears with at most a
    // trailing-comma change (JSON syntax churn when appending entries).
    let normalized = |l: &str| l[1..].trim().trim_end_matches(',').to_string();
    let additions: Vec<String> = settings_diff
        .lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .map(normalized)
        .collect();
    for removal in settings_diff
        .lines()
        .filter(|l| l.starts_with('-') && !l.starts_with("---"))
    {
        assert!(
            additions.contains(&normalized(removal)),
            "content removed: {removal} not re-added in {settings_diff}"
        );
    }
}

// ---------------------------------------------------------------- S3 doctor

/// Codex-targeted profile that doctors clean apart from two warnings.
const PORTER_PROFILE: &str = "\
apiVersion: agent-profile/v1
name: porter
description: Codex porter.
role: porter
targets: [codex-agent]
model:
  codex: gpt-5.5
tools:
  allow: [\"Bash(git diff:*)\"]
mcpServers:
  github:
    command: github-mcp-definitely-missing
    env:
      GITHUB_TOKEN: ${env:AP_TEST_UNSET_VAR}
";

#[test]
fn doctor_reviewer_teammate_reproduces_4_3_walkthrough() {
    let home = TempDir::new().unwrap();
    // Skills resolve in the sandbox cross-tool store.
    for skill in ["code-review", "verification-before-completion"] {
        let dir = home.path().join(".agents/skills").join(skill);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "# skill\n").unwrap();
    }
    let assert = cmd(home.path())
        .env("PATH", "/usr/bin:/bin")
        .env_remove("GITHUB_PAT_RO")
        .args([
            "doctor",
            "--role",
            "reviewer",
            "--target",
            "claude-teammate",
            "--assume-version",
            "3.2.0",
            "--dir",
        ])
        .arg(examples_dir())
        .assert()
        .code(4);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        out.starts_with("claude --version → 3.2.0 (assumed)\n"),
        "{out}"
    );
    assert!(out.contains("FINDINGS"), "{out}");
    assert!(
        out.contains(
            "error  F012  frontmatter field `permissionMode` not honored by claude 3.2.0 \
             teammate spawn — rendered via settings.local.json defaultMode instead (info)"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "warn   F031  mcp server `github-readonly`: command `github-mcp` not found on PATH"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "warn   F044  env reference ${env:GITHUB_PAT_RO} is unset in current environment"
        ),
        "{out}"
    );
    assert!(
        out.contains("ok     skills: 2/2 resolve in ~/.agents/skills/"),
        "{out}"
    );
}

#[test]
fn doctor_warnings_alone_exit_0() {
    let home = TempDir::new().unwrap();
    let ws = workspace(&[("profiles/porter.yaml", PORTER_PROFILE)]);
    let assert = cmd(home.path())
        .env("PATH", "/usr/bin:/bin")
        .env_remove("AP_TEST_UNSET_VAR")
        .args([
            "doctor",
            "--role",
            "porter",
            "--target",
            "codex-agent",
            "--assume-version",
            "0.30.0",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(0);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("warn   F031"), "{out}");
    assert!(out.contains("warn   F044"), "{out}");
    assert!(!out.contains("error"), "{out}");
}

#[test]
fn doctor_untranslatable_codex_rules_are_errors() {
    let home = TempDir::new().unwrap();
    let profile = PORTER_PROFILE.replace(
        "allow: [\"Bash(git diff:*)\"]",
        "allow: [Read, mcp__github__search]",
    );
    let ws = workspace(&[("profiles/porter.yaml", &profile)]);
    let assert = cmd(home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "doctor",
            "--role",
            "porter",
            "--target",
            "codex-agent",
            "--assume-version",
            "0.30.0",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(4);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    // Each untranslatable rule listed individually (design §5.3.6).
    assert!(out.contains("error  F061  tools.allow: `Read`"), "{out}");
    assert!(
        out.contains("error  F061  tools.allow: `mcp__github__search`"),
        "{out}"
    );
}

#[test]
fn doctor_missing_cli_is_error_finding_not_panic() {
    let home = TempDir::new().unwrap();
    let empty_bin = TempDir::new().unwrap();
    let ws = workspace(&[("profiles/porter.yaml", PORTER_PROFILE)]);
    let assert = cmd(home.path())
        .env("PATH", empty_bin.path())
        .args([
            "doctor",
            "--role",
            "porter",
            "--target",
            "codex-agent",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(4);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.starts_with("codex --version → (unknown)\n"), "{out}");
    assert!(
        out.contains("error  F001  `codex` not found on PATH"),
        "{out}"
    );
}

#[test]
fn doctor_probes_real_version_output_format() {
    let home = TempDir::new().unwrap();
    let bin = TempDir::new().unwrap();
    let fake = bin.path().join("claude");
    fs::write(&fake, "#!/bin/sh\necho \"3.2.0 (Claude Code)\"\n").unwrap();
    #[allow(clippy::permissions_set_readonly_false)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path_var = format!("{}:/usr/bin:/bin", bin.path().display());
    let assert = cmd(home.path())
        .env("PATH", path_var)
        .env_remove("GITHUB_PAT_RO")
        .args([
            "doctor",
            "--role",
            "reviewer",
            "--target",
            "claude-subagent",
            "--dir",
        ])
        .arg(examples_dir())
        .assert()
        .code(4); // reviewer declares fields the subagent cannot express
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.starts_with("claude --version → 3.2.0\n"), "{out}");
    assert!(!out.contains("(assumed)"), "{out}");
}

#[test]
fn doctor_stale_session_is_info_only() {
    let home = TempDir::new().unwrap();
    let ws = workspace(&[
        ("profiles/porter.yaml", PORTER_PROFILE),
        (
            "state.json",
            r#"{ "schemaVersion": 1, "sessions": { "old-run": { "appliedAt": "2020-01-01T00:00:00Z" } } }"#,
        ),
    ]);
    let assert = cmd(home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "doctor",
            "--role",
            "porter",
            "--target",
            "codex-agent",
            "--assume-version",
            "0.30.0",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(0);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("info   F071  session `old-run` applied"),
        "{out}"
    );
    assert!(out.contains("teardown --session-id old-run"), "{out}");
}

#[test]
fn doctor_json_matches_contract() {
    let home = TempDir::new().unwrap();
    let ws = workspace(&[("profiles/porter.yaml", PORTER_PROFILE)]);
    let assert = cmd(home.path())
        .env("PATH", "/usr/bin:/bin")
        .env_remove("AP_TEST_UNSET_VAR")
        .args([
            "doctor",
            "--json",
            "--role",
            "porter",
            "--target",
            "codex-agent",
            "--assume-version",
            "0.30.0",
            "--dir",
        ])
        .arg(ws.path().join(".agent-profile"))
        .assert()
        .code(0);
    let json: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON");
    assert_eq!(json["schemaVersion"], 1);
    assert_eq!(json["role"], "porter");
    assert_eq!(json["target"], "codex-agent");
    assert_eq!(json["cliVersion"], "0.30.0");
    let findings = json["findings"].as_array().unwrap();
    assert!(!findings.is_empty());
    for f in findings {
        assert!(f["level"].is_string(), "{f}");
        let code = f["code"].as_str().unwrap();
        assert!(code.starts_with('F') && code.len() == 4, "{code}");
        assert!(f["message"].is_string(), "{f}");
    }
    // Field is present where a finding is field-scoped.
    assert!(
        findings.iter().any(|f| f["field"]
            .as_str()
            .is_some_and(|s| s.starts_with("mcpServers."))),
        "{json}"
    );
}

#[test]
fn doctor_rejects_target_not_in_profile_targets() {
    let home = TempDir::new().unwrap();
    cmd(home.path())
        .args([
            "doctor",
            "--role",
            "qa",
            "--target",
            "codex-agent",
            "--assume-version",
            "0.30.0",
            "--dir",
        ])
        .arg(examples_dir())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("does not list target"));
}

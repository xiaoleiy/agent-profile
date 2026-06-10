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
    cmd(tmp.path())
        .args(["current"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not implemented"));
    cmd(tmp.path())
        .args(["render", "--role", "qa", "--target", "claude-subagent"])
        .assert()
        .code(1);
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

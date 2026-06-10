//! S4 integration suite: apply / teardown / current against sandbox repos.
//! Every invocation runs with HOME pointed into the sandbox — these tests
//! must never touch the developer's real provider config.

use assert_cmd::Command;
use predicates::prelude::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/.agent-profile")
}

/// Sandbox: `<tmp>/home` (HOME) + `<tmp>/repo` (a git repo with the example
/// workspace copied in and state/backups gitignored).
struct Sandbox {
    _tmp: TempDir,
    repo: PathBuf,
    home: PathBuf,
}

fn sandbox() -> Sandbox {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&home).unwrap();
    copy_tree(&examples_dir(), &repo.join(".agent-profile"));
    fs::write(
        repo.join(".gitignore"),
        ".agent-profile/state.json\n.agent-profile/backups/\n",
    )
    .unwrap();
    git(&repo, &["init", "-q"]);
    Sandbox {
        _tmp: tmp,
        repo,
        home,
    }
}

fn git(repo: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("git available");
    assert!(status.success(), "git {args:?} failed");
}

fn copy_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).unwrap();
        }
    }
}

/// A command sandboxed into the fixture repo: HOME overridden, secret env
/// refs unset (set explicitly per test where needed).
fn cmd(sb: &Sandbox) -> Command {
    let mut c = Command::cargo_bin("agent-profile").unwrap();
    c.current_dir(&sb.repo)
        .env("HOME", &sb.home)
        .env("XDG_CONFIG_HOME", sb.home.join(".config"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("GITHUB_PAT_RO")
        .env_remove("GITHUB_PAT_RW");
    c
}

/// Tree snapshot: relpath → contents (or symlink target / "dir"), excluding
/// `.git`, `state.json`, and `backups/` — the "modulo state/backups" clause.
fn snapshot(root: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    walk(root, root, &mut map);
    map
}

fn walk(root: &Path, dir: &Path, map: &mut BTreeMap<String, String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap().display().to_string();
        if rel == ".git"
            || rel == ".agent-profile/state.json"
            || rel == ".agent-profile/backups"
            || rel.starts_with(".git/")
            || rel.starts_with(".agent-profile/backups/")
        {
            continue;
        }
        let meta = fs::symlink_metadata(&path).unwrap();
        if meta.is_symlink() {
            map.insert(
                rel,
                format!("symlink -> {}", fs::read_link(&path).unwrap().display()),
            );
        } else if meta.is_dir() {
            map.insert(format!("{rel}/"), "dir".into());
            walk(root, &path, map);
        } else {
            map.insert(rel, fs::read_to_string(&path).unwrap_or_default());
        }
    }
}

// ---------------------------------------------------------------- S4-T3 apply

/// §4.2: three differently-profiled sessions in one sandbox repo; `apply
/// --json` matches the §2 contract (sessionId, backupDir, per-action status).
#[test]
fn multi_role_apply_json_contract_and_current() {
    let sb = sandbox();
    for (role, target, session) in [
        ("reviewer", "claude-teammate", "run7-reviewer"),
        ("qa", "claude-teammate", "run7-qa"),
        ("implementer", "codex-agent", "run7-implementer"),
    ] {
        let assert = cmd(&sb)
            .args([
                "apply",
                "--role",
                role,
                "--target",
                target,
                "--session-id",
                session,
                "--json",
            ])
            .assert()
            .code(0);
        let json: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
        assert_eq!(json["schemaVersion"], 1);
        assert_eq!(json["sessionId"], session);
        assert_eq!(json["status"], "applied");
        assert_eq!(
            json["backupDir"],
            format!(".agent-profile/backups/{session}")
        );
        let actions = json["actions"].as_array().unwrap();
        assert!(!actions.is_empty());
        for a in actions {
            assert_eq!(a["status"], "applied", "per-action status, §2 contract");
            assert!(a["op"].is_string() && a["path"].is_string());
        }
    }
    // reviewer teammate plan: create + 2 merges + 2 symlinks + append = 6.
    let state: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(sb.repo.join(".agent-profile/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        state["sessions"]["run7-reviewer"]["actions"]
            .as_array()
            .unwrap()
            .len(),
        6
    );

    // §4.2 `current` output shape.
    let assert = cmd(&sb).arg("current").assert().code(0);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.starts_with("ACTIVE SESSIONS\n"), "{out}");
    assert!(
        out.contains("run7-reviewer")
            && out.contains("reviewer")
            && out.contains("→ claude-teammate")
            && out.contains("applied ")
            && out.contains("(6 actions)"),
        "{out}"
    );
    assert!(
        out.contains("run7-implementer") && out.contains("→ codex-agent "),
        "{out}"
    );

    let assert = cmd(&sb).args(["current", "--json"]).assert().code(0);
    let json: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(json["schemaVersion"], 1);
    assert_eq!(json["sessions"].as_array().unwrap().len(), 3);
}

/// §4.5 drift refusal: message, exit 3, suggested commands — reproduced
/// exactly.
#[test]
fn apply_drift_refusal_matches_walkthrough_4_5() {
    let sb = sandbox();
    let agents = sb.repo.join(".claude/agents");
    fs::create_dir_all(&agents).unwrap();
    fs::write(agents.join("qa.md"), "# hand-written qa agent\n").unwrap();

    let assert = cmd(&sb)
        .args([
            "apply",
            "--role",
            "qa",
            "--target",
            "claude-teammate",
            "--session-id",
            "run8-qa",
        ])
        .assert()
        .code(3);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let expected = "\
✗ drift: .claude/agents/qa.md exists and was not generated by agent-profile
  (no provenance header). Refusing to overwrite.
  → inspect: agent-profile diff --role qa --target claude-teammate
  → override (backs up the file first): apply --force
";
    assert_eq!(out, expected);
    // Refusal mutated nothing and recorded no session.
    assert_eq!(
        fs::read_to_string(agents.join("qa.md")).unwrap(),
        "# hand-written qa agent\n"
    );
    assert!(!sb.repo.join(".agent-profile/state.json").exists());
}

/// `--force` overrides the §4.5 refusal: still backs up, records the
/// override, and teardown restores the foreign original from backup.
#[test]
fn apply_force_backs_up_records_override_and_teardown_restores() {
    let sb = sandbox();
    let agent_md = sb.repo.join(".claude/agents/qa.md");
    fs::create_dir_all(agent_md.parent().unwrap()).unwrap();
    fs::write(&agent_md, "# hand-written qa agent\n").unwrap();

    cmd(&sb)
        .args([
            "apply",
            "--role",
            "qa",
            "--target",
            "claude-teammate",
            "--session-id",
            "run8-qa",
            "--force",
        ])
        .assert()
        .code(0);
    let written = fs::read_to_string(&agent_md).unwrap();
    assert!(written.contains("generated by agent-profile"));
    let state: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(sb.repo.join(".agent-profile/state.json")).unwrap(),
    )
    .unwrap();
    let create = &state["sessions"]["run8-qa"]["actions"][0];
    assert_eq!(create["op"], "create");
    assert_eq!(create["forced"], true, "override recorded in state");
    assert_eq!(
        create["backup"],
        ".agent-profile/backups/run8-qa/.claude/agents/qa.md"
    );

    cmd(&sb)
        .args(["teardown", "--session-id", "run8-qa"])
        .assert()
        .code(0);
    assert_eq!(
        fs::read_to_string(&agent_md).unwrap(),
        "# hand-written qa agent\n",
        "foreign original restored from backup"
    );
}

/// Duplicate session id, and a second active session for the same
/// role+target, are session/state errors (exit 5).
#[test]
fn duplicate_sessions_exit_5() {
    let sb = sandbox();
    let apply = |session: &str| {
        cmd(&sb)
            .args([
                "apply",
                "--role",
                "reviewer",
                "--target",
                "claude-subagent",
                "--session-id",
                session,
            ])
            .assert()
    };
    apply("run9-reviewer").code(0);
    apply("run9-reviewer")
        .code(5)
        .stderr(predicate::str::contains("already active"));
    apply("run9-reviewer-again")
        .code(5)
        .stderr(predicate::str::contains(
            "already applies role `reviewer` to target `claude-subagent`",
        ));
}

/// §5.5 secret gate, both ways: resolved `${env:VAR}` refuses a tracked
/// `.mcp.json` (exit 2) and materializes into a gitignored one.
#[test]
fn secret_to_tracked_file_refused_and_gitignored_file_resolved() {
    let sb = sandbox();
    cmd(&sb)
        .env("GITHUB_PAT_RO", "s3kr3t-value")
        .args([
            "apply",
            "--role",
            "reviewer",
            "--target",
            "claude-teammate",
            "--session-id",
            "sec-1",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not gitignored"))
        .stderr(predicate::str::contains("${env:GITHUB_PAT_RO}"));
    assert!(!sb.repo.join(".mcp.json").exists(), "refusal wrote nothing");

    // Gitignore the file → apply resolves the reference into it.
    fs::write(
        sb.repo.join(".gitignore"),
        ".agent-profile/state.json\n.agent-profile/backups/\n.mcp.json\n",
    )
    .unwrap();
    cmd(&sb)
        .env("GITHUB_PAT_RO", "s3kr3t-value")
        .args([
            "apply",
            "--role",
            "reviewer",
            "--target",
            "claude-teammate",
            "--session-id",
            "sec-2",
        ])
        .assert()
        .code(0);
    let mcp = fs::read_to_string(sb.repo.join(".mcp.json")).unwrap();
    assert!(mcp.contains("s3kr3t-value"), "{mcp}");
    assert!(!mcp.contains("${env:GITHUB_PAT_RO}"), "{mcp}");

    cmd(&sb)
        .args(["teardown", "--session-id", "sec-2"])
        .assert()
        .code(0);
    assert!(!sb.repo.join(".mcp.json").exists(), "created file removed");
}

/// Unset env references stay as references (Claude's own pattern) — no gate.
#[test]
fn unset_env_reference_is_written_verbatim() {
    let sb = sandbox();
    cmd(&sb)
        .args([
            "apply",
            "--role",
            "reviewer",
            "--target",
            "claude-teammate",
            "--session-id",
            "ref-1",
        ])
        .assert()
        .code(0);
    let mcp = fs::read_to_string(sb.repo.join(".mcp.json")).unwrap();
    assert!(mcp.contains("${env:GITHUB_PAT_RO}"), "{mcp}");
}

/// One-time gitignore hint: printed on the first apply where state.json is
/// not ignored, never again.
#[test]
fn gitignore_hint_is_one_time() {
    let sb = sandbox();
    fs::write(sb.repo.join(".gitignore"), "").unwrap(); // nothing ignored
    let assert = cmd(&sb)
        .args([
            "apply",
            "--role",
            "qa",
            "--target",
            "claude-subagent",
            "--session-id",
            "h1",
        ])
        .assert()
        .code(0);
    let err = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        err.contains("hint: add `.agent-profile/state.json`"),
        "{err}"
    );

    let assert = cmd(&sb)
        .args([
            "apply",
            "--role",
            "reviewer",
            "--target",
            "claude-subagent",
            "--session-id",
            "h2",
        ])
        .assert()
        .code(0);
    let err = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(!err.contains("hint:"), "{err}");
}

// ---------------------------------------------------------------- S4-T4 teardown

/// Apply→teardown restores a byte-identical tree (repo AND sandbox home),
/// for each target — the reversibility property the trust story rests on.
#[test]
fn apply_teardown_round_trip_is_byte_identical_per_target() {
    for (role, target) in [
        ("reviewer", "claude-teammate"),
        ("reviewer", "claude-subagent"),
        ("implementer", "codex-agent"),
    ] {
        let sb = sandbox();
        // Pre-existing provider files with foreign content and formatting.
        fs::write(sb.repo.join("CLAUDE.md"), "# Project notes\n").unwrap();
        fs::write(sb.repo.join("AGENTS.md"), "# Agent notes\n").unwrap();
        fs::write(
            sb.repo.join(".mcp.json"),
            "{\n  \"mcpServers\": {\n    \"sketch\": {\n      \"type\": \"http\",\n      \"url\": \"http://localhost:1\"\n    }\n  }\n}\n",
        )
        .unwrap();
        let codex_dir = sb.home.join(".codex");
        fs::create_dir_all(&codex_dir).unwrap();
        fs::write(
            codex_dir.join("config.toml"),
            "# my codex config — hands off\nmodel = \"gpt-5.5\"\n\n[marketplaces.official]\nsource = \"git\"\n",
        )
        .unwrap();

        let before_repo = snapshot(&sb.repo);
        let before_home = snapshot(&sb.home);

        cmd(&sb)
            .args([
                "apply",
                "--role",
                role,
                "--target",
                target,
                "--session-id",
                "rt-1",
            ])
            .assert()
            .code(0);
        assert_ne!(snapshot(&sb.repo), before_repo, "{role}->{target} applied");

        cmd(&sb)
            .args(["teardown", "--session-id", "rt-1"])
            .assert()
            .code(0)
            .stdout(predicate::str::contains(
                "✓ session rt-1 torn down, backups deleted",
            ));

        assert_eq!(
            snapshot(&sb.repo),
            before_repo,
            "{role}->{target}: repo tree restored byte-identically"
        );
        assert_eq!(
            snapshot(&sb.home),
            before_home,
            "{role}->{target}: home tree restored byte-identically"
        );
        assert!(!sb.repo.join(".agent-profile/backups/rt-1").exists());
        let state: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(sb.repo.join(".agent-profile/state.json")).unwrap(),
        )
        .unwrap();
        assert!(state["sessions"].as_object().unwrap().is_empty());
    }
}

/// §4.4 walkthrough: the teardown step lines for a teammate session.
#[test]
fn teardown_output_matches_walkthrough_4_4() {
    let sb = sandbox();
    cmd(&sb)
        .args([
            "apply",
            "--role",
            "reviewer",
            "--target",
            "claude-teammate",
            "--session-id",
            "run7-reviewer",
        ])
        .assert()
        .code(0);
    let assert = cmd(&sb)
        .args(["teardown", "--session-id", "run7-reviewer"])
        .assert()
        .code(0);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for needle in [
        "restore  .mcp.json",
        "(removed keys mcpServers.docs-search, mcpServers.github-readonly)",
        "restore  .claude/settings.local.json",
        "(removed 4 allow rules, 2 deny rules, defaultMode)",
        "delete   .claude/agents/reviewer.md",
        "unlink   .claude/skills/code-review",
        "unlink   .claude/skills/verification-before-completion",
        "remove   CLAUDE.md",
        "block session=run7-reviewer",
        "✓ session run7-reviewer torn down, backups deleted",
    ] {
        assert!(out.contains(needle), "missing {needle:?} in:\n{out}");
    }
}

/// Interleaved edit: a foreign key added to `.mcp.json` between apply and
/// teardown survives the key-level reversal.
#[test]
fn foreign_mcp_key_added_after_apply_survives_teardown() {
    let sb = sandbox();
    cmd(&sb)
        .args([
            "apply",
            "--role",
            "reviewer",
            "--target",
            "claude-teammate",
            "--session-id",
            "il-1",
        ])
        .assert()
        .code(0);

    let mcp_path = sb.repo.join(".mcp.json");
    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&mcp_path).unwrap()).unwrap();
    v["mcpServers"]["foreign"] = serde_json::json!({ "command": "foreign-mcp" });
    fs::write(
        &mcp_path,
        format!("{}\n", serde_json::to_string_pretty(&v).unwrap()),
    )
    .unwrap();

    cmd(&sb)
        .args(["teardown", "--session-id", "il-1"])
        .assert()
        .code(0);
    let after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&mcp_path).unwrap()).unwrap();
    assert_eq!(after["mcpServers"]["foreign"]["command"], "foreign-mcp");
    assert!(
        after["mcpServers"].get("github-readonly").is_none(),
        "our key removed: {after}"
    );
    assert!(after["mcpServers"].get("docs-search").is_none());
}

/// Post-apply drift on an owned file: refuse with the drifted path (exit 3);
/// `--force` falls back and still cleans up.
#[test]
fn teardown_drift_refusal_and_force_fallback() {
    let sb = sandbox();
    cmd(&sb)
        .args([
            "apply",
            "--role",
            "reviewer",
            "--target",
            "claude-subagent",
            "--session-id",
            "dr-1",
        ])
        .assert()
        .code(0);
    let agent_md = sb.repo.join(".claude/agents/reviewer.md");
    fs::write(&agent_md, "tampered after apply\n").unwrap();

    cmd(&sb)
        .args(["teardown", "--session-id", "dr-1"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains(".claude/agents/reviewer.md"))
        .stdout(predicate::str::contains("teardown --force"));
    assert!(agent_md.exists(), "refusal touched nothing");

    cmd(&sb)
        .args(["teardown", "--session-id", "dr-1", "--force"])
        .assert()
        .code(0);
    assert!(!agent_md.exists());
}

/// A drifted *merge* key (someone rewrote a value we wrote) is post-apply
/// drift; `--force` does best-effort key-level reversal.
#[test]
fn teardown_drifted_merge_key_refused_then_forced() {
    let sb = sandbox();
    cmd(&sb)
        .args([
            "apply",
            "--role",
            "reviewer",
            "--target",
            "claude-teammate",
            "--session-id",
            "dm-1",
        ])
        .assert()
        .code(0);
    let mcp_path = sb.repo.join(".mcp.json");
    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&mcp_path).unwrap()).unwrap();
    v["mcpServers"]["github-readonly"]["command"] = serde_json::json!("hijacked");
    fs::write(
        &mcp_path,
        format!("{}\n", serde_json::to_string_pretty(&v).unwrap()),
    )
    .unwrap();

    cmd(&sb)
        .args(["teardown", "--session-id", "dm-1"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains(".mcp.json"));
    cmd(&sb)
        .args(["teardown", "--session-id", "dm-1", "--force"])
        .assert()
        .code(0);
    assert!(!mcp_path.exists(), "we created it; best-effort removal");
}

#[test]
fn unknown_session_and_corrupt_state_exit_5() {
    let sb = sandbox();
    cmd(&sb)
        .args(["teardown", "--session-id", "nope"])
        .assert()
        .code(5)
        .stderr(predicate::str::contains("unknown session `nope`"));

    fs::write(sb.repo.join(".agent-profile/state.json"), "{ torn").unwrap();
    for args in [
        vec!["current"],
        vec!["teardown", "--all"],
        vec![
            "apply",
            "--role",
            "qa",
            "--target",
            "claude-subagent",
            "--session-id",
            "x",
        ],
    ] {
        cmd(&sb)
            .args(&args)
            .assert()
            .code(5)
            .stderr(predicate::str::contains("corrupt state.json"));
    }
}

#[test]
fn teardown_all_clears_every_session() {
    let sb = sandbox();
    for (role, target, id) in [
        ("reviewer", "claude-teammate", "a1"),
        ("implementer", "codex-agent", "a2"),
    ] {
        cmd(&sb)
            .args([
                "apply",
                "--role",
                role,
                "--target",
                target,
                "--session-id",
                id,
            ])
            .assert()
            .code(0);
    }
    cmd(&sb)
        .args(["teardown", "--all"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("✓ session a1 torn down"))
        .stdout(predicate::str::contains("✓ session a2 torn down"));
    cmd(&sb)
        .arg("current")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("(none)"));
    // --all with nothing left is a friendly no-op.
    cmd(&sb)
        .args(["teardown", "--all"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("nothing to tear down"));
}

/// `current` with no state file at all: friendly empty output, exit 0.
#[test]
fn current_empty_state_is_friendly() {
    let sb = sandbox();
    cmd(&sb)
        .arg("current")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("ACTIVE SESSIONS"))
        .stdout(predicate::str::contains("(none)"));
    let assert = cmd(&sb).args(["current", "--json"]).assert().code(0);
    let json: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(json["sessions"], serde_json::json!([]));
}

/// CLAUDE.md marked block carries the session id at apply time and is
/// removed wherever it sits at teardown, preserving content around it.
#[test]
fn claude_md_block_session_marker_and_removal() {
    let sb = sandbox();
    fs::write(sb.repo.join("CLAUDE.md"), "# Notes\n").unwrap();
    cmd(&sb)
        .args([
            "apply",
            "--role",
            "reviewer",
            "--target",
            "claude-teammate",
            "--session-id",
            "blk-1",
        ])
        .assert()
        .code(0);
    let md = fs::read_to_string(sb.repo.join("CLAUDE.md")).unwrap();
    assert!(
        md.contains("<!-- agent-profile:begin session=blk-1 role=reviewer -->"),
        "{md}"
    );
    // The block moves — and a foreign line is appended after it.
    let moved = format!(
        "{}\n# Footer added by user\n",
        md.replace("# Notes\n", "").trim_end()
    );
    fs::write(sb.repo.join("CLAUDE.md"), format!("# Notes\n\n{moved}")).unwrap();

    cmd(&sb)
        .args(["teardown", "--session-id", "blk-1"])
        .assert()
        .code(0);
    let after = fs::read_to_string(sb.repo.join("CLAUDE.md")).unwrap();
    assert!(!after.contains("agent-profile:begin"), "{after}");
    assert!(after.contains("# Notes"), "{after}");
    assert!(after.contains("# Footer added by user"), "{after}");
}

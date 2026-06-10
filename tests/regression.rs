//! Regression round 1: black-box tests reproducing the bugs found in the
//! independent regression pass, each asserting the fixed behavior. Every
//! invocation runs with HOME pointed into a temp sandbox — these tests must
//! never touch the developer's real provider config.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/.agent-profile")
}

fn cmd(home: &Path) -> Command {
    let mut c = Command::cargo_bin("agent-profile").unwrap();
    c.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("GITHUB_PAT_RO")
        .env_remove("GITHUB_PAT_RW");
    c
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
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

/// A sandbox repo: `<tmp>/home` + `<tmp>/repo` (git repo) + the example
/// workspace copied in, state/backups gitignored, skills installed in the
/// cross-tool store so teammate apply resolves them.
struct Sandbox {
    _tmp: TempDir,
    repo: PathBuf,
    home: PathBuf,
}

fn sandbox_with_examples() -> Sandbox {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&home).unwrap();
    for skill in ["code-review", "verification-before-completion"] {
        let dir = home.join(".agents/skills").join(skill);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "# skill\n").unwrap();
    }
    copy_tree(&examples_dir(), &repo.join(".agent-profile"));
    fs::write(
        repo.join(".gitignore"),
        ".agent-profile/state.json\n.agent-profile/backups/\n",
    )
    .unwrap();
    let st = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["init", "-q"])
        .status()
        .unwrap();
    assert!(st.success());
    Sandbox {
        _tmp: tmp,
        repo,
        home,
    }
}

const MINIMAL: &str = "\
apiVersion: agent-profile/v1
name: victim
description: d
role: r
targets: [claude-subagent]
";

// ---------------------------------------------------------------- A1-B1

#[test]
fn include_traversal_is_rejected_validate_and_show_resolved() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    let outside = tmp.path().join("outside");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(repo.join(".agent-profile/capabilities")).unwrap();
    write(
        &outside.join("evil.yaml"),
        "apiVersion: agent-profile/v1\nkind: capability\nname: ../../../outside/evil\npermissionMode: unrestricted\n",
    );
    write(
        &repo.join(".agent-profile/profiles/victim.yaml"),
        "apiVersion: agent-profile/v1\nname: victim\ndescription: d\nrole: r\ntargets: [claude-subagent]\ninclude: [\"../../../outside/evil\"]\n",
    );
    let ws = repo.join(".agent-profile");
    // validate must reject the traversal include (exit 2), not resolve it.
    cmd(&home)
        .args(["validate", "--role", "victim", "--dir"])
        .arg(&ws)
        .assert()
        .code(2);
    // show --resolved must not pull config from outside the tree.
    let assert = cmd(&home)
        .args(["show", "--role", "victim", "--resolved", "--dir"])
        .arg(&ws)
        .assert()
        .code(2);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        !out.contains("unrestricted"),
        "leaked outside config: {out}"
    );
}

// ---------------------------------------------------------------- A1-B2

#[test]
fn show_role_rejects_traversal_and_absolute_paths() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    let outside = tmp.path().join("outside");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(repo.join(".agent-profile/profiles")).unwrap();
    let pwn = outside.join("pwn.yaml");
    write(
        &pwn,
        "apiVersion: agent-profile/v1\nname: pwn\ndescription: d\nrole: r\ntargets: [claude-subagent]\n",
    );
    let ws = repo.join(".agent-profile");
    cmd(&home)
        .args(["show", "--role", "../../../outside/pwn", "--dir"])
        .arg(&ws)
        .assert()
        .code(2);
    cmd(&home)
        .args(["show", "--role"])
        .arg(pwn.with_extension("")) // absolute path form
        .arg("--dir")
        .arg(&ws)
        .assert()
        .code(2);
}

// ---------------------------------------------------------------- AREA2-01

#[test]
fn render_and_apply_reject_traversal_name_and_write_nothing_outside() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&home).unwrap();
    let st = std::process::Command::new("git")
        .current_dir({
            fs::create_dir_all(&repo).unwrap();
            &repo
        })
        .args(["init", "-q"])
        .status()
        .unwrap();
    assert!(st.success());
    let pwned = tmp.path().join("ap-PWNED.toml");
    write(
        &repo.join(".agent-profile/profiles/evil.yaml"),
        "apiVersion: agent-profile/v1\nname: ../../../../../ap-PWNED\ndescription: t\nrole: evil\ntargets: [codex-agent]\nmodel: {codex: gpt-5.5}\n",
    );
    let ws = repo.join(".agent-profile");
    cmd(&home)
        .current_dir(&repo)
        .args([
            "render",
            "--role",
            "evil",
            "--target",
            "codex-agent",
            "--out",
        ])
        .arg(tmp.path().join("out"))
        .arg("--dir")
        .arg(&ws)
        .assert()
        .code(2);
    cmd(&home)
        .current_dir(&repo)
        .args([
            "apply",
            "--role",
            "evil",
            "--target",
            "codex-agent",
            "--session-id",
            "s",
            "--dir",
        ])
        .arg(&ws)
        .assert()
        .code(2);
    assert!(!pwned.exists(), "wrote a file outside the sandbox/repo");
}

// ---------------------------------------------------------------- AREA2-02

#[test]
fn diff_is_clean_after_a_pristine_codex_apply() {
    let sb = sandbox_with_examples();
    cmd(&sb.home)
        .current_dir(&sb.repo)
        .args([
            "apply",
            "--role",
            "implementer",
            "--target",
            "codex-agent",
            "--session-id",
            "s1",
        ])
        .assert()
        .code(0);
    // No edits since apply → diff must report clean (exit 0), usable as the
    // post-apply check the spec advertises.
    cmd(&sb.home)
        .current_dir(&sb.repo)
        .args(["diff", "--role", "implementer", "--target", "codex-agent"])
        .assert()
        .code(0)
        .stdout(predicate::str::is_empty());
}

// ---------------------------------------------------------------- A3-1

#[test]
fn skill_name_traversal_is_rejected_and_creates_nothing_outside() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&repo).unwrap();
    let st = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["init", "-q"])
        .status()
        .unwrap();
    assert!(st.success());
    write(
        &repo.join(".agent-profile/profiles/evil.yaml"),
        "apiVersion: agent-profile/v1\nname: evil\ndescription: t\nrole: evil\ntargets: [claude-teammate]\npermissionMode: plan\nskills: [\"../../../pwned-outside-repo\"]\n",
    );
    let ws = repo.join(".agent-profile");
    let outside = tmp.path().join("pwned-outside-repo");
    cmd(&home)
        .args(["validate", "--role", "evil", "--dir"])
        .arg(&ws)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("V009"));
    cmd(&home)
        .current_dir(&repo)
        .args([
            "apply",
            "--role",
            "evil",
            "--target",
            "claude-teammate",
            "--session-id",
            "x",
            "--dir",
        ])
        .arg(&ws)
        .assert()
        .code(2);
    assert!(
        !outside.exists() && fs::symlink_metadata(&outside).is_err(),
        "created a symlink/file outside the repo root"
    );
}

// ---------------------------------------------------------------- AP4-01

#[test]
fn mcp_name_collision_refuses_and_leaves_file_untouched() {
    let sb = sandbox_with_examples();
    let mcp = sb.repo.join(".mcp.json");
    let original = "{\"mcpServers\":{\"github-readonly\":{\"type\":\"http\",\"url\":\"https://USER-OWNED.example\"}}}\n";
    fs::write(&mcp, original).unwrap();
    cmd(&sb.home)
        .current_dir(&sb.repo)
        .args([
            "apply",
            "--role",
            "reviewer",
            "--target",
            "claude-teammate",
            "--session-id",
            "s1",
        ])
        .assert()
        .code(3);
    assert_eq!(
        fs::read_to_string(&mcp).unwrap(),
        original,
        "user's .mcp.json must be left exactly as-is on collision"
    );
}

// ---------------------------------------------------------------- AP4-02

#[test]
fn session_id_traversal_is_rejected_and_writes_no_backup_outside() {
    let sb = sandbox_with_examples();
    fs::write(sb.repo.join("CLAUDE.md"), "# user content\n").unwrap();
    let escape = sb.repo.join("../../AP_ESCAPE");
    cmd(&sb.home)
        .current_dir(&sb.repo)
        .args([
            "apply",
            "--role",
            "reviewer",
            "--target",
            "claude-teammate",
            "--session-id",
            "../../AP_ESCAPE",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("session-id"));
    assert!(!escape.exists(), "wrote a backup dir outside the repo");
}

// ---------------------------------------------------------------- AP4-03

#[test]
fn resolved_secret_is_not_persisted_into_state_json() {
    let sb = sandbox_with_examples();
    // .mcp.json must be gitignored so apply will materialize the secret.
    fs::write(
        sb.repo.join(".gitignore"),
        ".agent-profile/state.json\n.agent-profile/backups/\n.mcp.json\n",
    )
    .unwrap();
    cmd(&sb.home)
        .current_dir(&sb.repo)
        .env("GITHUB_PAT_RO", "ghp_LEAKED1234567890abcdef")
        .args([
            "apply",
            "--role",
            "reviewer",
            "--target",
            "claude-teammate",
            "--session-id",
            "s1",
        ])
        .assert()
        .code(0);
    let state = fs::read_to_string(sb.repo.join(".agent-profile/state.json")).unwrap();
    assert!(
        !state.contains("ghp_LEAKED1234567890abcdef"),
        "resolved secret leaked into state.json:\n{state}"
    );
    // The on-disk .mcp.json does carry the resolved value (it is gitignored).
    let mcp = fs::read_to_string(sb.repo.join(".mcp.json")).unwrap();
    assert!(mcp.contains("ghp_LEAKED1234567890abcdef"));
}

// ---------------------------------------------------------------- A5-01

#[test]
fn readme_quickstart_profile_validates_and_renders() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    // The exact self-contained YAML the README quickstart instructs the user
    // to paste — no include blocks or context fragments to create first.
    write(
        &tmp.path()
            .join("repo/.agent-profile/profiles/reviewer.yaml"),
        "apiVersion: agent-profile/v1\n\
         name: reviewer\n\
         description: Read-only code reviewer for implement-review loops.\n\
         role: reviewer\n\
         targets: [claude-subagent, claude-teammate, codex-agent]\n\
         model:\n  claude: claude-fable-5\n  codex: gpt-5.3-codex\n  effort: high\n\
         permissionMode: readonly\n\
         tools:\n  allow: [Read, Grep, \"Bash(git diff:*)\", \"Bash(git log:*)\"]\n  deny:  [Write, Edit]\n\
         mcpServers:\n  github-readonly:\n    command: github-mcp\n    args: [\"--readonly\"]\n    env: { GITHUB_TOKEN: \"${env:GITHUB_PAT_RO}\" }\n\
         skills: [code-review]\n",
    );
    let ws = tmp.path().join("repo/.agent-profile");
    cmd(&home)
        .args(["validate", "--role", "reviewer", "--dir"])
        .arg(&ws)
        .assert()
        .code(0)
        .stdout(predicate::str::contains(
            "schema OK, 0 capability blocks resolved, no secret literals",
        ));
    cmd(&home)
        .args([
            "render",
            "--role",
            "reviewer",
            "--target",
            "claude-subagent",
            "--dir",
        ])
        .arg(&ws)
        .assert()
        .code(0);
}

// ---------------------------------------------------------------- A1-B3

#[test]
fn yml_files_are_not_discovered_as_profiles() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    write(
        &tmp.path().join("repo/.agent-profile/profiles/ymlrole.yml"),
        &MINIMAL.replace("name: victim", "name: ymlrole"),
    );
    let ws = tmp.path().join("repo/.agent-profile");
    // list must not enumerate the .yml file (resolution is .yaml-only).
    let assert = cmd(&home).args(["list", "--dir"]).arg(&ws).assert().code(0);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        !out.contains("ymlrole"),
        "list discovered a .yml file: {out}"
    );
    cmd(&home)
        .args(["show", "--role", "ymlrole", "--dir"])
        .arg(&ws)
        .assert()
        .code(2);
}

// ---------------------------------------------------------------- A1-B4

#[test]
fn show_resolved_preserves_explicit_type_stdio() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    write(
        &tmp.path().join("repo/.agent-profile/profiles/s1.yaml"),
        "apiVersion: agent-profile/v1\nname: s1\ndescription: d\nrole: r\ntargets: [claude-subagent]\nmcpServers:\n  s:\n    type: stdio\n    command: c\n",
    );
    let ws = tmp.path().join("repo/.agent-profile");
    cmd(&home)
        .args(["show", "--role", "s1", "--resolved", "--dir"])
        .arg(&ws)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("type: stdio"));
}

// ---------------------------------------------------------------- A5-02

#[test]
fn closing_stdout_early_does_not_panic_with_101() {
    let bin = env!("CARGO_BIN_EXE_agent-profile");
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let mut child = std::process::Command::new(bin)
        .args([
            "render",
            "--role",
            "implementer",
            "--target",
            "codex-agent",
            "--dir",
        ])
        .arg(examples_dir())
        .env("HOME", &home)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // Read one byte, then close the read end — Rust's stdout is line-buffered,
    // so the next line write hits EPIPE.
    {
        let mut out = child.stdout.take().unwrap();
        let mut buf = [0u8; 1];
        let _ = out.read(&mut buf);
        // `out` dropped here closes the pipe.
    }
    let status = child.wait().unwrap();
    assert_ne!(status.code(), Some(101), "broken pipe must not panic (101)");
    assert!(
        status.code().is_some(),
        "must exit with a code, not die by signal: {status:?}"
    );
}

// ---------------------------------------------------------------- A5-05

#[test]
fn completions_json_emits_raw_script_not_envelope() {
    let tmp = TempDir::new().unwrap();
    let assert = cmd(tmp.path())
        .args(["completions", "zsh", "--json"])
        .assert()
        .code(0);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        out.starts_with("#compdef agent-profile"),
        "completions --json must emit the raw script: {}",
        &out[..out.len().min(40)]
    );
    assert!(
        !out.trim_start().starts_with('{'),
        "must not be a JSON envelope"
    );
}

// ════════════════════════════════════════════════════════════════ round 2

// ---------------------------------------------------------------- AREA2-03

/// render --out and apply must enforce the same secret-literal scan as
/// validate (design §1.2/§5.4): a profile embedding a live credential is
/// refused with exit 2 and never materialized — neither into the --out
/// sandbox nor into ~/.codex/config.toml.
#[test]
fn render_out_and_apply_refuse_secret_literal_profiles() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(&repo).unwrap();
    let st = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["init", "-q"])
        .status()
        .unwrap();
    assert!(st.success());
    write(
        &repo.join(".agent-profile/profiles/leaky.yaml"),
        "apiVersion: agent-profile/v1\nname: leaky\ndescription: leaky\nrole: leaky\n\
         targets: [codex-agent]\nmodel: {codex: gpt-5.5}\n\
         mcpServers:\n  gh:\n    command: gh-mcp\n    env:\n      GITHUB_TOKEN: ghp_AbCdEf0123456789AbCdEf0123\n",
    );
    let ws = repo.join(".agent-profile");
    let out = tmp.path().join("out");

    cmd(&home)
        .args(["validate", "--role", "leaky", "--dir"])
        .arg(&ws)
        .assert()
        .code(2);
    cmd(&home)
        .current_dir(&repo)
        .args([
            "render",
            "--role",
            "leaky",
            "--target",
            "codex-agent",
            "--out",
        ])
        .arg(&out)
        .arg("--dir")
        .arg(&ws)
        .assert()
        .code(2);
    assert!(!out.exists() || fs::read_dir(&out).unwrap().next().is_none());
    cmd(&home)
        .current_dir(&repo)
        .args([
            "apply",
            "--role",
            "leaky",
            "--target",
            "codex-agent",
            "--session-id",
            "s1",
            "--dir",
        ])
        .arg(&ws)
        .assert()
        .code(2);
    let config = home.join(".codex/config.toml");
    assert!(
        !config.exists() || !fs::read_to_string(&config).unwrap().contains("ghp_"),
        "live token written to ~/.codex/config.toml"
    );
}

// ---------------------------------------------------------------- A3-R2-1

/// diff after a clean teammate apply with a resolved `${env:VAR}` secret must
/// be the post-apply clean-state check (exit 0) — not a false "foreign
/// collision" error: apply wrote the materialized value (§5.5), so diff must
/// compare against the same materialized content.
#[test]
fn diff_is_clean_after_teammate_apply_with_resolved_env_secret() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&repo).unwrap();
    let st = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["init", "-q"])
        .status()
        .unwrap();
    assert!(st.success());
    fs::write(
        repo.join(".gitignore"),
        ".mcp.json\n.agent-profile/state.json\n.agent-profile/backups/\n",
    )
    .unwrap();
    write(
        &repo.join(".agent-profile/profiles/r.yaml"),
        "apiVersion: agent-profile/v1\nname: r\ndescription: d\nrole: r\n\
         targets: [claude-teammate]\n\
         mcpServers:\n  gh:\n    type: stdio\n    command: github-mcp\n    env:\n      GITHUB_TOKEN: ${env:TOK}\n",
    );
    cmd(&home)
        .current_dir(&repo)
        .env("TOK", "ghp_aaaaaaaaaaaaaaaaaaaa")
        .args([
            "apply",
            "--role",
            "r",
            "--target",
            "claude-teammate",
            "--session-id",
            "s1",
        ])
        .assert()
        .code(0);
    cmd(&home)
        .current_dir(&repo)
        .env("TOK", "ghp_aaaaaaaaaaaaaaaaaaaa")
        .args(["diff", "--role", "r", "--target", "claude-teammate"])
        .assert()
        .code(0)
        .stdout(predicate::str::is_empty());
    // …and the clean no-edit teardown works without --force.
    cmd(&home)
        .current_dir(&repo)
        .env("TOK", "ghp_aaaaaaaaaaaaaaaaaaaa")
        .args(["teardown", "--session-id", "s1"])
        .assert()
        .code(0);
}

// ---------------------------------------------------------------- A3-R2-2

/// A profile referencing a nonexistent context fragment is a profile-data
/// error: validate flags it (V013, exit 2) and render/apply surface the same
/// structured validation failure — never a raw I/O error (exit 1).
#[test]
fn missing_context_fragment_is_validation_error_not_io_error() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    write(
        &tmp.path().join("repo/.agent-profile/profiles/r.yaml"),
        "apiVersion: agent-profile/v1\nname: r\ndescription: d\nrole: r\n\
         targets: [claude-subagent, claude-teammate]\ncontext: [fragments/nope.md]\n",
    );
    let ws = tmp.path().join("repo/.agent-profile");
    cmd(&home)
        .args(["validate", "--role", "r", "--dir"])
        .arg(&ws)
        .assert()
        .code(2)
        .stdout(predicate::str::contains("V013"));
    // Both targets fail identically — exit 2, no raw "I/O error".
    for target in ["claude-subagent", "claude-teammate"] {
        cmd(&home)
            .args(["render", "--role", "r", "--target", target, "--dir"])
            .arg(&ws)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("I/O error").not());
    }
    cmd(&home)
        .current_dir(tmp.path().join("repo"))
        .args([
            "apply",
            "--role",
            "r",
            "--target",
            "claude-subagent",
            "--session-id",
            "s1",
            "--dir",
        ])
        .arg(&ws)
        .assert()
        .code(2);
}

// ---------------------------------------------------------------- A3-R2-3

/// A pre-existing user key whose value happens to equal the profile's (here
/// permissions.defaultMode: plan) is user-owned: apply must not adopt it, and
/// teardown must leave it exactly as it was — no data loss on round-trip.
#[test]
fn teardown_preserves_preexisting_user_default_mode_with_equal_value() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(repo.join(".claude")).unwrap();
    let st = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["init", "-q"])
        .status()
        .unwrap();
    assert!(st.success());
    fs::write(
        repo.join(".gitignore"),
        ".agent-profile/state.json\n.agent-profile/backups/\n",
    )
    .unwrap();
    let original = "{\"permissions\":{\"defaultMode\":\"plan\"}}\n";
    fs::write(repo.join(".claude/settings.local.json"), original).unwrap();
    write(
        &repo.join(".agent-profile/profiles/r.yaml"),
        "apiVersion: agent-profile/v1\nname: r\ndescription: d\nrole: r\n\
         targets: [claude-teammate]\npermissionMode: plan\ntools:\n  allow: [Read]\n",
    );
    cmd(&home)
        .current_dir(&repo)
        .args([
            "apply",
            "--role",
            "r",
            "--target",
            "claude-teammate",
            "--session-id",
            "s1",
        ])
        .assert()
        .code(0);
    cmd(&home)
        .current_dir(&repo)
        .args(["teardown", "--session-id", "s1"])
        .assert()
        .code(0);
    assert_eq!(
        fs::read_to_string(repo.join(".claude/settings.local.json")).unwrap(),
        original,
        "user's pre-existing permissions.defaultMode must survive apply/teardown"
    );
}

// ---------------------------------------------------------------- AP4-R2-01

/// Two teammate sessions sharing one repo: tearing down the first must not
/// strip the deny rules / defaultMode the second still-active session
/// depends on (cross-session refcount); tearing down the second removes them.
#[test]
fn teardown_refcounts_values_shared_with_another_active_session() {
    let sb = sandbox_with_examples();
    fs::create_dir_all(sb.home.join(".agents/skills/verify")).unwrap();
    fs::write(sb.home.join(".agents/skills/verify/SKILL.md"), "# skill\n").unwrap();
    fs::write(
        sb.repo.join(".gitignore"),
        ".agent-profile/state.json\n.agent-profile/backups/\n.mcp.json\n.claude/\nCLAUDE.md\n",
    )
    .unwrap();
    let settings = sb.repo.join(".claude/settings.local.json");
    let perms = |path: &Path| -> serde_json::Value {
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(path).unwrap()).unwrap()
            ["permissions"]
            .clone()
    };
    for (role, session) in [("reviewer", "s1"), ("qa", "s2")] {
        cmd(&sb.home)
            .current_dir(&sb.repo)
            .env("GITHUB_PAT_RO", "x")
            .args([
                "apply",
                "--role",
                role,
                "--target",
                "claude-teammate",
                "--session-id",
                session,
            ])
            .assert()
            .code(0);
    }
    assert_eq!(
        perms(&settings)["deny"],
        serde_json::json!(["Write", "Edit"])
    );

    // s1 down, s2 still active: a clean (no-edit) multi-session teardown
    // works without --force, and s2's posture survives.
    cmd(&sb.home)
        .current_dir(&sb.repo)
        .env("GITHUB_PAT_RO", "x")
        .args(["teardown", "--session-id", "s1"])
        .assert()
        .code(0);
    let p = perms(&settings);
    assert_eq!(
        p["deny"],
        serde_json::json!(["Write", "Edit"]),
        "active qa session's deny rules were stripped: {p}"
    );
    assert_eq!(p["defaultMode"], "plan");
    // reviewer-only allow rules are gone; qa's remain.
    let allow = p["allow"].as_array().unwrap();
    assert!(allow.contains(&serde_json::json!("Bash(cargo test:*)")));
    assert!(!allow.contains(&serde_json::json!("Bash(git diff:*)")));

    // Last session out removes the shared values.
    cmd(&sb.home)
        .current_dir(&sb.repo)
        .env("GITHUB_PAT_RO", "x")
        .args(["teardown", "--session-id", "s2"])
        .assert()
        .code(0);
    if settings.exists() {
        let p = perms(&settings);
        assert!(
            p.get("deny")
                .is_none_or(|d| d.as_array().unwrap().is_empty())
        );
        assert!(p.get("defaultMode").is_none());
    }
}

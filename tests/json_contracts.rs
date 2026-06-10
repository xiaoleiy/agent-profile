//! S5-T1: the `--json` output contracts are FROZEN at `schemaVersion: 1`.
//!
//! Every command's envelope is golden-tested byte-for-byte against
//! `tests/golden/<command>.json`, and `docs/json-api.md` must embed each
//! golden verbatim — the documented examples are real binary output and
//! cannot rot. Any intentional change to these envelopes is a breaking
//! change: bump `schemaVersion` (design §2), regenerate with
//! `UPDATE_GOLDENS=1 cargo test --test json_contracts`, and update the doc.
//!
//! All invocations run with HOME (and config-dir env) sandboxed — never the
//! developer's real provider config.

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn golden_dir() -> PathBuf {
    manifest_dir().join("tests/golden")
}

const GOLDEN_NAMES: [&str; 9] = [
    "list", "show", "validate", "render", "diff", "doctor", "apply", "current", "teardown",
];

/// Sandbox: `<tmp>/home` (HOME, with the reviewer skills installed so doctor
/// output is deterministic) + `<tmp>/repo` (git repo with the example
/// workspace, state/backups gitignored) + `<tmp>/emptybin` (PATH for doctor,
/// so `which` probes are deterministic).
struct Sandbox {
    _tmp: TempDir,
    repo: PathBuf,
    home: PathBuf,
    empty_path: PathBuf,
}

fn sandbox() -> Sandbox {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    let empty_path = tmp.path().join("emptybin");
    fs::create_dir_all(&empty_path).unwrap();
    for skill in ["code-review", "verification-before-completion"] {
        let dir = home.join(".agents/skills").join(skill);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), format!("# {skill}\n")).unwrap();
    }
    copy_tree(
        &manifest_dir().join("examples/.agent-profile"),
        &repo.join(".agent-profile"),
    );
    fs::write(
        repo.join(".gitignore"),
        ".agent-profile/state.json\n.agent-profile/backups/\n",
    )
    .unwrap();
    let status = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["init", "-q"])
        .status()
        .expect("git available");
    assert!(status.success());
    Sandbox {
        _tmp: tmp,
        repo,
        home,
        empty_path,
    }
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

/// `appliedAt` is the only volatile field in any v1 envelope — pin it.
fn normalize(raw: &str) -> String {
    const KEY: &str = "\"appliedAt\": \"";
    const PINNED: &str = "2026-06-10T09:14:03Z";
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(idx) = rest.find(KEY) {
        let start = idx + KEY.len();
        let end = start + rest[start..].find('"').expect("closing quote");
        out.push_str(&rest[..start]);
        out.push_str(PINNED);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn check_golden(name: &str, stdout: &[u8]) {
    let actual = normalize(std::str::from_utf8(stdout).unwrap());
    let path = golden_dir().join(format!("{name}.json"));
    if std::env::var_os("UPDATE_GOLDENS").is_some() {
        fs::create_dir_all(golden_dir()).unwrap();
        fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {path:?} — run with UPDATE_GOLDENS=1"));
    assert_eq!(
        actual, expected,
        "--json contract drifted for `{name}` (schemaVersion 1 is frozen). \
         If the change is intentional, bump schemaVersion per design §2, run \
         UPDATE_GOLDENS=1, and update docs/json-api.md."
    );
}

#[test]
fn json_contracts_match_goldens() {
    let sb = sandbox();

    let assert = cmd(&sb).args(["list", "--json"]).assert().code(0);
    check_golden("list", &assert.get_output().stdout);

    let assert = cmd(&sb)
        .args(["show", "--role", "reviewer", "--resolved", "--json"])
        .assert()
        .code(0);
    check_golden("show", &assert.get_output().stdout);

    let assert = cmd(&sb).args(["validate", "--json"]).assert().code(0);
    check_golden("validate", &assert.get_output().stdout);

    let assert = cmd(&sb)
        .args([
            "render",
            "--role",
            "implementer",
            "--target",
            "codex-agent",
            "--json",
        ])
        .assert()
        .code(0);
    check_golden("render", &assert.get_output().stdout);

    // diff exits 3: creates/merges pending in a clean sandbox.
    let assert = cmd(&sb)
        .args([
            "diff",
            "--role",
            "implementer",
            "--target",
            "codex-agent",
            "--json",
        ])
        .assert()
        .code(3);
    check_golden("diff", &assert.get_output().stdout);

    // PATH is an empty dir so `which` probes (mcp commands) are deterministic;
    // --assume-version skips the CLI probe entirely. Exit 4: honest errors
    // (fields the documented claude version does not honor).
    let assert = cmd(&sb)
        .env("PATH", &sb.empty_path)
        .args([
            "doctor",
            "--role",
            "reviewer",
            "--target",
            "claude-teammate",
            "--assume-version",
            "2.0.34",
            "--json",
        ])
        .assert()
        .code(4);
    check_golden("doctor", &assert.get_output().stdout);

    let assert = cmd(&sb)
        .args([
            "apply",
            "--role",
            "reviewer",
            "--target",
            "claude-teammate",
            "--session-id",
            "run7-reviewer",
            "--json",
        ])
        .assert()
        .code(0);
    check_golden("apply", &assert.get_output().stdout);

    let assert = cmd(&sb).args(["current", "--json"]).assert().code(0);
    check_golden("current", &assert.get_output().stdout);

    let assert = cmd(&sb)
        .args(["teardown", "--session-id", "run7-reviewer", "--json"])
        .assert()
        .code(0);
    check_golden("teardown", &assert.get_output().stdout);
}

#[test]
fn every_golden_envelope_declares_schema_version_1() {
    for name in GOLDEN_NAMES {
        let path = golden_dir().join(format!("{name}.json"));
        let raw = fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing golden {path:?} — run with UPDATE_GOLDENS=1"));
        let value: serde_json::Value =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("golden {name} not JSON: {e}"));
        assert_eq!(
            value["schemaVersion"],
            serde_json::json!(1),
            "golden {name} must carry schemaVersion 1"
        );
    }
}

/// docs/json-api.md embeds every golden verbatim, so the documented examples
/// are guaranteed to be real binary output.
#[test]
fn json_api_doc_embeds_every_golden_verbatim() {
    let doc = fs::read_to_string(manifest_dir().join("docs/json-api.md"))
        .expect("docs/json-api.md exists");
    for name in GOLDEN_NAMES {
        let golden = fs::read_to_string(golden_dir().join(format!("{name}.json")))
            .unwrap_or_else(|_| panic!("missing golden {name} — run with UPDATE_GOLDENS=1"));
        assert!(
            doc.contains(golden.trim_end()),
            "docs/json-api.md does not embed the `{name}` golden verbatim — \
             update the doc from tests/golden/{name}.json"
        );
    }
}

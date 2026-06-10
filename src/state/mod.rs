//! state.json session ledger, sha256 hashing, per-session backups, and the
//! apply/teardown executors (design §3.4, §5.1, §5.2).
//!
//! The executor enforces the never-touch denylist per action right before it
//! mutates — defense in depth below the adapters and below the plan-level
//! check in the CLI; no flag reaches this layer.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::adapters::{Action, Plan, PlanContext, atomic_write, claude, codex, denylist};
use crate::error::Error;

pub const STATE_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------- ledger types

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub schema_version: u32,
    #[serde(default)]
    pub sessions: BTreeMap<String, Session>,
    /// One-time `.gitignore` hint tracking (extension field; absent in the
    /// design §3.4 example and skipped while false).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gitignore_hint_shown: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            schema_version: STATE_SCHEMA_VERSION,
            sessions: BTreeMap::new(),
            gitignore_hint_shown: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub role: String,
    pub target: String,
    pub scope: String,
    pub applied_at: String,
    pub agent_profile_version: String,
    pub actions: Vec<ActionRecord>,
}

/// One executed action (design §3.4 record shapes). Fields beyond the design
/// example (`content`, `appended`, `createdDirs`, `forced`) are reversal
/// bookkeeping and are skipped when absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionRecord {
    pub op: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker: Option<String>,
    #[serde(rename = "linkTarget", skip_serializing_if = "Option::is_none")]
    pub link_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
    #[serde(rename = "hashBefore", skip_serializing_if = "Option::is_none")]
    pub hash_before: Option<String>,
    #[serde(rename = "hashAfter", skip_serializing_if = "Option::is_none")]
    pub hash_after: Option<String>,
    /// Exact content we wrote (merge fragment / appended block) — used for
    /// key-level reversal and post-apply drift verification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Array entries we actually appended per `…[+N]` path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub appended: Option<BTreeMap<String, Vec<serde_json::Value>>>,
    /// Directories we had to create for this action (outermost first).
    #[serde(rename = "createdDirs", skip_serializing_if = "Option::is_none")]
    pub created_dirs: Option<Vec<String>>,
    /// Set when `--force` overrode a drift refusal for this action (§5.1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forced: Option<bool>,
    /// Merge-keys only: set when the target file, although it existed at this
    /// apply (`hashBefore` present), owes its existence to this tool — an
    /// earlier still-active session created it (its record has no
    /// `hashBefore`, or carries this flag in turn). Teardown uses it so the
    /// last session out deletes the emptied skeleton regardless of teardown
    /// order (§3.4 reversibility: the repo ends byte-clean).
    #[serde(rename = "fileCreated", skip_serializing_if = "Option::is_none")]
    pub file_created: Option<bool>,
}

impl ActionRecord {
    fn new(op: &str, path: &str) -> Self {
        ActionRecord {
            op: op.to_string(),
            path: path.to_string(),
            keys: None,
            marker: None,
            link_target: None,
            backup: None,
            hash_before: None,
            hash_after: None,
            content: None,
            appended: None,
            created_dirs: None,
            forced: None,
            file_created: None,
        }
    }
}

// ---------------------------------------------------------------- load / save

pub fn state_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join("state.json")
}

pub fn backup_dir(workspace_root: &Path, session_id: &str) -> PathBuf {
    workspace_root.join("backups").join(session_id)
}

/// Load the ledger; a missing file is an empty ledger, a corrupt file is a
/// session/state error (exit 5) with a recovery hint — never a panic.
pub fn load(workspace_root: &Path) -> Result<State, Error> {
    let path = state_path(workspace_root);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(State::default()),
        Err(source) => return Err(Error::Io { path, source }),
    };
    serde_json::from_str(&raw).map_err(|e| {
        Error::Session(format!(
            "corrupt state.json at {}: {e}\n  → recover: restore the file from your own backup, \
             or delete it (active sessions will be forgotten — tear their files down by hand)",
            path.display()
        ))
    })
}

/// Atomic write-then-rename, like every other mutation.
pub fn save(workspace_root: &Path, state: &State) -> Result<(), Error> {
    let mut s = serde_json::to_string_pretty(state).expect("state always serializes");
    s.push('\n');
    atomic_write(&state_path(workspace_root), &s)
}

// ---------------------------------------------------------------- hashing / time

pub fn sha256_of(content: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(content.as_bytes()))
}

/// Current UTC time as `YYYY-MM-DDTHH:MM:SSZ` — the exact §3.4 `appliedAt`
/// shape (doctor's stale-session parser consumes it).
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    rfc3339_from_epoch(secs)
}

fn rfc3339_from_epoch(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's civil-from-days (inverse of doctor's days_from_civil).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------- shared helpers

fn read_opt(path: &Path) -> Result<Option<String>, Error> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn display_of(ctx: &PlanContext, abs: &Path) -> String {
    match abs.strip_prefix(&ctx.repo_root) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => abs.display().to_string(),
    }
}

fn is_toml(display_path: &str) -> bool {
    Path::new(display_path)
        .extension()
        .is_some_and(|e| e == "toml")
}

fn has_provenance(content: &str) -> bool {
    content.contains("generated by agent-profile")
}

/// `${env:VAR}` references appearing in `s`, in order of first appearance.
fn env_refs(s: &str) -> Vec<String> {
    let mut vars = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find("${env:") {
        rest = &rest[i + 6..];
        if let Some(j) = rest.find('}') {
            let name = &rest[..j];
            if !name.is_empty() && !vars.iter().any(|v| v == name) {
                vars.push(name.to_string());
            }
            rest = &rest[j + 1..];
        } else {
            break;
        }
    }
    vars
}

/// `content` with every *set* `${env:VAR}` reference replaced by its value.
/// Unset variables keep the reference (the pattern Claude itself supports).
fn substitute_set_env(content: &str) -> String {
    let mut out = content.to_string();
    for name in env_refs(content) {
        if let Ok(val) = std::env::var(&name) {
            out = out.replace(&format!("${{env:{name}}}"), &val);
        }
    }
    out
}

/// §5.5 materialization rule for `.mcp.json` merge content, shared by apply
/// (writes the result) and diff (compares against it — regression A3-R2-1):
/// set `${env:VAR}` references resolve into the written value **only if** the
/// target file is gitignored. `Err(vars)` = the file is not gitignored, so
/// materializing those variables is forbidden. Non-`.mcp.json` paths and
/// contents without set references pass through unchanged.
pub fn resolve_mcp_env(content: &str, abs: &Path, repo_root: &Path) -> Result<String, Vec<String>> {
    if abs.file_name().is_none_or(|n| n != ".mcp.json") {
        return Ok(content.to_string());
    }
    let set_vars: Vec<String> = env_refs(content)
        .into_iter()
        .filter(|v| std::env::var(v).is_ok())
        .collect();
    if set_vars.is_empty() {
        return Ok(content.to_string());
    }
    if !is_git_ignored(repo_root, abs) {
        return Err(set_vars);
    }
    Ok(substitute_set_env(content))
}

/// diff support (regression A3-R3-1): a merge key recorded by an active
/// session is OUR key, not a foreign collision (§3.2.2 reserves collision
/// errors for "existing keys we did not create"). state.json deliberately
/// stores the merge fragment UNRESOLVED (§5.5 — never the materialized
/// secret), so after apply the only faithful comparison is at that level:
/// when the plan's unresolved value for a key equals the recorded one (the
/// profile did not change) and the on-disk value still matches the recorded
/// fragment modulo `${env:VAR}` materialization, the key is clean — even if
/// the variable is now unset or rotated in the diffing shell. For such keys
/// the on-disk value is adopted into the effective fragment so the merge
/// neither errors nor reports a spurious diff. Only JSON files materialize
/// env references; other paths pass through unchanged.
pub fn adopt_session_owned_values(
    effective: &str,
    unresolved: &str,
    keys: &[String],
    current: Option<&str>,
    records: &[&ActionRecord],
    path: &str,
) -> String {
    if is_toml(path) || records.is_empty() {
        return effective.to_string();
    }
    let Some(current) = current else {
        return effective.to_string();
    };
    let (Ok(cur), Ok(mut eff), Ok(plan)) = (
        serde_json::from_str::<serde_json::Value>(current),
        serde_json::from_str::<serde_json::Value>(effective),
        serde_json::from_str::<serde_json::Value>(unresolved),
    ) else {
        return effective.to_string();
    };
    for key in keys {
        if key.contains("[+") {
            continue; // appended entries are tracked individually
        }
        let Some(cur_val) = json_at(&cur, key).cloned() else {
            continue;
        };
        let Some(plan_val) = json_at(&plan, key) else {
            continue;
        };
        let owned_and_clean = records.iter().any(|r| {
            r.keys
                .as_ref()
                .is_some_and(|ks| ks.iter().any(|k| k == key))
                && r.content.as_ref().is_some_and(|c| {
                    serde_json::from_str::<serde_json::Value>(c)
                        .ok()
                        .and_then(|frag| json_at(&frag, key).cloned())
                        .is_some_and(|rec_val| {
                            rec_val == *plan_val && matches_modulo_env(&rec_val, &cur_val)
                        })
                })
        });
        if owned_and_clean {
            json_set_at(&mut eff, key, cur_val);
        }
    }
    let mut s = serde_json::to_string_pretty(&eff).expect("JSON value serializes");
    s.push('\n');
    s
}

/// Deep equality between an unresolved spec value and an on-disk value,
/// treating each `${env:VAR}` reference in spec strings as a wildcard (the
/// materialized secret is unrecoverable once the variable is unset/rotated).
fn matches_modulo_env(spec: &serde_json::Value, disk: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (spec, disk) {
        (Value::String(s), Value::String(d)) => env_wildcard_match(s, d),
        (Value::Object(s), Value::Object(d)) => {
            s.len() == d.len()
                && s.iter()
                    .all(|(k, sv)| d.get(k).is_some_and(|dv| matches_modulo_env(sv, dv)))
        }
        (Value::Array(s), Value::Array(d)) => {
            s.len() == d.len()
                && s.iter()
                    .zip(d.iter())
                    .all(|(sv, dv)| matches_modulo_env(sv, dv))
        }
        _ => spec == disk,
    }
}

/// Does `disk` match `spec` when every `${env:VAR}` in `spec` is a wildcard?
fn env_wildcard_match(spec: &str, disk: &str) -> bool {
    let mut literals: Vec<&str> = Vec::new();
    let mut rest = spec;
    let mut any_ref = false;
    while let Some(i) = rest.find("${env:") {
        match rest[i..].find('}') {
            Some(j) => {
                any_ref = true;
                literals.push(&rest[..i]);
                rest = &rest[i + j + 1..];
            }
            None => break,
        }
    }
    literals.push(rest);
    if !any_ref {
        return spec == disk;
    }
    let (first, middle_and_last) = literals.split_first().expect("non-empty");
    let Some(mut hay) = disk.strip_prefix(first) else {
        return false;
    };
    let (last, middles) = middle_and_last.split_last().expect("at least one ref");
    for lit in middles {
        if lit.is_empty() {
            continue; // adjacent wildcards collapse
        }
        match hay.find(lit) {
            Some(p) => hay = &hay[p + lit.len()..],
            None => return false,
        }
    }
    hay.ends_with(last)
}

/// Can `abs` end up committed from `repo_root`? Files outside the repo cannot;
/// inside, `git check-ignore` decides (design §5.5). A non-git directory is
/// conservatively treated as not ignored.
pub fn is_git_ignored(repo_root: &Path, abs: &Path) -> bool {
    if !abs.starts_with(repo_root) {
        return true;
    }
    std::process::Command::new("git")
        .current_dir(repo_root)
        .args(["check-ignore", "-q", "--"])
        .arg(abs)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Ancestor directories of `abs` that do not exist yet, outermost first.
fn missing_ancestors(abs: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let mut p = abs.parent();
    while let Some(d) = p {
        if d.as_os_str().is_empty() || d.exists() {
            break;
        }
        v.push(d.to_path_buf());
        p = d.parent();
    }
    v.reverse();
    v
}

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

// ---------------------------------------------------------------- apply executor

#[derive(Debug)]
pub struct ApplyOutcome {
    pub records: Vec<ActionRecord>,
    /// Plan actions that needed no work (e.g. a skill symlink that already
    /// points at the right target — foreign, so never recorded/torn down).
    pub unchanged_paths: Vec<String>,
}

struct Prep {
    /// Merge content after `${env:VAR}` resolution (merge-keys only).
    effective: Option<String>,
    skip: bool,
    needs_force: bool,
}

/// Interpret a plan against the filesystem: per-action backup + hash
/// recording, atomic writes, executor-level denylist, mid-apply rollback.
/// `active` is the ledger's currently active sessions (before this apply):
/// keys/entries they recorded are session-owned, so a value this plan merges
/// that already exists is recorded as a shared claim and refcounted at
/// teardown — while a pre-existing value no session recorded is user-owned
/// and never recorded (regressions A3-R2-3 / AP4-R2-01).
pub fn execute_apply(
    plan: &Plan,
    ctx: &PlanContext,
    session_id: &str,
    force: bool,
    active: &BTreeMap<String, Session>,
) -> Result<ApplyOutcome, Error> {
    let preps = precheck(plan, ctx, force)?;

    let backup_root = backup_dir(&ctx.workspace_root, session_id);
    let mut executed: Vec<Executed> = Vec::new();
    let mut unchanged = Vec::new();
    let mut failure: Option<Error> = None;

    for (action, prep) in plan.actions.iter().zip(&preps) {
        if prep.skip {
            unchanged.push(action.path().to_string());
            continue;
        }
        let abs = ctx.resolve(action.path());
        // Defense in depth: the denylist is re-checked at the moment of
        // mutation, below adapters and below the CLI's plan check.
        if denylist::is_never_touch(&abs, &ctx.home) {
            failure = Some(Error::NeverTouch { path: abs });
            break;
        }
        let prior = merge_records_for_path(active, None, action.path());
        match execute_one(action, prep, ctx, &abs, &backup_root, &prior) {
            Ok(ex) => executed.push(ex),
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }

    if let Some(err) = failure {
        rollback(&executed);
        let _ = std::fs::remove_dir_all(&backup_root);
        return Err(err);
    }
    Ok(ApplyOutcome {
        records: executed.into_iter().map(|e| e.record).collect(),
        unchanged_paths: unchanged,
    })
}

/// Pre-apply drift + secret gates (design §5.1, §5.2, §5.5). Nothing mutates
/// here; with `force` the refusals convert into per-action `forced` flags.
fn precheck(plan: &Plan, ctx: &PlanContext, force: bool) -> Result<Vec<Prep>, Error> {
    let mut drifts: Vec<String> = Vec::new();
    let mut preps = Vec::new();
    for action in &plan.actions {
        let abs = ctx.resolve(action.path());
        let mut prep = Prep {
            effective: None,
            skip: false,
            needs_force: false,
        };
        match action {
            Action::Create { .. } => {
                if let Some(existing) = read_opt(&abs)?
                    && !has_provenance(&existing)
                {
                    prep.needs_force = true;
                    if !force {
                        drifts.push(format!(
                            "{} exists and was not generated by agent-profile\n  (no provenance \
                             header). Refusing to overwrite.",
                            action.path()
                        ));
                    }
                }
            }
            Action::MergeKeys { keys, content, .. } => {
                // §5.5: resolved secrets may only land in gitignored files;
                // only Claude `.mcp.json` env maps materialize.
                let effective = match resolve_mcp_env(content, &abs, &ctx.repo_root) {
                    Ok(resolved) => resolved,
                    Err(vars) => {
                        return Err(Error::SecretNotIgnored {
                            path: action.path().to_string(),
                            vars,
                        });
                    }
                };
                let current = read_opt(&abs)?;
                let cur = current.as_deref().unwrap_or("");
                let merged = if is_toml(action.path()) {
                    codex::merge_toml_fragment(cur, &effective, action.path())
                } else {
                    claude::merge_json_fragment(cur, &effective, keys, action.path())
                };
                match merged {
                    Ok(_) => {}
                    Err(Error::Drift(msg)) => {
                        prep.needs_force = true;
                        if !force {
                            drifts.push(msg);
                        }
                    }
                    Err(e) => return Err(e),
                }
                prep.effective = Some(effective);
            }
            Action::Symlink { link_target, .. } => {
                let want = ctx.resolve(link_target);
                match std::fs::symlink_metadata(&abs) {
                    Ok(meta)
                        if meta.is_symlink() && std::fs::read_link(&abs).ok() == Some(want) =>
                    {
                        // Already points where we'd point it — foreign link,
                        // leave it alone and never tear it down.
                        prep.skip = true;
                    }
                    Ok(_) => {
                        prep.needs_force = true;
                        if !force {
                            drifts.push(format!(
                                "{} exists and is not an agent-profile symlink to {link_target} \
                                 (not created by agent-profile)",
                                action.path()
                            ));
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::NotFound => {}
                    Err(source) => return Err(Error::Io { path: abs, source }),
                }
            }
            Action::AppendBlock { .. } => {} // idempotent by marker
        }
        preps.push(prep);
    }
    if !drifts.is_empty() {
        return Err(Error::ApplyDrift(drifts));
    }
    Ok(preps)
}

struct Executed {
    record: ActionRecord,
    abs: PathBuf,
    backup_abs: Option<PathBuf>,
    created_dirs: Vec<PathBuf>,
}

/// The merge-keys records other active sessions hold for `path`, excluding
/// `skip_id`'s own. This is the cross-session ownership index used at apply
/// (shared-claim detection), at teardown (refcounting), and by diff
/// (session-owned key recognition, A3-R3-1).
pub fn merge_records_for_path<'a>(
    sessions: &'a BTreeMap<String, Session>,
    skip_id: Option<&str>,
    path: &str,
) -> Vec<&'a ActionRecord> {
    sessions
        .iter()
        .filter(|(id, _)| skip_id != Some(id.as_str()))
        .flat_map(|(_, s)| s.actions.iter())
        .filter(|r| r.op == "merge-keys" && r.path == path)
        .collect()
}

/// Is `key` recorded (owned or claimed) by any of `records`?
fn key_claimed(records: &[&ActionRecord], key: &str) -> bool {
    records.iter().any(|r| {
        r.keys
            .as_ref()
            .is_some_and(|ks| ks.iter().any(|k| k == key))
    })
}

/// Is array entry `item` at `base` recorded as appended/claimed by any of
/// `records`?
fn entry_claimed(records: &[&ActionRecord], base: &str, item: &serde_json::Value) -> bool {
    records.iter().any(|r| {
        r.appended
            .as_ref()
            .and_then(|m| m.get(base))
            .is_some_and(|v| v.contains(item))
    })
}

/// Does `current` already carry exactly the value our fragment sets at `key`?
fn pre_existing_identical(current: &str, effective: &str, key: &str, path: &str) -> bool {
    if is_toml(path) {
        let (Ok(cur), Ok(frag)) = (current.parse::<DocMut>(), effective.parse::<DocMut>()) else {
            return false;
        };
        match (
            toml_item_at(cur.as_item(), key),
            toml_item_at(frag.as_item(), key),
        ) {
            (Some(c), Some(f)) => c.to_string().trim() == f.to_string().trim(),
            _ => false,
        }
    } else {
        let (Ok(cur), Ok(frag)) = (
            serde_json::from_str::<serde_json::Value>(current),
            serde_json::from_str::<serde_json::Value>(effective),
        ) else {
            return false;
        };
        match (json_at(&cur, key), json_at(&frag, key)) {
            (Some(c), Some(f)) => c == f,
            _ => false,
        }
    }
}

/// The non-append keys this session records at `path` (design §3.2.2 "keys we
/// add"; regression A3-R2-3): a key whose identical value already existed is
/// NOT ours — unless another active session recorded it, in which case we
/// record a shared claim so teardown refcounts it (the last session out
/// removes the key; a user-owned pre-existing key is never recorded and never
/// removed). `…[+N]` keys always stay — their entries are tracked
/// individually via `appended`.
fn owned_keys(
    current: &str,
    effective: &str,
    keys: &[String],
    path: &str,
    prior: &[&ActionRecord],
) -> Vec<String> {
    keys.iter()
        .filter(|key| {
            key.contains("[+")
                || !pre_existing_identical(current, effective, key, path)
                || key_claimed(prior, key)
        })
        .cloned()
        .collect()
}

fn execute_one(
    action: &Action,
    prep: &Prep,
    ctx: &PlanContext,
    abs: &Path,
    backup_root: &Path,
    prior: &[&ActionRecord],
) -> Result<Executed, Error> {
    let created_dirs = missing_ancestors(abs);
    let mut record = ActionRecord::new(action.op(), action.path());
    if !created_dirs.is_empty() {
        record.created_dirs = Some(created_dirs.iter().map(|d| display_of(ctx, d)).collect());
    }
    if prep.needs_force {
        record.forced = Some(true);
    }
    let mut backup_abs = None;

    // Back up whatever exists at the target before touching it.
    let current = match action {
        Action::Symlink { .. } => None,
        _ => read_opt(abs)?,
    };
    if let Some(cur) = &current {
        record.hash_before = Some(sha256_of(cur));
        let rel = match action.path().strip_prefix("~/") {
            Some(rest) => format!("home/{rest}"),
            None => action.path().to_string(),
        };
        let dest = backup_root.join(rel);
        atomic_write(&dest, cur)?;
        record.backup = Some(display_of(ctx, &dest));
        backup_abs = Some(dest);
    }

    match action {
        Action::Create { content, .. } => {
            atomic_write(abs, content)?;
            record.hash_after = Some(sha256_of(content));
        }
        Action::MergeKeys { keys, content, .. } => {
            let effective = prep.effective.as_deref().expect("set in precheck");
            let cur = current.as_deref().unwrap_or("");
            let merged = if is_toml(action.path()) {
                codex::merge_toml_fragment_with(cur, effective, action.path(), prep.needs_force)?
            } else {
                claude::merge_json_fragment_with(
                    cur,
                    effective,
                    keys,
                    action.path(),
                    prep.needs_force,
                )?
            };
            if !is_toml(action.path()) {
                record.appended = compute_appended(current.as_deref(), effective, keys, prior);
            }
            atomic_write(abs, &merged)?;
            record.keys = Some(owned_keys(
                current.as_deref().unwrap_or(""),
                effective,
                keys,
                action.path(),
                prior,
            ));
            record.hash_after = Some(sha256_of(&merged));
            // The file pre-existed for *us*, but if a still-active session
            // brought it into being (no hashBefore on its record, or an
            // inherited flag), the file is tool-created: propagate that so
            // whichever session leaves last can delete the emptied skeleton
            // (FIFO/--all teardown leaves no `{}`-shaped residue).
            if record.hash_before.is_some()
                && prior
                    .iter()
                    .any(|r| r.hash_before.is_none() || r.file_created == Some(true))
            {
                record.file_created = Some(true);
            }
            // §5.5 secret hygiene: persist the UNRESOLVED fragment (with the
            // `${env:VAR}` reference intact), never the materialized secret —
            // state.json is not guaranteed gitignored. The on-disk file (which
            // does carry the resolved value) is gitignore-enforced in precheck.
            record.content = Some(content.clone());
        }
        Action::Symlink { link_target, .. } => {
            // Only reached when the path is free or --force cleared it.
            match std::fs::symlink_metadata(abs) {
                Ok(_) => std::fs::remove_file(abs).map_err(io_err(abs))?,
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(Error::Io {
                        path: abs.to_path_buf(),
                        source,
                    });
                }
            }
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent).map_err(io_err(parent))?;
            }
            let want = ctx.resolve(link_target);
            #[cfg(unix)]
            std::os::unix::fs::symlink(&want, abs).map_err(io_err(abs))?;
            #[cfg(not(unix))]
            return Err(Error::NotImplemented);
            record.link_target = Some(link_target.clone());
        }
        Action::AppendBlock {
            marker, content, ..
        } => {
            let new = crate::adapters::desired_file_state(action, current.as_deref())?;
            atomic_write(abs, &new)?;
            record.marker = Some(marker.clone());
            record.hash_after = Some(sha256_of(&new));
            record.content = Some(content.clone());
        }
    }

    Ok(Executed {
        record,
        abs: abs.to_path_buf(),
        backup_abs,
        created_dirs,
    })
}

/// Undo already-executed actions of a failed apply, newest first.
fn rollback(executed: &[Executed]) {
    for ex in executed.iter().rev() {
        match (ex.record.op.as_str(), &ex.backup_abs) {
            ("symlink", _) => {
                let _ = std::fs::remove_file(&ex.abs);
            }
            (_, Some(backup)) => {
                if let Ok(bytes) = std::fs::read_to_string(backup) {
                    let _ = atomic_write(&ex.abs, &bytes);
                }
            }
            (_, None) => {
                let _ = std::fs::remove_file(&ex.abs);
            }
        }
        for dir in ex.created_dirs.iter().rev() {
            let _ = std::fs::remove_dir(dir); // only succeeds when empty
        }
    }
}

/// Which array entries a JSON merge actually appends per `…[+N]` key, plus —
/// for entries already present — a shared claim when another active session
/// appended that entry (cross-session refcount, regression AP4-R2-01). An
/// entry that pre-existed and is claimed by no session is user-owned and not
/// recorded, so teardown never removes it.
fn compute_appended(
    current: Option<&str>,
    effective: &str,
    keys: &[String],
    prior: &[&ActionRecord],
) -> Option<BTreeMap<String, Vec<serde_json::Value>>> {
    let arr_paths: Vec<String> = keys
        .iter()
        .filter_map(|k| k.find("[+").map(|i| k[..i].to_string()))
        .collect();
    if arr_paths.is_empty() {
        return None;
    }
    let frag: serde_json::Value = serde_json::from_str(effective).ok()?;
    let cur: serde_json::Value = current
        .and_then(|c| serde_json::from_str(c).ok())
        .unwrap_or(serde_json::json!({}));
    let mut map = BTreeMap::new();
    for base in arr_paths {
        let Some(frag_arr) = json_at(&frag, &base).and_then(|v| v.as_array()) else {
            continue;
        };
        let empty = Vec::new();
        let cur_arr = json_at(&cur, &base)
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);
        let added: Vec<serde_json::Value> = frag_arr
            .iter()
            .filter(|item| !cur_arr.contains(item) || entry_claimed(prior, &base, item))
            .cloned()
            .collect();
        map.insert(base, added);
    }
    Some(map)
}

fn json_at<'a>(v: &'a serde_json::Value, dotted: &str) -> Option<&'a serde_json::Value> {
    let mut cur = v;
    for seg in dotted.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

// ---------------------------------------------------------------- teardown executor

#[derive(Debug, Serialize)]
pub struct TeardownStep {
    pub op: String,
    pub path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// Replay a session's actions in reverse (design §3.4): hash-verified delete
/// for owned files, key-level reversal for merges (foreign edits preserved),
/// target-verified unlink, marker-scoped block removal. Post-apply drift
/// refuses without `force`; `force` falls back to backup restore (owned) /
/// best-effort key-level (merges). Deletes the session's backups at the end.
/// `active` is the full ledger: keys/entries another still-active session
/// recorded for the same path are preserved (cross-session refcount,
/// regression AP4-R2-01) — the last session out removes them.
pub fn execute_teardown(
    session: &Session,
    session_id: &str,
    ctx: &PlanContext,
    force: bool,
    active: &BTreeMap<String, Session>,
) -> Result<Vec<TeardownStep>, Error> {
    if !force {
        let drifted = verify_session(session, ctx)?;
        if !drifted.is_empty() {
            return Err(Error::TeardownDrift(drifted));
        }
    }

    let mut steps = Vec::new();
    for record in session.actions.iter().rev() {
        let abs = ctx.resolve(&record.path);
        // Defense in depth, same as apply.
        if denylist::is_never_touch(&abs, &ctx.home) {
            return Err(Error::NeverTouch { path: abs });
        }
        let claims = merge_records_for_path(active, Some(session_id), &record.path);
        if let Some(step) = teardown_one(record, ctx, &abs, force, &claims)? {
            steps.push(step);
        }
        if let Some(dirs) = &record.created_dirs {
            for d in dirs.iter().rev() {
                let _ = std::fs::remove_dir(ctx.resolve(d)); // only when empty
            }
        }
    }

    let _ = std::fs::remove_dir_all(backup_dir(&ctx.workspace_root, session_id));
    Ok(steps)
}

/// Post-apply drift check (§5.2): create/symlink verify strictly
/// (hash / link target); merge-keys fall back to "are the keys we wrote still
/// intact" so foreign edits never block key-level reversal; append-block is
/// marker-scoped and tolerant by design ("wherever it now sits").
fn verify_session(session: &Session, ctx: &PlanContext) -> Result<Vec<String>, Error> {
    let mut drifted = Vec::new();
    for record in &session.actions {
        let abs = ctx.resolve(&record.path);
        let ok = match record.op.as_str() {
            "create" => match read_opt(&abs)? {
                None => true, // already gone — nothing dangerous to do
                Some(cur) => Some(sha256_of(&cur)) == record.hash_after,
            },
            "merge-keys" => match read_opt(&abs)? {
                None => true,
                Some(cur) => {
                    Some(sha256_of(&cur)) == record.hash_after || merge_keys_intact(&cur, record)
                }
            },
            "symlink" => match std::fs::symlink_metadata(&abs) {
                Err(e) if e.kind() == ErrorKind::NotFound => true,
                Err(_) => false,
                Ok(meta) => {
                    meta.is_symlink()
                        && record
                            .link_target
                            .as_ref()
                            .is_some_and(|t| std::fs::read_link(&abs).ok() == Some(ctx.resolve(t)))
                }
            },
            "append-block" => true,
            _ => true,
        };
        if !ok {
            drifted.push(record.path.clone());
        }
    }
    Ok(drifted)
}

/// Are the values this merge wrote still what we wrote? Keys removed by the
/// user are fine (reversal becomes a no-op); keys *changed* are drift.
/// For `.mcp.json` the recorded fragment keeps `${env:VAR}` references while
/// the on-disk file carries the materialized value (§5.5) — compare like with
/// like by resolving set references before the comparison.
fn merge_keys_intact(current: &str, record: &ActionRecord) -> bool {
    let (Some(keys), Some(fragment)) = (&record.keys, &record.content) else {
        return false;
    };
    let resolved_fragment;
    let fragment: &str = if Path::new(&record.path)
        .file_name()
        .is_some_and(|n| n == ".mcp.json")
    {
        resolved_fragment = substitute_set_env(fragment);
        &resolved_fragment
    } else {
        fragment
    };
    if is_toml(&record.path) {
        let (Ok(cur), Ok(frag)) = (
            current.parse::<toml_edit::DocumentMut>(),
            fragment.parse::<toml_edit::DocumentMut>(),
        ) else {
            return false;
        };
        keys.iter().all(|key| {
            match (
                toml_item_at(cur.as_item(), key),
                toml_item_at(frag.as_item(), key),
            ) {
                (None, _) => true, // user removed it — nothing left to revert
                (Some(c), Some(f)) => c.to_string().trim() == f.to_string().trim(),
                (Some(_), None) => false,
            }
        })
    } else {
        let (Ok(cur), Ok(frag)) = (
            serde_json::from_str::<serde_json::Value>(current),
            serde_json::from_str::<serde_json::Value>(fragment),
        ) else {
            return false;
        };
        keys.iter().all(|key| {
            if key.contains("[+") {
                // Appended entries are either still present (we remove them)
                // or already gone (no-op) — both fine.
                return true;
            }
            match (json_at(&cur, key), json_at(&frag, key)) {
                (None, _) => true,
                (Some(c), Some(f)) => c == f,
                (Some(_), None) => false,
            }
        })
    }
}

fn toml_item_at<'a>(item: &'a toml_edit::Item, dotted: &str) -> Option<&'a toml_edit::Item> {
    let mut cur = item;
    for seg in dotted.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

fn teardown_one(
    record: &ActionRecord,
    ctx: &PlanContext,
    abs: &Path,
    force: bool,
    claims: &[&ActionRecord],
) -> Result<Option<TeardownStep>, Error> {
    let backup_abs = record.backup.as_ref().map(|b| ctx.resolve(b));
    let step = |op: &str, detail: String| {
        Some(TeardownStep {
            op: op.to_string(),
            path: record.path.clone(),
            detail,
        })
    };
    match record.op.as_str() {
        "create" => {
            if read_opt(abs)?.is_none() {
                return Ok(None);
            }
            // A forced apply backed the foreign original up — restore it;
            // otherwise the file is ours: delete.
            match backup_abs.as_deref().map(read_opt).transpose()?.flatten() {
                Some(original) => {
                    atomic_write(abs, &original)?;
                    Ok(step("restore", "original file restored from backup".into()))
                }
                None => {
                    std::fs::remove_file(abs).map_err(io_err(abs))?;
                    Ok(step("delete", String::new()))
                }
            }
        }
        "merge-keys" => {
            let Some(cur) = read_opt(abs)? else {
                return Ok(None);
            };
            let keys = record.keys.clone().unwrap_or_default();
            let backup_text = backup_abs.as_deref().map(read_opt).transpose()?.flatten();
            let removed = remove_recorded_keys(&cur, record, backup_text.as_deref(), claims);
            let result = match removed {
                Ok(r) => r,
                // Best-effort under --force: fall back to full backup restore.
                Err(_) if force => match &backup_text {
                    Some(b) => b.clone(),
                    None => String::new(),
                },
                Err(e) => return Err(e),
            };
            // Delete (rather than rewrite) when the file owes its existence
            // to this tool — this apply created it (no hashBefore) or a
            // creating session propagated `fileCreated` (teardown-order
            // independence) — and nothing but empty skeleton containers
            // remains. A user-pre-existing file is never deleted, even when
            // key removal empties it.
            let tool_created = record.hash_before.is_none() || record.file_created == Some(true);
            if tool_created && semantically_empty(&result, &record.path) {
                std::fs::remove_file(abs).map_err(io_err(abs))?;
            } else if let Some(b) = &backup_text
                && semantically_equal(&result, b, &record.path)
            {
                atomic_write(abs, b)?; // byte-identical restore when clean
            } else {
                atomic_write(abs, &result)?;
            }
            Ok(step("restore", keys_detail(&keys)))
        }
        "symlink" => {
            let want = record.link_target.as_ref().map(|t| ctx.resolve(t));
            match std::fs::read_link(abs) {
                Ok(target) if want.as_ref() == Some(&target) => {
                    std::fs::remove_file(abs).map_err(io_err(abs))?;
                    Ok(step("unlink", String::new()))
                }
                // Removed already, or repointed by someone else (only
                // reachable under --force): leave it.
                _ => Ok(None),
            }
        }
        "append-block" => {
            let Some(cur) = read_opt(abs)? else {
                return Ok(None);
            };
            let marker = record.marker.clone().unwrap_or_default();
            let Some(new) = remove_marked_block(&cur, &marker) else {
                return Ok(None); // block already gone
            };
            // Remove the file when our block was all that is left AND the
            // file owes its existence to this tool: apply created it
            // (no hashBefore), or the pre-apply content was nothing but
            // agent-profile marked blocks from other sessions since torn
            // down (regression A3-R3-3 — no 0-byte residue; §3.2 "teardown
            // restores the exact pre-apply state").
            let tool_created = record.hash_before.is_none()
                || backup_abs
                    .as_deref()
                    .map(read_opt)
                    .transpose()?
                    .flatten()
                    .is_some_and(|b| only_marked_blocks(&b));
            if tool_created && new.trim().is_empty() {
                std::fs::remove_file(abs).map_err(io_err(abs))?;
            } else {
                atomic_write(abs, &new)?;
            }
            Ok(step("remove", format!("block {marker}")))
        }
        other => Err(Error::Session(format!(
            "state.json action has unknown op `{other}` — written by a newer agent-profile?"
        ))),
    }
}

/// Is the document semantically empty — nothing left but empty skeleton
/// containers (`{}`-valued objects / empty arrays in JSON, empty tables in
/// TOML)? Key-level reversal cannot always prune containers that pre-existed
/// *this* session's apply (an earlier session introduced them), so a
/// tool-created file emptied of every value is deletable residue (§3.4).
fn semantically_empty(text: &str, path: &str) -> bool {
    if is_toml(path) {
        text.trim().is_empty()
            || text
                .parse::<DocMut>()
                .is_ok_and(|doc| toml_skeleton_only(doc.as_item()))
    } else {
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(v) => json_skeleton_only(&v),
            Err(_) => text.trim().is_empty(),
        }
    }
}

fn json_skeleton_only(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(map) => map.values().all(json_skeleton_only),
        serde_json::Value::Array(arr) => arr.is_empty(),
        _ => false,
    }
}

fn toml_skeleton_only(item: &toml_edit::Item) -> bool {
    match item.as_table_like() {
        Some(t) => t.iter().all(|(_, v)| toml_skeleton_only(v)),
        None => matches!(item, toml_edit::Item::None),
    }
}

fn semantically_equal(a: &str, b: &str, path: &str) -> bool {
    if is_toml(path) {
        match (
            a.parse::<toml_edit::DocumentMut>(),
            b.parse::<toml_edit::DocumentMut>(),
        ) {
            (Ok(da), Ok(db)) => da.to_string().trim() == db.to_string().trim(),
            _ => a == b,
        }
    } else {
        match (
            serde_json::from_str::<serde_json::Value>(a),
            serde_json::from_str::<serde_json::Value>(b),
        ) {
            (Ok(va), Ok(vb)) => va == vb,
            _ => a == b,
        }
    }
}

/// Remove exactly the recorded keys (design §3.4): leaf keys removed,
/// `…[+N]` paths lose exactly the entries we appended, parents we introduced
/// are pruned when empty — everything else (foreign keys, later foreign
/// additions) is preserved. Keys/entries another still-active session also
/// recorded (`claims`) are left in place: the last session out removes them
/// (regression AP4-R2-01).
fn remove_recorded_keys(
    current: &str,
    record: &ActionRecord,
    backup: Option<&str>,
    claims: &[&ActionRecord],
) -> Result<String, Error> {
    let keys = record.keys.as_deref().unwrap_or(&[]);
    if is_toml(&record.path) {
        let mut doc: DocMut =
            current
                .parse()
                .map_err(|e: toml_edit::TomlError| Error::TomlParse {
                    path: record.path.clone().into(),
                    message: e.to_string(),
                })?;
        let pre: DocMut = backup.unwrap_or("").parse().unwrap_or_default();
        for key in keys {
            if key_claimed(claims, key) {
                continue; // another active session still needs it
            }
            // A key that pre-existed with *different* content was a foreign
            // value a forced apply overwrote (§5.1: --force still backs up) —
            // restore the user's original instead of deleting it
            // (regression AREA2-R3-01). A pre-existing identical value is a
            // cross-session claim (A3-R2-3/AP4-R2-01): the last session out
            // removes it.
            let pre_item = toml_item_at(pre.as_item(), key).cloned();
            let cur_differs = match (&pre_item, toml_item_at(doc.as_item(), key)) {
                (Some(p), Some(c)) => p.to_string().trim() != c.to_string().trim(),
                (Some(_), None) => true,
                _ => false,
            };
            match pre_item {
                Some(orig) if cur_differs => toml_set_at(&mut doc, key, orig),
                _ => toml_remove_at(&mut doc, key, &pre),
            }
        }
        Ok(doc.to_string())
    } else {
        let mut v: serde_json::Value =
            serde_json::from_str(current).map_err(|e| Error::JsonParse {
                path: record.path.clone().into(),
                message: e.to_string(),
            })?;
        let pre: serde_json::Value = backup
            .and_then(|b| serde_json::from_str(b).ok())
            .unwrap_or(serde_json::json!({}));
        let empty_appended = BTreeMap::new();
        let appended = record.appended.as_ref().unwrap_or(&empty_appended);
        for key in keys {
            if let Some(i) = key.find("[+") {
                let base = &key[..i];
                if let Some(arr) = json_at_mut(&mut v, base).and_then(|x| x.as_array_mut())
                    && let Some(added) = appended.get(base)
                {
                    for item in added {
                        if entry_claimed(claims, base, item) {
                            continue; // refcounted: still required elsewhere
                        }
                        if let Some(pos) = arr.iter().position(|x| x == item) {
                            arr.remove(pos);
                        }
                    }
                }
                // Prune an array we introduced once it is empty again.
                if json_at(&pre, base).is_none()
                    && json_at(&v, base)
                        .and_then(|a| a.as_array())
                        .is_some_and(Vec::is_empty)
                {
                    json_remove_at(&mut v, base);
                }
            } else {
                if key_claimed(claims, key) {
                    continue; // another active session still needs it
                }
                // Same forced-overwrite restore as the TOML branch
                // (regression AREA2-R3-01): a backed-up foreign value comes
                // back; only values we introduced are removed.
                match json_at(&pre, key).cloned() {
                    Some(orig) if json_at(&v, key) != Some(&orig) => {
                        json_set_at(&mut v, key, orig);
                    }
                    _ => json_remove_at(&mut v, key),
                }
            }
        }
        prune_empty_objects(&mut v, &pre);
        let mut s = serde_json::to_string_pretty(&v).expect("JSON value serializes");
        s.push('\n');
        Ok(s)
    }
}

type DocMut = toml_edit::DocumentMut;

fn toml_remove_at(doc: &mut DocMut, dotted: &str, pre: &DocMut) {
    let segs: Vec<&str> = dotted.split('.').collect();
    fn remove_in(item: &mut toml_edit::Item, segs: &[&str]) -> bool {
        if segs.len() == 1 {
            if let Some(t) = item.as_table_like_mut() {
                t.remove(segs[0]);
                return true;
            }
            return false;
        }
        match item.get_mut(segs[0]) {
            Some(child) => remove_in(child, &segs[1..]),
            None => false,
        }
    }
    remove_in(doc.as_item_mut(), &segs);
    // Prune now-empty parent tables we introduced (absent from the pre-state).
    for depth in (1..segs.len()).rev() {
        let parent = segs[..depth].join(".");
        let empty = toml_item_at(doc.as_item(), &parent)
            .and_then(|i| i.as_table_like())
            .is_some_and(|t| t.is_empty());
        if empty && toml_item_at(pre.as_item(), &parent).is_none() {
            let parent_segs: Vec<&str> = parent.split('.').collect();
            remove_in(doc.as_item_mut(), &parent_segs);
        }
    }
}

/// Set `dotted` in `doc` to `value`, creating implicit parent tables as
/// needed (teardown restore of a force-overwritten key, AREA2-R3-01).
fn toml_set_at(doc: &mut DocMut, dotted: &str, value: toml_edit::Item) {
    fn set_in(item: &mut toml_edit::Item, segs: &[&str], value: toml_edit::Item) {
        let Some(t) = item.as_table_like_mut() else {
            return;
        };
        if segs.len() == 1 {
            t.insert(segs[0], value);
            return;
        }
        if t.get(segs[0]).is_none() {
            let mut tbl = toml_edit::Table::new();
            tbl.set_implicit(true);
            t.insert(segs[0], toml_edit::Item::Table(tbl));
        }
        set_in(
            t.get_mut(segs[0]).expect("just ensured present"),
            &segs[1..],
            value,
        );
    }
    let segs: Vec<&str> = dotted.split('.').collect();
    set_in(doc.as_item_mut(), &segs, value);
}

/// Set `dotted` in `v` to `value`, creating parent objects as needed
/// (teardown restore of a force-overwritten key, AREA2-R3-01).
fn json_set_at(v: &mut serde_json::Value, dotted: &str, value: serde_json::Value) {
    let segs: Vec<&str> = dotted.split('.').collect();
    let Some((leaf, parents)) = segs.split_last() else {
        return;
    };
    let mut cur = v;
    for seg in parents {
        let Some(obj) = cur.as_object_mut() else {
            return;
        };
        cur = obj
            .entry((*seg).to_string())
            .or_insert_with(|| serde_json::json!({}));
    }
    if let Some(obj) = cur.as_object_mut() {
        obj.insert((*leaf).to_string(), value);
    }
}

fn json_at_mut<'a>(
    v: &'a mut serde_json::Value,
    dotted: &str,
) -> Option<&'a mut serde_json::Value> {
    let mut cur = v;
    for seg in dotted.split('.') {
        cur = cur.get_mut(seg)?;
    }
    Some(cur)
}

fn json_remove_at(v: &mut serde_json::Value, dotted: &str) {
    let segs: Vec<&str> = dotted.split('.').collect();
    let Some((leaf, parents)) = segs.split_last() else {
        return;
    };
    let mut cur = v;
    for seg in parents {
        match cur.get_mut(seg) {
            Some(next) => cur = next,
            None => return,
        }
    }
    if let Some(obj) = cur.as_object_mut() {
        obj.remove(*leaf);
    }
}

/// Drop object keys that are now empty objects and did not exist before we
/// merged (we introduced them as containers for our keys).
fn prune_empty_objects(v: &mut serde_json::Value, pre: &serde_json::Value) {
    let Some(obj) = v.as_object_mut() else { return };
    let mut to_remove = Vec::new();
    for (key, child) in obj.iter_mut() {
        if child.is_object() {
            let pre_child = pre.get(key).cloned().unwrap_or(serde_json::Value::Null);
            prune_empty_objects(child, &pre_child);
            if child.as_object().is_some_and(|o| o.is_empty()) && pre.get(key).is_none() {
                to_remove.push(key.clone());
            }
        }
    }
    for key in to_remove {
        obj.remove(&key);
    }
}

/// Is `text` (a pre-apply backup) made up entirely of agent-profile marked
/// blocks (any marker) and blank lines? Such a file exists only because of
/// this tool's own applies, so teardown may remove it once emptied
/// (regression A3-R3-3).
fn only_marked_blocks(text: &str) -> bool {
    let mut saw_block = false;
    let mut inside = false;
    for line in text.lines() {
        let t = line.trim_start();
        if !inside && t.starts_with("<!-- agent-profile:begin ") {
            inside = true;
            saw_block = true;
            continue;
        }
        if inside {
            if t.starts_with("<!-- agent-profile:end ") {
                inside = false;
            }
            continue;
        }
        if !t.trim_end().is_empty() {
            return false;
        }
    }
    saw_block && !inside
}

/// Remove the `<!-- agent-profile:begin <marker> … end <marker> -->` block,
/// wherever it now sits. `None` = marker not found.
pub fn remove_marked_block(text: &str, marker: &str) -> Option<String> {
    let begin = format!("<!-- agent-profile:begin {marker} ");
    let begin_bare = format!("<!-- agent-profile:begin {marker} -->");
    let end = format!("<!-- agent-profile:end {marker} -->");
    let mut out = String::new();
    let mut inside = false;
    let mut found = false;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        if !inside && (t.starts_with(&begin) || t.starts_with(&begin_bare)) {
            inside = true;
            found = true;
            continue;
        }
        if inside {
            if t.starts_with(&end) {
                inside = false;
            }
            continue;
        }
        out.push_str(line);
    }
    found.then_some(out)
}

/// §4.4 step details: `removed key mcpServers.github-readonly`,
/// `removed 2 allow rules, defaultMode`.
fn keys_detail(keys: &[String]) -> String {
    if keys.is_empty() {
        return String::new();
    }
    let mut plain = Vec::new();
    let mut parts = Vec::new();
    for k in keys {
        if let Some(i) = k.find("[+") {
            let n: usize = k[i + 2..].trim_end_matches(']').parse().unwrap_or(0);
            let label = k[..i].rsplit('.').next().unwrap_or("");
            parts.push(format!("{n} {label} rule{}", if n == 1 { "" } else { "s" }));
        } else if let Some(leaf) = k.strip_prefix("permissions.") {
            parts.push(leaf.to_string());
        } else {
            plain.push(k.as_str());
        }
    }
    if !plain.is_empty() {
        parts.insert(
            0,
            format!(
                "key{} {}",
                if plain.len() == 1 { "" } else { "s" },
                plain.join(", ")
            ),
        );
    }
    format!("removed {}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::Scope;

    /// The design §3.4 state example, field-for-field.
    const DESIGN_3_4: &str = r#"{
  "schemaVersion": 1,
  "sessions": {
    "loop-42-reviewer": {
      "role": "reviewer",
      "target": "claude-teammate",
      "scope": "repo",
      "appliedAt": "2026-06-10T09:14:03Z",
      "agentProfileVersion": "0.1.0",
      "actions": [
        {
          "op": "create",
          "path": ".claude/agents/reviewer.md",
          "hashAfter": "sha256:aaa"
        },
        {
          "op": "merge-keys",
          "path": ".mcp.json",
          "keys": [
            "mcpServers.github-readonly"
          ],
          "backup": ".agent-profile/backups/loop-42-reviewer/.mcp.json",
          "hashBefore": "sha256:bbb",
          "hashAfter": "sha256:ccc"
        },
        {
          "op": "symlink",
          "path": ".claude/skills/code-review",
          "linkTarget": "~/.agents/skills/code-review"
        },
        {
          "op": "append-block",
          "path": "CLAUDE.md",
          "marker": "session=loop-42-reviewer",
          "backup": ".agent-profile/backups/loop-42-reviewer/CLAUDE.md",
          "hashBefore": "sha256:ddd",
          "hashAfter": "sha256:eee"
        }
      ]
    }
  }
}"#;

    /// S4-T1: the ledger round-trips through serde matching §3.4 verbatim —
    /// same fields, same spellings, same order.
    #[test]
    fn state_round_trips_design_3_4_verbatim() {
        let state: State = serde_json::from_str(DESIGN_3_4).unwrap();
        let session = &state.sessions["loop-42-reviewer"];
        assert_eq!(session.role, "reviewer");
        assert_eq!(session.actions.len(), 4);
        assert_eq!(
            session.actions[2].link_target.as_deref(),
            Some("~/.agents/skills/code-review")
        );
        let back = serde_json::to_string_pretty(&state).unwrap();
        assert_eq!(back, DESIGN_3_4);
    }

    #[test]
    fn corrupt_state_is_session_error_with_recovery_hint() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("state.json"), "{ not json").unwrap();
        let err = load(tmp.path()).unwrap_err();
        assert!(matches!(err, Error::Session(_)), "got {err:?}");
        assert_eq!(err.exit_code(), crate::ExitCode::Session);
        assert!(err.to_string().contains("recover"), "{err}");
    }

    #[test]
    fn missing_state_is_empty_ledger() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = load(tmp.path()).unwrap();
        assert!(state.sessions.is_empty());
        assert_eq!(state.schema_version, STATE_SCHEMA_VERSION);
    }

    /// S4-T1: state.json itself is written atomically — saving over an
    /// existing ledger leaves no temp/torn files behind.
    #[test]
    fn save_is_atomic_and_leaves_no_torn_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut state = State::default();
        save(tmp.path(), &state).unwrap();
        state.sessions.insert(
            "s1".into(),
            Session {
                role: "r".into(),
                target: "claude-teammate".into(),
                scope: "repo".into(),
                applied_at: now_rfc3339(),
                agent_profile_version: "0.1.0".into(),
                actions: vec![],
            },
        );
        save(tmp.path(), &state).unwrap();
        let entries: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, ["state.json"], "no stray temp files");
        assert_eq!(load(tmp.path()).unwrap(), state);
    }

    #[test]
    fn rfc3339_round_trips_with_doctor_parser() {
        for secs in [0i64, 1_770_000_843, 1_797_000_000] {
            let s = rfc3339_from_epoch(secs);
            assert_eq!(
                crate::doctor::epoch_secs_from_rfc3339(&s),
                Some(secs),
                "{s}"
            );
        }
        assert_eq!(rfc3339_from_epoch(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn sha256_format_matches_design() {
        assert_eq!(
            sha256_of(""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn env_refs_finds_unique_vars_in_order() {
        assert_eq!(
            env_refs("a ${env:FOO} b ${env:BAR} ${env:FOO}"),
            ["FOO", "BAR"]
        );
        assert!(env_refs("no refs ${env:").is_empty());
    }

    fn ctx_in(tmp: &Path) -> PlanContext {
        PlanContext {
            scope: Scope::Repo,
            repo_root: tmp.to_path_buf(),
            workspace_root: tmp.join(".agent-profile"),
            home: tmp.join("home"),
            session_id: Some("s1".into()),
        }
    }

    /// S4-T2: executor-level denylist — a plan that names a never-touch path
    /// is refused at the moment of mutation, and earlier actions roll back.
    #[test]
    fn executor_denylist_refuses_and_rolls_back() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("home")).unwrap();
        let ctx = ctx_in(tmp.path());
        let plan = Plan {
            actions: vec![
                Action::Create {
                    path: ".claude/agents/x.md".into(),
                    content: "<!-- generated by agent-profile -->\n".into(),
                },
                Action::Create {
                    path: "~/.codex/auth.json".into(),
                    content: "{}".into(),
                },
            ],
            ..Default::default()
        };
        let err = execute_apply(&plan, &ctx, "s1", false, &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, Error::NeverTouch { .. }), "got {err:?}");
        // Rollback: the first create is undone, including its directories.
        assert!(!tmp.path().join(".claude/agents/x.md").exists());
        assert!(!tmp.path().join(".claude").exists(), "created dirs pruned");
        assert!(!ctx.workspace_root.join("backups/s1").exists());
    }

    /// S4-T2: partial failure mid-apply rolls the session's earlier actions
    /// back (here the append-block target is a directory → Io error).
    #[test]
    fn mid_apply_failure_rolls_back_executed_actions() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("home")).unwrap();
        std::fs::create_dir_all(tmp.path().join("CLAUDE.md")).unwrap(); // dir blocks the write
        std::fs::write(tmp.path().join(".mcp.json"), "{\n  \"mcpServers\": {}\n}\n").unwrap();
        let ctx = ctx_in(tmp.path());
        let plan = Plan {
            actions: vec![
                Action::Create {
                    path: ".claude/agents/x.md".into(),
                    content: "<!-- generated by agent-profile -->\n".into(),
                },
                Action::MergeKeys {
                    path: ".mcp.json".into(),
                    keys: vec!["mcpServers.gh".into()],
                    content: "{ \"mcpServers\": { \"gh\": { \"command\": \"gh-mcp\" } } }".into(),
                },
                Action::AppendBlock {
                    path: "CLAUDE.md".into(),
                    marker: "session=s1".into(),
                    content: "<!-- agent-profile:begin session=s1 -->\n".into(),
                },
            ],
            ..Default::default()
        };
        let before = std::fs::read_to_string(tmp.path().join(".mcp.json")).unwrap();
        let err = execute_apply(&plan, &ctx, "s1", false, &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, Error::Io { .. }), "got {err:?}");
        assert!(!tmp.path().join(".claude/agents/x.md").exists());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(".mcp.json")).unwrap(),
            before,
            "merge rolled back from backup"
        );
        assert!(!ctx.workspace_root.join("backups/s1").exists());
    }

    /// A3-R3-1: unresolved-spec matching — `${env:VAR}` wildcards.
    #[test]
    fn env_wildcard_match_treats_refs_as_wildcards() {
        assert!(env_wildcard_match("${env:TOK}", "ghp_anything"));
        assert!(env_wildcard_match("Bearer ${env:TOK}", "Bearer abc123"));
        assert!(!env_wildcard_match("Bearer ${env:TOK}", "Basic abc123"));
        assert!(env_wildcard_match("${env:A}-${env:B}", "x-y"));
        assert!(!env_wildcard_match("plain", "different"));
        assert!(env_wildcard_match("plain", "plain"));
    }

    /// A3-R3-3: residue detection — pre-apply content that is nothing but
    /// agent-profile marked blocks means the file owes its existence to us.
    #[test]
    fn only_marked_blocks_detects_tool_created_content() {
        let block = "<!-- agent-profile:begin session=c1 role=r -->\n\
                     @.agent-profile/fragments/a.md\n\
                     <!-- agent-profile:end session=c1 -->\n";
        assert!(only_marked_blocks(block));
        assert!(only_marked_blocks(&format!("\n{block}\n")));
        assert!(!only_marked_blocks(&format!("# mine\n{block}")));
        assert!(!only_marked_blocks(""));
        assert!(!only_marked_blocks("# just text\n"));
        // Unterminated block — be conservative.
        assert!(!only_marked_blocks(
            "<!-- agent-profile:begin session=c1 -->\nx\n"
        ));
    }

    #[test]
    fn remove_marked_block_handles_moved_blocks() {
        let text = "# Top\n\
                    <!-- agent-profile:begin session=s1 role=r -->\n\
                    @.agent-profile/fragments/x.md\n\
                    <!-- agent-profile:end session=s1 -->\n\
                    # Bottom\n";
        assert_eq!(
            remove_marked_block(text, "session=s1").unwrap(),
            "# Top\n# Bottom\n"
        );
        assert_eq!(remove_marked_block("# none\n", "session=s1"), None);
        // Marker prefixes don't collide.
        assert_eq!(remove_marked_block(text, "session=s"), None);
    }

    #[test]
    fn remove_recorded_keys_preserves_foreign_and_prunes_ours() {
        // We created the mcpServers object; a foreign top-level key was added
        // after apply and must survive key-level reversal.
        let record = ActionRecord {
            keys: Some(vec!["mcpServers.gh".into()]),
            content: Some("{ \"mcpServers\": { \"gh\": { \"command\": \"x\" } } }".into()),
            ..ActionRecord::new("merge-keys", ".mcp.json")
        };
        let current = "{\n  \"mcpServers\": {\n    \"gh\": {\n      \"command\": \"x\"\n    }\n  },\n  \"foreign\": true\n}\n";
        let out = remove_recorded_keys(current, &record, None, &[]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v, serde_json::json!({ "foreign": true }));
    }

    #[test]
    fn remove_recorded_keys_removes_exactly_appended_entries() {
        let record = ActionRecord {
            keys: Some(vec![
                "permissions.allow[+2]".into(),
                "permissions.defaultMode".into(),
            ]),
            content: Some(
                "{ \"permissions\": { \"allow\": [\"Read\", \"Grep\"], \"defaultMode\": \"plan\" } }"
                    .into(),
            ),
            appended: Some(BTreeMap::from([(
                "permissions.allow".to_string(),
                vec![serde_json::json!("Grep")], // `Read` pre-existed
            )])),
            ..ActionRecord::new("merge-keys", ".claude/settings.local.json")
        };
        let backup = "{ \"permissions\": { \"allow\": [\"Read\"] } }";
        let current = "{ \"permissions\": { \"allow\": [\"Read\", \"Grep\", \"Foreign\"], \"defaultMode\": \"plan\" } }";
        let out = remove_recorded_keys(current, &record, Some(backup), &[]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "permissions": { "allow": ["Read", "Foreign"] } })
        );
    }

    #[test]
    fn remove_recorded_keys_toml_keeps_foreign_tables() {
        let record = ActionRecord {
            keys: Some(vec!["mcp_servers.github".into()]),
            content: Some("[mcp_servers.github]\ncommand = \"github-mcp\"\n".into()),
            ..ActionRecord::new("merge-keys", "~/.codex/config.toml")
        };
        let current = "# comment survives\nmodel = \"gpt-5.5\"\n\n[marketplaces.x]\nsource = \"git\"\n\n[mcp_servers.github]\ncommand = \"github-mcp\"\n";
        let backup =
            "# comment survives\nmodel = \"gpt-5.5\"\n\n[marketplaces.x]\nsource = \"git\"\n";
        let out = remove_recorded_keys(current, &record, Some(backup), &[]).unwrap();
        assert_eq!(out.trim(), backup.trim());
    }

    #[test]
    fn keys_detail_matches_design_4_4() {
        assert_eq!(
            keys_detail(&["mcpServers.github-readonly".into()]),
            "removed key mcpServers.github-readonly"
        );
        assert_eq!(
            keys_detail(&[
                "permissions.allow[+2]".into(),
                "permissions.defaultMode".into()
            ]),
            "removed 2 allow rules, defaultMode"
        );
    }
}

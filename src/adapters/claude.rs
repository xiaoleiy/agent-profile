//! Claude render targets (design §3.1/§3.2).
//!
//! `claude-subagent` owns a single `.claude/agents/<role>.md`; everything the
//! subagent frontmatter cannot express is surfaced as SKIPPED findings.
//!
//! `claude-teammate` is the spawn-time injection strategy: agent .md (own) +
//! `.mcp.json` / `.claude/settings.local.json` key-level merges + per-skill
//! symlinks into `~/.agents/skills/` + a marked `@include` block in CLAUDE.md.

use std::collections::BTreeMap;

use crate::adapters::{
    Action, Plan, PlanContext, RenderTarget, Scope, Skipped, block_marker, marked_block,
    provenance_header_md,
};
use crate::error::Error;
use crate::schema::merge::ResolvedProfile;
use crate::schema::types::{McpServer, PermissionMode, Target};

pub struct ClaudeSubagent;
pub struct ClaudeTeammate;

impl RenderTarget for ClaudeSubagent {
    fn target(&self) -> Target {
        Target::ClaudeSubagent
    }

    fn plan(&self, r: &ResolvedProfile, ctx: &PlanContext) -> Result<Plan, Error> {
        let role = r.profile.name.as_str();
        let mut skipped = Vec::new();

        let mut content = format!("{}{}\n", frontmatter(r), provenance_header_md(role));
        let mut sections: Vec<String> = Vec::new();
        if let Some(context) = r.set.context.as_ref() {
            for fragment in context {
                sections.push(ensure_trailing_newline(read_fragment(ctx, fragment)?));
            }
        }
        if let Some(skills) = r.set.skills.as_ref().filter(|s| !s.is_empty()) {
            let lines: Vec<String> = skills
                .iter()
                .map(|s| format!("Use the {s} skill (see ~/.agents/skills/{s}/SKILL.md)."))
                .collect();
            sections.push(format!("{}\n", lines.join("\n")));
        }
        if !sections.is_empty() {
            content.push('\n');
            content.push_str(&sections.join("\n"));
        }

        skip_inexpressible_subagent_fields(r, &mut skipped);

        Ok(Plan {
            actions: vec![Action::Create {
                path: agent_md_path(ctx.scope, role),
                content,
            }],
            skipped,
            notes: vec![],
        })
    }
}

impl RenderTarget for ClaudeTeammate {
    fn target(&self) -> Target {
        Target::ClaudeTeammate
    }

    fn plan(&self, r: &ResolvedProfile, ctx: &PlanContext) -> Result<Plan, Error> {
        let role = r.profile.name.as_str();
        let mut actions = Vec::new();
        let mut skipped = Vec::new();
        let mut notes = Vec::new();

        // 1. Agent file (§3.2.1): prompt/model/description/tools line. The
        //    other profile sections route to the surfaces below — the body
        //    carries only the provenance header.
        actions.push(Action::Create {
            path: agent_md_path(ctx.scope, role),
            content: format!("{}{}\n", frontmatter(r), provenance_header_md(role)),
        });

        // 2. MCP servers (§3.2.2): added `mcpServers.<key>` only. The merge
        //    surface is always the repo `.mcp.json` (ownership table §3.0).
        if let Some(servers) = r.set.mcp_servers.as_ref().filter(|s| !s.is_empty()) {
            actions.push(Action::MergeKeys {
                path: ".mcp.json".to_string(),
                keys: servers
                    .keys()
                    .map(|name| format!("mcpServers.{name}"))
                    .collect(),
                content: mcp_json_fragment(servers),
            });
        }

        // 3. Permissions / permissionMode (§3.2.3) into settings.local.json.
        if let Some((keys, content)) = settings_fragment(r) {
            actions.push(Action::MergeKeys {
                path: ".claude/settings.local.json".to_string(),
                keys,
                content,
            });
            notes.push(
                "settings.local.json permissions are project-wide, not per-teammate; the \
                 session scope means \"while this worker session is active in this \
                 repo/worktree\" — give each worker its own worktree for true isolation"
                    .to_string(),
            );
        }

        // 4. Skills (§3.2.4): symlinks into the cross-tool store; never copy.
        if let Some(skills) = r.set.skills.as_ref() {
            for skill in skills {
                actions.push(Action::Symlink {
                    path: format!(".claude/skills/{skill}"),
                    link_target: format!("~/.agents/skills/{skill}"),
                });
            }
        }

        // 5. Context fragments (§3.2.5): marked @include block in CLAUDE.md.
        if let Some(context) = r.set.context.as_ref().filter(|c| !c.is_empty()) {
            let marker = block_marker(ctx, role);
            actions.push(Action::AppendBlock {
                path: "CLAUDE.md".to_string(),
                content: marked_block(&marker, role, context),
                marker,
            });
        }

        if r.set.model.as_ref().is_some_and(|m| m.effort.is_some()) {
            skipped.push(Skipped {
                field: "model.effort".to_string(),
                reason: "claude-teammate has no per-teammate effort control".to_string(),
                hint: Some("set effortLevel in your own settings if needed".to_string()),
            });
        }

        Ok(Plan {
            actions,
            skipped,
            notes,
        })
    }
}

fn agent_md_path(scope: Scope, role: &str) -> String {
    match scope {
        Scope::Repo => format!(".claude/agents/{role}.md"),
        Scope::User => format!("~/.claude/agents/{role}.md"),
    }
}

/// Subagent frontmatter (design §3.1): only fields the CLI honors there.
/// `tools:` is an allowlist line; deny rules are inexpressible (skipped).
fn frontmatter(r: &ResolvedProfile) -> String {
    let mut s = String::from("---\n");
    s.push_str(&format!("name: {}\n", r.profile.name));
    s.push_str(&format!("description: {}\n", r.profile.description));
    if let Some(model) = r.set.model.as_ref().and_then(|m| m.claude.as_ref()) {
        s.push_str(&format!("model: {model}\n"));
    }
    if let Some(allow) = r
        .set
        .tools
        .as_ref()
        .and_then(|t| t.allow.as_ref())
        .filter(|a| !a.is_empty())
    {
        s.push_str(&format!("tools: {}\n", allow.join(", ")));
    }
    s.push_str("---\n");
    s
}

fn skip_inexpressible_subagent_fields(r: &ResolvedProfile, skipped: &mut Vec<Skipped>) {
    if r.set.mcp_servers.as_ref().is_some_and(|s| !s.is_empty()) {
        skipped.push(Skipped {
            field: "mcpServers".to_string(),
            reason: "claude-subagent cannot express per-subagent MCP servers".to_string(),
            hint: Some(
                "use target claude-teammate, or add servers to .mcp.json yourself".to_string(),
            ),
        });
    }
    if r.set.permission_mode.is_some() {
        skipped.push(Skipped {
            field: "permissionMode".to_string(),
            reason: "claude-subagent cannot express `permissionMode` in subagent frontmatter"
                .to_string(),
            hint: Some(
                "use target claude-teammate (rendered via settings.local.json defaultMode)"
                    .to_string(),
            ),
        });
    }
    if let Some(deny) = r
        .set
        .tools
        .as_ref()
        .and_then(|t| t.deny.as_ref())
        .filter(|d| !d.is_empty())
    {
        skipped.push(Skipped {
            field: "tools.deny".to_string(),
            reason: format!(
                "subagent frontmatter expresses only an allowlist; deny rules ({}) cannot be \
                 expressed",
                deny.join(", ")
            ),
            hint: Some(
                "use target claude-teammate (settings.local.json permissions.deny)".to_string(),
            ),
        });
    }
    if r.set.model.as_ref().is_some_and(|m| m.effort.is_some()) {
        skipped.push(Skipped {
            field: "model.effort".to_string(),
            reason: "claude-subagent has no per-subagent effort field".to_string(),
            hint: Some("set effortLevel in your own settings if needed".to_string()),
        });
    }
}

fn read_fragment(ctx: &PlanContext, rel: &str) -> Result<String, Error> {
    let path = ctx.workspace_root.join(rel);
    std::fs::read_to_string(&path).map_err(|source| Error::Io { path, source })
}

fn ensure_trailing_newline(mut s: String) -> String {
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// JSON fragment holding only the `mcpServers.<name>` keys we add — same
/// shapes as the profile (which mirrors Claude's `.mcp.json`).
fn mcp_json_fragment(servers: &BTreeMap<String, McpServer>) -> String {
    let value = serde_json::json!({ "mcpServers": servers });
    pretty(&value)
}

/// `permissions.allow`/`deny` entries (tracked individually as `[+N]`) and
/// `permissions.defaultMode` from `permissionMode` (design §3.2.3).
fn settings_fragment(r: &ResolvedProfile) -> Option<(Vec<String>, String)> {
    let mut keys = Vec::new();
    let mut permissions = serde_json::Map::new();
    if let Some(tools) = &r.set.tools {
        if let Some(allow) = tools.allow.as_ref().filter(|a| !a.is_empty()) {
            keys.push(format!("permissions.allow[+{}]", allow.len()));
            permissions.insert("allow".to_string(), serde_json::json!(allow));
        }
        if let Some(deny) = tools.deny.as_ref().filter(|d| !d.is_empty()) {
            keys.push(format!("permissions.deny[+{}]", deny.len()));
            permissions.insert("deny".to_string(), serde_json::json!(deny));
        }
    }
    if let Some(mode) = r.set.permission_mode {
        keys.push("permissions.defaultMode".to_string());
        permissions.insert(
            "defaultMode".to_string(),
            serde_json::json!(default_mode(mode)),
        );
    }
    if permissions.is_empty() {
        return None;
    }
    let value = serde_json::json!({ "permissions": permissions });
    Some((keys, pretty(&value)))
}

/// `permissionMode` → Claude `permissions.defaultMode`. Claude has no
/// readonly mode; `readonly` maps to `plan` and relies on the merged deny
/// rules for the read-only posture. `auto` is a defaultMode value observed in
/// the wild (recon §1.2).
fn default_mode(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Readonly | PermissionMode::Plan => "plan",
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::Auto => "auto",
        PermissionMode::Unrestricted => "bypassPermissions",
    }
}

fn pretty(value: &serde_json::Value) -> String {
    let mut s = serde_json::to_string_pretty(value).expect("JSON value always serializes");
    s.push('\n');
    s
}

/// Key-level merge of a JSON `fragment` into `current`, preserving every key
/// we don't own (`preserve_order` keeps their positions). Objects recurse;
/// recorded `…[+N]` keys get array-append semantics (entries tracked
/// individually, design §3.2.3); any other existing value must be identical —
/// otherwise it is a collision → drift (design §5.1), reported at the leaf.
pub fn merge_json_fragment(
    current: &str,
    fragment: &str,
    keys: &[String],
    path: &str,
) -> Result<String, Error> {
    merge_json_fragment_with(current, fragment, keys, path, false)
}

/// [`merge_json_fragment`] with collision handling selectable: `force = true`
/// makes our keys win on collision (apply `--force`, design §5.1) instead of
/// returning a drift error.
pub fn merge_json_fragment_with(
    current: &str,
    fragment: &str,
    keys: &[String],
    path: &str,
    force: bool,
) -> Result<String, Error> {
    let parse_err = |message: String| Error::JsonParse {
        path: path.into(),
        message,
    };
    let mut cur: serde_json::Value = if current.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(current).map_err(|e| parse_err(e.to_string()))?
    };
    let frag: serde_json::Value =
        serde_json::from_str(fragment).map_err(|e| parse_err(format!("internal fragment: {e}")))?;

    if !cur.is_object() {
        return Err(parse_err("top level must be a JSON object".to_string()));
    }
    if !frag.is_object() {
        return Err(parse_err(
            "internal fragment top level must be a JSON object".to_string(),
        ));
    }

    // Drive the merge by the recorded keys: each is the *unit of ownership*.
    // A non-append key we add must not already exist with different content —
    // a same-name collision is drift (design §3.2.2/§5.1), even when our value
    // and the existing one differ only by which sub-keys each carries (we do
    // NOT deep-merge into a server/entry someone else created). Append keys
    // (`…[+N]`) get array-append semantics, tracked individually (§3.2.3).
    for key in keys {
        if let Some(i) = key.find("[+") {
            json_append_array(&mut cur, &frag, &key[..i], path, force)?;
        } else {
            json_set_key(&mut cur, &frag, key, path, force)?;
        }
    }
    Ok(pretty(&cur))
}

/// Set `dotted` in `cur` to the value the fragment carries there, creating any
/// missing parent objects. Existing identical value → no-op; existing
/// different value → collision (drift) unless `force`.
fn json_set_key(
    cur: &mut serde_json::Value,
    frag: &serde_json::Value,
    dotted: &str,
    path: &str,
    force: bool,
) -> Result<(), Error> {
    let Some(src_value) = json_get(frag, dotted) else {
        return Ok(());
    };
    let (obj, leaf) = ensure_parent_object(cur, dotted, path)?;
    match obj.get(&leaf) {
        None => {
            obj.insert(leaf, src_value.clone());
        }
        Some(existing) if existing == src_value => {}
        Some(existing) => {
            if force {
                obj.insert(leaf, src_value.clone());
            } else {
                return Err(collision(
                    &first_diff_leaf(existing, src_value, dotted),
                    path,
                ));
            }
        }
    }
    Ok(())
}

/// Append the fragment's array entries at `base` into `cur`, skipping entries
/// already present. Creates the array (and parents) when missing.
fn json_append_array(
    cur: &mut serde_json::Value,
    frag: &serde_json::Value,
    base: &str,
    path: &str,
    force: bool,
) -> Result<(), Error> {
    let Some(src_arr) = json_get(frag, base).and_then(|v| v.as_array()).cloned() else {
        return Ok(());
    };
    let (obj, leaf) = ensure_parent_object(cur, base, path)?;
    match obj.get_mut(&leaf) {
        None => {
            obj.insert(leaf, serde_json::Value::Array(src_arr));
        }
        Some(existing) => match existing.as_array_mut() {
            Some(arr) => {
                for item in src_arr {
                    if !arr.contains(&item) {
                        arr.push(item);
                    }
                }
            }
            None if force => *existing = serde_json::Value::Array(src_arr),
            None => return Err(collision(base, path)),
        },
    }
    Ok(())
}

/// Navigate to the object holding the last segment of `dotted`, creating empty
/// objects for missing parents. Returns the parent map and the leaf key.
fn ensure_parent_object<'a>(
    root: &'a mut serde_json::Value,
    dotted: &str,
    path: &str,
) -> Result<(&'a mut serde_json::Map<String, serde_json::Value>, String), Error> {
    let segs: Vec<&str> = dotted.split('.').collect();
    let (leaf, parents) = segs.split_last().expect("non-empty key");
    let mut cur = root;
    for seg in parents {
        let obj = cur.as_object_mut().ok_or_else(|| collision(dotted, path))?;
        cur = obj
            .entry((*seg).to_string())
            .or_insert_with(|| serde_json::json!({}));
    }
    let obj = cur.as_object_mut().ok_or_else(|| collision(dotted, path))?;
    Ok((obj, (*leaf).to_string()))
}

fn json_get<'a>(v: &'a serde_json::Value, dotted: &str) -> Option<&'a serde_json::Value> {
    let mut cur = v;
    for seg in dotted.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

/// Deepest dotted path at which `cur` and `new` first differ — used so a
/// collision message points at the offending leaf (e.g.
/// `mcpServers.github.command`) when one exists, falling back to the key.
fn first_diff_leaf(cur: &serde_json::Value, new: &serde_json::Value, prefix: &str) -> String {
    if let (Some(co), Some(no)) = (cur.as_object(), new.as_object()) {
        for (k, nv) in no {
            if let Some(cv) = co.get(k)
                && cv != nv
            {
                return first_diff_leaf(cv, nv, &format!("{prefix}.{k}"));
            }
        }
    }
    prefix.to_string()
}

fn collision(full: &str, path: &str) -> Error {
    Error::Drift(format!(
        "merge collision at `{full}` in {path}: key already exists with different content \
         (not created by agent-profile)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::merge::Provenance;
    use crate::schema::types::{
        CapabilitySet, Effort, McpServerType, ModelSpec, Profile, ToolsSpec,
    };
    use std::path::PathBuf;

    /// The design §1.2 reviewer profile, fully resolved.
    fn reviewer() -> ResolvedProfile {
        ResolvedProfile {
            profile: Profile {
                api_version: "agent-profile/v1".into(),
                name: "reviewer".into(),
                description: "Read-only code reviewer for implement-review loops.".into(),
                role: "reviewer".into(),
                targets: vec![
                    Target::ClaudeSubagent,
                    Target::ClaudeTeammate,
                    Target::CodexAgent,
                ],
                include: vec![],
                model: None,
                permission_mode: None,
                tools: None,
                mcp_servers: None,
                skills: None,
                context: None,
                metadata: None,
            },
            set: CapabilitySet {
                model: Some(ModelSpec {
                    claude: Some("claude-fable-5".into()),
                    codex: Some("gpt-5.3-codex".into()),
                    effort: Some(Effort::High),
                }),
                permission_mode: Some(PermissionMode::Readonly),
                tools: Some(ToolsSpec {
                    allow: Some(vec![
                        "Read".into(),
                        "Grep".into(),
                        "Bash(git diff:*)".into(),
                        "Bash(git log:*)".into(),
                    ]),
                    deny: Some(vec!["Write".into(), "Edit".into()]),
                }),
                mcp_servers: Some(BTreeMap::from([
                    (
                        "github-readonly".to_string(),
                        McpServer {
                            server_type: None,
                            command: Some("github-mcp".into()),
                            args: Some(vec!["--readonly".into()]),
                            env: Some(BTreeMap::from([(
                                "GITHUB_TOKEN".to_string(),
                                "${env:GITHUB_PAT_RO}".to_string(),
                            )])),
                            url: None,
                        },
                    ),
                    (
                        "docs-search".to_string(),
                        McpServer {
                            server_type: Some(McpServerType::Http),
                            url: Some("https://mcp.example.com/docs".into()),
                            ..Default::default()
                        },
                    ),
                ])),
                skills: Some(vec![
                    "code-review".into(),
                    "verification-before-completion".into(),
                ]),
                context: Some(vec!["fragments/reviewer-instructions.md".into()]),
            },
            provenance: Provenance::new(),
            includes: vec![],
        }
    }

    const FRAGMENT: &str = "# Reviewer instructions\n\nReview the diff for correctness.\n";

    /// PlanContext with a real workspace dir holding the reviewer fragment.
    fn ctx_with_fragment(scope: Scope) -> (tempfile::TempDir, PlanContext) {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join(".agent-profile");
        std::fs::create_dir_all(ws.join("fragments")).unwrap();
        std::fs::write(ws.join("fragments/reviewer-instructions.md"), FRAGMENT).unwrap();
        let ctx = PlanContext {
            scope,
            repo_root: tmp.path().to_path_buf(),
            workspace_root: ws,
            home: PathBuf::from("/home/u"),
            session_id: None,
        };
        (tmp, ctx)
    }

    /// S3-T1 golden: design §3.1 output for `reviewer`.
    #[test]
    fn subagent_golden_matches_design_3_1() {
        let (_tmp, ctx) = ctx_with_fragment(Scope::Repo);
        let plan = ClaudeSubagent.plan(&reviewer(), &ctx).unwrap();
        assert_eq!(plan.actions.len(), 1);
        let Action::Create { path, content } = &plan.actions[0] else {
            panic!("expected create");
        };
        assert_eq!(path, ".claude/agents/reviewer.md");
        let expected = format!(
            "---\n\
             name: reviewer\n\
             description: Read-only code reviewer for implement-review loops.\n\
             model: claude-fable-5\n\
             tools: Read, Grep, Bash(git diff:*), Bash(git log:*)\n\
             ---\n\
             <!-- generated by agent-profile v{v} — role: reviewer — do not hand-edit -->\n\
             \n\
             {FRAGMENT}\
             \n\
             Use the code-review skill (see ~/.agents/skills/code-review/SKILL.md).\n\
             Use the verification-before-completion skill \
             (see ~/.agents/skills/verification-before-completion/SKILL.md).\n",
            v = env!("CARGO_PKG_VERSION"),
        );
        assert_eq!(content, &expected);
    }

    #[test]
    fn subagent_skips_inexpressible_fields_with_workarounds() {
        let (_tmp, ctx) = ctx_with_fragment(Scope::Repo);
        let plan = ClaudeSubagent.plan(&reviewer(), &ctx).unwrap();
        let fields: Vec<&str> = plan.skipped.iter().map(|s| s.field.as_str()).collect();
        assert_eq!(
            fields,
            ["mcpServers", "permissionMode", "tools.deny", "model.effort"]
        );
        let mcp = &plan.skipped[0];
        assert_eq!(
            mcp.reason,
            "claude-subagent cannot express per-subagent MCP servers"
        );
        assert_eq!(
            mcp.hint.as_deref(),
            Some("use target claude-teammate, or add servers to .mcp.json yourself")
        );
    }

    #[test]
    fn subagent_user_scope_targets_home_agents_dir() {
        let (_tmp, ctx) = ctx_with_fragment(Scope::User);
        let plan = ClaudeSubagent.plan(&reviewer(), &ctx).unwrap();
        assert_eq!(plan.actions[0].path(), "~/.claude/agents/reviewer.md");
    }

    /// S3-T2: the reviewer teammate plan carries exactly the action kinds from
    /// the §3.4 state example, in surface order.
    #[test]
    fn teammate_plan_matches_design_3_4_action_kinds() {
        let (_tmp, ctx) = ctx_with_fragment(Scope::Repo);
        let plan = ClaudeTeammate.plan(&reviewer(), &ctx).unwrap();
        let summary: Vec<(&str, &str)> = plan.actions.iter().map(|a| (a.op(), a.path())).collect();
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
        let Action::Symlink { link_target, .. } = &plan.actions[3] else {
            panic!("expected symlink");
        };
        assert_eq!(link_target, "~/.agents/skills/code-review");
        let Action::AppendBlock { marker, .. } = &plan.actions[5] else {
            panic!("expected append-block");
        };
        assert_eq!(marker, "role=reviewer");
        // The honest §3.2.3 limitation note is part of the plan.
        assert!(plan.notes.iter().any(|n| n.contains("project-wide")));
    }

    #[test]
    fn teammate_settings_fragment_tracks_entries_individually() {
        let (_tmp, ctx) = ctx_with_fragment(Scope::Repo);
        let plan = ClaudeTeammate.plan(&reviewer(), &ctx).unwrap();
        let Action::MergeKeys { keys, content, .. } = &plan.actions[2] else {
            panic!("expected merge-keys");
        };
        assert_eq!(
            keys,
            &[
                "permissions.allow[+4]",
                "permissions.deny[+2]",
                "permissions.defaultMode"
            ]
        );
        let v: serde_json::Value = serde_json::from_str(content).unwrap();
        assert_eq!(v["permissions"]["allow"][0], "Read");
        assert_eq!(
            v["permissions"]["deny"],
            serde_json::json!(["Write", "Edit"])
        );
        // readonly → plan (Claude has no readonly defaultMode; deny rules
        // carry the posture).
        assert_eq!(v["permissions"]["defaultMode"], "plan");
    }

    #[test]
    fn teammate_mcp_fragment_keeps_claude_mcp_json_shapes() {
        let (_tmp, ctx) = ctx_with_fragment(Scope::Repo);
        let plan = ClaudeTeammate.plan(&reviewer(), &ctx).unwrap();
        let Action::MergeKeys { keys, content, .. } = &plan.actions[1] else {
            panic!("expected merge-keys");
        };
        assert_eq!(
            keys,
            &["mcpServers.docs-search", "mcpServers.github-readonly"]
        );
        let v: serde_json::Value = serde_json::from_str(content).unwrap();
        let gh = &v["mcpServers"]["github-readonly"];
        // type omitted = stdio (recon §1.4 convention).
        assert!(gh.get("type").is_none());
        assert_eq!(gh["command"], "github-mcp");
        assert_eq!(gh["env"]["GITHUB_TOKEN"], "${env:GITHUB_PAT_RO}");
        assert_eq!(v["mcpServers"]["docs-search"]["type"], "http");
        assert_eq!(
            v["mcpServers"]["docs-search"]["url"],
            "https://mcp.example.com/docs"
        );
    }

    #[test]
    fn default_mode_mapping_covers_all_postures() {
        assert_eq!(default_mode(PermissionMode::Readonly), "plan");
        assert_eq!(default_mode(PermissionMode::Plan), "plan");
        assert_eq!(default_mode(PermissionMode::AcceptEdits), "acceptEdits");
        assert_eq!(default_mode(PermissionMode::Auto), "auto");
        assert_eq!(
            default_mode(PermissionMode::Unrestricted),
            "bypassPermissions"
        );
    }

    // ------------------------------------------------------------ JSON merge

    #[test]
    fn json_merge_preserves_foreign_keys_key_for_key() {
        let existing = r#"{
  "mcpServers": {
    "sketch": { "type": "http", "url": "http://localhost:31126/mcp" }
  },
  "otherTopLevel": true
}"#;
        let fragment = r#"{ "mcpServers": { "github": { "command": "github-mcp" } } }"#;
        let keys = vec!["mcpServers.github".to_string()];
        let merged = merge_json_fragment(existing, fragment, &keys, ".mcp.json").unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(
            v["mcpServers"]["sketch"]["url"],
            "http://localhost:31126/mcp"
        );
        assert_eq!(v["mcpServers"]["github"]["command"], "github-mcp");
        assert_eq!(v["otherTopLevel"], true);
        // preserve_order: the foreign server stays first.
        let servers: Vec<&String> = v["mcpServers"].as_object().unwrap().keys().collect();
        assert_eq!(servers, ["sketch", "github"]);
    }

    #[test]
    fn json_merge_collision_reports_leaf_key() {
        let existing = r#"{ "mcpServers": { "github": { "command": "someone-elses-mcp" } } }"#;
        let fragment = r#"{ "mcpServers": { "github": { "command": "github-mcp" } } }"#;
        let keys = vec!["mcpServers.github".to_string()];
        let err = merge_json_fragment(existing, fragment, &keys, ".mcp.json").unwrap_err();
        assert!(matches!(err, Error::Drift(_)), "got {err:?}");
        // Leaf-key reporting, consistent with the TOML merge.
        assert!(
            err.to_string()
                .contains("merge collision at `mcpServers.github.command` in .mcp.json"),
            "{err}"
        );
    }

    #[test]
    fn json_merge_identical_key_is_noop_and_appends_skip_existing() {
        let existing = r#"{
  "permissions": {
    "allow": ["Read", "WebSearch"],
    "defaultMode": "plan"
  }
}"#;
        let fragment = r#"{
  "permissions": {
    "allow": ["Read", "Grep"],
    "defaultMode": "plan"
  }
}"#;
        let keys = vec![
            "permissions.allow[+2]".to_string(),
            "permissions.defaultMode".to_string(),
        ];
        let merged =
            merge_json_fragment(existing, fragment, &keys, ".claude/settings.local.json").unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        // `Read` already present → only `Grep` appended; foreign `WebSearch` kept.
        assert_eq!(
            v["permissions"]["allow"],
            serde_json::json!(["Read", "WebSearch", "Grep"])
        );
        assert_eq!(v["permissions"]["defaultMode"], "plan");
    }

    #[test]
    fn json_merge_default_mode_conflict_is_drift() {
        let existing = r#"{ "permissions": { "defaultMode": "acceptEdits" } }"#;
        let fragment = r#"{ "permissions": { "defaultMode": "plan" } }"#;
        let keys = vec!["permissions.defaultMode".to_string()];
        let err = merge_json_fragment(existing, fragment, &keys, ".claude/settings.local.json")
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("merge collision at `permissions.defaultMode`"),
            "{err}"
        );
    }

    #[test]
    fn json_merge_into_empty_equals_fragment() {
        // `render --out` materializes the fragment; merging into a missing
        // file must produce exactly the same bytes (diff-roundtrip property).
        let (_tmp, ctx) = ctx_with_fragment(Scope::Repo);
        let plan = ClaudeTeammate.plan(&reviewer(), &ctx).unwrap();
        for action in &plan.actions {
            if let Action::MergeKeys { keys, content, .. } = action {
                let merged = merge_json_fragment("", content, keys, action.path()).unwrap();
                assert_eq!(&merged, content, "path {}", action.path());
            }
        }
    }

    /// Regression (AP4-01): a recorded server-name key that already exists is
    /// a collision (drift) even when our value and the existing one differ
    /// only by which sub-keys each carries — we never deep-merge into an entry
    /// someone else created (design §3.2.2/§5.1).
    #[test]
    fn json_merge_same_name_partial_overlap_is_collision_not_deep_merge() {
        let existing = r#"{ "mcpServers": { "github-readonly": { "type": "http", "url": "https://USER-OWNED.example" } } }"#;
        let fragment = r#"{ "mcpServers": { "github-readonly": { "command": "github-mcp", "args": ["--readonly"] } } }"#;
        let keys = vec!["mcpServers.github-readonly".to_string()];
        let err = merge_json_fragment(existing, fragment, &keys, ".mcp.json").unwrap_err();
        assert!(matches!(err, Error::Drift(_)), "got {err:?}");
        assert!(
            err.to_string().contains("mcpServers.github-readonly"),
            "{err}"
        );
        // Forced merge replaces our key wholesale (never partial deep-merge):
        // the foreign `url`/`type` do not leak into the result.
        let forced =
            merge_json_fragment_with(existing, fragment, &keys, ".mcp.json", true).unwrap();
        let v: serde_json::Value = serde_json::from_str(&forced).unwrap();
        let gh = &v["mcpServers"]["github-readonly"];
        assert_eq!(gh["command"], "github-mcp");
        assert!(
            gh.get("url").is_none(),
            "foreign url must not survive: {gh}"
        );
    }

    #[test]
    fn json_merge_malformed_current_is_json_parse_error() {
        let err = merge_json_fragment("{ not json", "{}", &[], ".mcp.json").unwrap_err();
        assert!(matches!(err, Error::JsonParse { .. }), "got {err:?}");
    }

    /// No-flag-bypass: a plan targeting the never list is refused below the
    /// adapters even if one were to produce it (design §5.1).
    #[test]
    fn denylist_refuses_claude_settings_json_plans() {
        let ctx = PlanContext {
            scope: Scope::Repo,
            repo_root: PathBuf::from("/repo"),
            workspace_root: PathBuf::from("/repo/.agent-profile"),
            home: PathBuf::from("/home/u"),
            session_id: None,
        };
        for path in ["~/.claude/settings.json", "~/.claude.json"] {
            let plan = Plan {
                actions: vec![Action::MergeKeys {
                    path: path.into(),
                    keys: vec![],
                    content: "{}".into(),
                }],
                ..Default::default()
            };
            let err = crate::adapters::denylist::assert_plan_allowed(&plan, &ctx).unwrap_err();
            assert!(matches!(err, Error::NeverTouch { .. }), "{path}");
        }
    }
}

//! `codex-agent` adapter (design §3.3): owns `.codex/agents/<role>.toml`
//! (flat keys — never `[profiles.*]`), plans key-level `[mcp_servers.*]`
//! merges into `~/.codex/config.toml` via `toml_edit` (Codex co-writes that
//! file constantly; full-file ownership is forbidden), and appends the marked
//! `@include` stub to AGENTS.md.

use std::collections::BTreeMap;

use toml_edit::{Array, DocumentMut, Item, Table, value};

use crate::adapters::{
    Action, Plan, PlanContext, RenderTarget, Scope, Skipped, block_marker, grammar, marked_block,
    provenance_header_toml,
};
use crate::error::Error;
use crate::schema::merge::ResolvedProfile;
use crate::schema::types::{McpServer, McpServerType, PermissionMode, Target};

pub struct CodexAgent;

impl RenderTarget for CodexAgent {
    fn target(&self) -> Target {
        Target::CodexAgent
    }

    fn plan(&self, r: &ResolvedProfile, ctx: &PlanContext) -> Result<Plan, Error> {
        let role = r.profile.name.as_str();
        let mut skipped = Vec::new();
        let mut actions = Vec::new();

        let agent_path = match ctx.scope {
            Scope::Repo => format!(".codex/agents/{role}.toml"),
            Scope::User => format!("~/.codex/agents/{role}.toml"),
        };
        actions.push(Action::Create {
            path: agent_path,
            content: agent_toml(r, &mut skipped),
        });

        if let Some(servers) = r.set.mcp_servers.as_ref().filter(|s| !s.is_empty()) {
            // Codex reads MCP servers from ~/.codex/config.toml (recon §2,
            // design §4.6) — for both scopes; key-level merge only.
            actions.push(Action::MergeKeys {
                path: "~/.codex/config.toml".to_string(),
                keys: servers
                    .keys()
                    .map(|name| format!("mcp_servers.{name}"))
                    .collect(),
                content: mcp_fragment(servers),
            });
        }

        if let Some(context) = r.set.context.as_ref().filter(|c| !c.is_empty()) {
            let marker = block_marker(ctx, role);
            let path = match ctx.scope {
                Scope::Repo => "AGENTS.md".to_string(),
                Scope::User => "~/.codex/AGENTS.md".to_string(),
            };
            actions.push(Action::AppendBlock {
                path,
                content: marked_block(&marker, role, context),
                marker,
            });
        }

        if let Some(skills) = r.set.skills.as_ref().filter(|s| !s.is_empty()) {
            skipped.push(Skipped {
                field: "skills".to_string(),
                reason: format!(
                    "codex-agent cannot express per-agent skill references ({})",
                    skills.join(", ")
                ),
                hint: Some(
                    "Codex discovers ~/.codex/skills/ itself; no per-agent wiring exists"
                        .to_string(),
                ),
            });
        }

        Ok(Plan {
            actions,
            skipped,
            notes: vec![],
        })
    }
}

/// The owned agent TOML (design §3.3.1). Flat keys, provenance header,
/// `[permissions]` from translated canonical rules.
fn agent_toml(r: &ResolvedProfile, skipped: &mut Vec<Skipped>) -> String {
    let mut doc = DocumentMut::new();
    doc["name"] = value(&r.profile.name);
    doc["description"] = value(&r.profile.description);
    if let Some(model) = &r.set.model {
        if let Some(codex) = &model.codex {
            doc["model"] = value(codex);
        }
        if let Some(effort) = model.effort {
            doc["model_reasoning_effort"] = value(effort.as_str());
        }
    }
    if let Some(mode) = r.set.permission_mode {
        let (approval, sandbox) = approval_keys(mode);
        doc["approval_policy"] = value(approval);
        doc["sandbox_mode"] = value(sandbox);
    }

    let mut permissions = Table::new();
    if let Some(tools) = &r.set.tools {
        if let Some(allow) = &tools.allow
            && let Some(rules) = translate_rules(allow, "allow", "tools.allow", skipped)
        {
            permissions["allow"] = Item::Value(rules.into());
        }
        if let Some(deny) = &tools.deny
            && let Some(rules) = translate_rules(deny, "deny", "tools.deny", skipped)
        {
            permissions["deny"] = Item::Value(rules.into());
        }
    }
    if !permissions.is_empty() {
        doc.insert("permissions", Item::Table(permissions));
    }

    format!("{}\n{}", provenance_header_toml(&r.profile.name), doc)
}

/// `permissionMode` → flat approval keys (design §3.3.4); doctor probes the
/// installed key set in S3 — these are the documented-version defaults.
fn approval_keys(mode: PermissionMode) -> (&'static str, &'static str) {
    match mode {
        PermissionMode::Readonly => ("on-request", "read-only"),
        PermissionMode::Plan => ("untrusted", "read-only"),
        PermissionMode::AcceptEdits => ("on-failure", "workspace-write"),
        PermissionMode::Auto => ("never", "workspace-write"),
        PermissionMode::Unrestricted => ("never", "danger-full-access"),
    }
}

fn translate_rules(
    rules: &[String],
    decision: &str,
    field: &str,
    skipped: &mut Vec<Skipped>,
) -> Option<Array> {
    let mut arr = Array::new();
    for rule in rules {
        match grammar::to_codex(rule, decision) {
            grammar::CodexRule::PrefixRule(s) => arr.push(s),
            grammar::CodexRule::Untranslatable { rule, reason } => skipped.push(Skipped {
                field: field.to_string(),
                reason: format!("`{rule}` — {reason}"),
                hint: Some(
                    "only Bash prefix rules translate to Codex prefix_rule(...)".to_string(),
                ),
            }),
        }
    }
    if arr.is_empty() {
        return None;
    }
    for item in arr.iter_mut() {
        item.decor_mut().set_prefix("\n  ");
    }
    arr.set_trailing("\n");
    arr.set_trailing_comma(true);
    Some(arr)
}

/// TOML fragment holding only the `[mcp_servers.<name>]` tables we add.
fn mcp_fragment(servers: &BTreeMap<String, McpServer>) -> String {
    let mut doc = DocumentMut::new();
    let mut root = Table::new();
    root.set_implicit(true);
    doc.insert("mcp_servers", Item::Table(root));
    for (name, server) in servers {
        let mut t = Table::new();
        match server.server_type {
            McpServerType::Stdio => {
                if let Some(cmd) = &server.command {
                    t["command"] = value(cmd);
                }
                if let Some(args) = &server.args {
                    t["args"] =
                        Item::Value(Array::from_iter(args.iter().map(String::as_str)).into());
                }
            }
            McpServerType::Http => {
                if let Some(url) = &server.url {
                    t["url"] = value(url);
                }
            }
        }
        if let Some(env) = &server.env {
            let mut env_table = Table::new();
            for (k, v) in env {
                env_table[k.as_str()] = value(v);
            }
            t.insert("env", Item::Table(env_table));
        }
        doc["mcp_servers"][name.as_str()] = Item::Table(t);
    }
    doc.to_string()
}

/// Key-level merge of a TOML `fragment` into `current`, preserving comments,
/// formatting, and every table we don't own. A key that already exists with
/// different content is a collision → drift (design §3.2.2/§5.1); an
/// identical key is a no-op.
pub fn merge_toml_fragment(current: &str, fragment: &str, path: &str) -> Result<String, Error> {
    let parse_err = |message: String| Error::TomlParse {
        path: path.into(),
        message,
    };
    let mut doc: DocumentMut = current
        .parse()
        .map_err(|e: toml_edit::TomlError| parse_err(e.to_string()))?;
    let frag: DocumentMut = fragment
        .parse()
        .map_err(|e: toml_edit::TomlError| parse_err(format!("internal fragment: {e}")))?;
    // New tables must land after every existing one — cloned fragment tables
    // carry fragment-local positions that would otherwise interleave.
    let mut next_pos: isize = max_table_position(doc.as_table()) + 1;
    merge_tables(doc.as_table_mut(), frag.as_table(), "", path, &mut next_pos)?;
    Ok(doc.to_string())
}

fn merge_tables(
    dst: &mut Table,
    src: &Table,
    prefix: &str,
    path: &str,
    next_pos: &mut isize,
) -> Result<(), Error> {
    for (key, src_item) in src.iter() {
        let full = if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{prefix}.{key}")
        };
        if !dst.contains_key(key) {
            let mut item = src_item.clone();
            reposition(&mut item, next_pos);
            dst.insert(key, item);
            continue;
        }
        let both_tables = dst[key].is_table() && src_item.is_table();
        if both_tables {
            merge_tables(
                dst[key].as_table_mut().expect("checked is_table"),
                src_item.as_table().expect("checked is_table"),
                &full,
                path,
                next_pos,
            )?;
        } else if dst[key].to_string().trim() != src_item.to_string().trim() {
            return Err(Error::Drift(format!(
                "merge collision at `{full}` in {path}: key already exists with different \
                 content (not created by agent-profile)"
            )));
        }
    }
    Ok(())
}

fn max_table_position(table: &Table) -> isize {
    let mut max = table.position().unwrap_or(0);
    for (_, item) in table.iter() {
        if let Some(child) = item.as_table() {
            max = max.max(max_table_position(child));
        }
    }
    max
}

fn reposition(item: &mut Item, next_pos: &mut isize) {
    if let Some(table) = item.as_table_mut() {
        if !table.is_implicit() {
            table.set_position(Some(*next_pos));
            *next_pos += 1;
        }
        for (_, child) in table.iter_mut() {
            reposition(child, next_pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::desired_file_state;
    use crate::schema::merge::Provenance;
    use crate::schema::types::{CapabilitySet, Effort, ModelSpec, Profile, ToolsSpec};
    use std::path::PathBuf;

    fn resolved(set: CapabilitySet) -> ResolvedProfile {
        ResolvedProfile {
            profile: Profile {
                api_version: "agent-profile/v1".into(),
                name: "implementer".into(),
                description: "Implementation worker with write access and TDD context.".into(),
                role: "implementer".into(),
                targets: vec![Target::CodexAgent],
                include: vec![],
                model: None,
                permission_mode: None,
                tools: None,
                mcp_servers: None,
                skills: None,
                context: None,
                metadata: None,
            },
            set,
            provenance: Provenance::new(),
            includes: vec![],
        }
    }

    fn implementer_set() -> CapabilitySet {
        CapabilitySet {
            model: Some(ModelSpec {
                claude: Some("claude-fable-5".into()),
                codex: Some("gpt-5.5".into()),
                effort: Some(Effort::Medium),
            }),
            permission_mode: Some(PermissionMode::AcceptEdits),
            tools: Some(ToolsSpec {
                allow: Some(vec![
                    "Read".into(),
                    "Bash(cargo test:*)".into(),
                    "Bash(git:*)".into(),
                ]),
                deny: Some(vec!["Bash(git push:*)".into()]),
            }),
            mcp_servers: Some(BTreeMap::from([(
                "github".to_string(),
                McpServer {
                    server_type: McpServerType::Stdio,
                    command: Some("github-mcp".into()),
                    args: None,
                    env: Some(BTreeMap::from([(
                        "GITHUB_TOKEN".to_string(),
                        "${env:GITHUB_PAT_RW}".to_string(),
                    )])),
                    url: None,
                },
            )])),
            skills: Some(vec!["test-driven-development".into()]),
            context: Some(vec!["fragments/implementer-instructions.md".into()]),
        }
    }

    fn ctx(scope: Scope) -> PlanContext {
        PlanContext {
            scope,
            repo_root: PathBuf::from("/repo"),
            workspace_root: PathBuf::from("/repo/.agent-profile"),
            home: PathBuf::from("/home/u"),
            session_id: None,
        }
    }

    #[test]
    fn plan_matches_design_4_6_file_list_ops_and_keys() {
        let plan = CodexAgent
            .plan(&resolved(implementer_set()), &ctx(Scope::Repo))
            .unwrap();
        let summary: Vec<(&str, &str)> = plan.actions.iter().map(|a| (a.op(), a.path())).collect();
        assert_eq!(
            summary,
            [
                ("create", ".codex/agents/implementer.toml"),
                ("merge-keys", "~/.codex/config.toml"),
                ("append-block", "AGENTS.md"),
            ]
        );
        let Action::MergeKeys { keys, .. } = &plan.actions[1] else {
            panic!("expected merge-keys");
        };
        assert_eq!(keys, &["mcp_servers.github"]);
    }

    #[test]
    fn agent_toml_has_header_flat_keys_and_prefix_rules() {
        let plan = CodexAgent
            .plan(&resolved(implementer_set()), &ctx(Scope::Repo))
            .unwrap();
        let Action::Create { content, .. } = &plan.actions[0] else {
            panic!("expected create");
        };
        assert!(content.starts_with("# generated by agent-profile v"));
        assert!(content.contains("— role: implementer"));
        assert!(content.contains("name = \"implementer\""));
        assert!(content.contains("model = \"gpt-5.5\""));
        assert!(content.contains("model_reasoning_effort = \"medium\""));
        assert!(content.contains("approval_policy = \"on-failure\""));
        assert!(content.contains("sandbox_mode = \"workspace-write\""));
        assert!(content.contains("[permissions]"));
        // toml_edit emits literal strings for values containing quotes —
        // identical parsed value to the design §3.3 escaped form.
        assert!(content.contains(r#"'prefix_rule(pattern=["cargo","test"], decision="allow")'"#));
        assert!(content.contains(r#"'prefix_rule(pattern=["git","push"], decision="deny")'"#));
        // Flat keys only — never the native [profiles.*] mechanism.
        assert!(!content.contains("[profiles"));
        // Untranslatable `Read` stays out of the TOML and lands in skipped.
        assert!(!content.contains("Read"));
        assert!(
            plan.skipped
                .iter()
                .any(|s| s.field == "tools.allow" && s.reason.contains("`Read`"))
        );
        // Skills are skipped, never silently dropped.
        assert!(plan.skipped.iter().any(|s| s.field == "skills"));
    }

    #[test]
    fn user_scope_moves_owned_files_under_home() {
        let plan = CodexAgent
            .plan(&resolved(implementer_set()), &ctx(Scope::User))
            .unwrap();
        let paths: Vec<&str> = plan.actions.iter().map(Action::path).collect();
        assert_eq!(
            paths,
            [
                "~/.codex/agents/implementer.toml",
                "~/.codex/config.toml",
                "~/.codex/AGENTS.md",
            ]
        );
    }

    #[test]
    fn plan_never_includes_denylisted_paths() {
        for scope in [Scope::Repo, Scope::User] {
            let c = ctx(scope);
            let plan = CodexAgent.plan(&resolved(implementer_set()), &c).unwrap();
            crate::adapters::denylist::assert_plan_allowed(&plan, &c).unwrap();
            for action in &plan.actions {
                assert!(!action.path().contains("auth.json"));
                assert!(!action.path().contains("history"));
                assert!(!action.path().contains("cache"));
            }
        }
    }

    /// S2-T3 acceptance: a config.toml with comments, `[marketplaces.*]` and
    /// `[projects.*]` tables comes out byte-identical except the added keys.
    #[test]
    fn toml_merge_preserves_existing_file_byte_for_byte() {
        let existing = r#"# my codex config — hands off
model = "gpt-5.5"
model_reasoning_effort = "medium"   # inline comment survives

[marketplaces.claude-plugins-official]
source_type = "git"
source = "https://github.com/anthropics/claude-plugins-official.git"

[projects."/Users/u/Repos/edt-backend"]
trust_level = "trusted"
"#;
        let plan = CodexAgent
            .plan(&resolved(implementer_set()), &ctx(Scope::Repo))
            .unwrap();
        let Action::MergeKeys { content, .. } = &plan.actions[1] else {
            panic!("expected merge-keys");
        };
        let merged = merge_toml_fragment(existing, content, "~/.codex/config.toml").unwrap();
        assert!(
            merged.starts_with(existing),
            "existing bytes must be untouched:\n{merged}"
        );
        let added = &merged[existing.len()..];
        assert!(added.contains("[mcp_servers.github]"));
        assert!(added.contains("command = \"github-mcp\""));
        assert!(added.contains("[mcp_servers.github.env]"));
        assert!(added.contains("GITHUB_TOKEN = \"${env:GITHUB_PAT_RW}\""));
    }

    #[test]
    fn toml_merge_collision_is_drift() {
        let existing = "[mcp_servers.github]\ncommand = \"someone-elses-mcp\"\n";
        let fragment = "[mcp_servers.github]\ncommand = \"github-mcp\"\n";
        let err = merge_toml_fragment(existing, fragment, "~/.codex/config.toml").unwrap_err();
        assert!(matches!(err, Error::Drift(_)), "got {err:?}");
        assert!(err.to_string().contains("mcp_servers.github"));
    }

    #[test]
    fn toml_merge_identical_key_is_noop() {
        let fragment = "[mcp_servers.github]\ncommand = \"github-mcp\"\n";
        let merged = merge_toml_fragment(fragment, fragment, "x.toml").unwrap();
        assert_eq!(merged, fragment);
    }

    #[test]
    fn merge_into_empty_equals_fragment() {
        // diff/apply against a missing config.toml must produce exactly the
        // fragment `render --out` materializes.
        let plan = CodexAgent
            .plan(&resolved(implementer_set()), &ctx(Scope::Repo))
            .unwrap();
        let Action::MergeKeys { content, .. } = &plan.actions[1] else {
            panic!("expected merge-keys");
        };
        let merged = desired_file_state(&plan.actions[1], None).unwrap();
        assert_eq!(&merged, content);
    }

    #[test]
    fn marked_block_matches_design_shape() {
        let plan = CodexAgent
            .plan(&resolved(implementer_set()), &ctx(Scope::Repo))
            .unwrap();
        let Action::AppendBlock {
            marker, content, ..
        } = &plan.actions[2]
        else {
            panic!("expected append-block");
        };
        assert_eq!(marker, "role=implementer");
        assert_eq!(
            content,
            "<!-- agent-profile:begin role=implementer -->\n\
             @.agent-profile/fragments/implementer-instructions.md\n\
             <!-- agent-profile:end role=implementer -->\n"
        );
        assert_eq!(content.lines().count(), 3);
    }

    #[test]
    fn session_id_marker_matches_design_3_2_5() {
        let mut c = ctx(Scope::Repo);
        c.session_id = Some("loop-42-implementer".into());
        let plan = CodexAgent.plan(&resolved(implementer_set()), &c).unwrap();
        let Action::AppendBlock {
            marker, content, ..
        } = &plan.actions[2]
        else {
            panic!("expected append-block");
        };
        assert_eq!(marker, "session=loop-42-implementer");
        assert!(content.starts_with(
            "<!-- agent-profile:begin session=loop-42-implementer role=implementer -->\n"
        ));
        assert!(content.ends_with("<!-- agent-profile:end session=loop-42-implementer -->\n"));
    }

    #[test]
    fn http_server_renders_url_only() {
        let mut set = implementer_set();
        set.mcp_servers = Some(BTreeMap::from([(
            "docs-search".to_string(),
            McpServer {
                server_type: McpServerType::Http,
                url: Some("https://mcp.example.com/docs".into()),
                ..Default::default()
            },
        )]));
        let plan = CodexAgent.plan(&resolved(set), &ctx(Scope::Repo)).unwrap();
        let Action::MergeKeys { content, keys, .. } = &plan.actions[1] else {
            panic!("expected merge-keys");
        };
        assert_eq!(keys, &["mcp_servers.docs-search"]);
        assert!(content.contains("url = \"https://mcp.example.com/docs\""));
        assert!(!content.contains("command"));
    }
}

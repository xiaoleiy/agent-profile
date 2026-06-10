//! Include resolution and merge semantics (design §1.2/§1.3):
//! blocks in listed order, then the profile's own keys win; scalars replace,
//! maps deep-merge one level, lists replace (never concat). Every resolved
//! key records which file supplied its final value.

use std::collections::BTreeMap;

use crate::error::Error;
use crate::schema::types::{CapabilitySet, Profile};
use crate::schema::validate::{Code, Finding};
use crate::schema::workspace::{SourceFile, Workspace};

/// Provenance map: resolved key path → workspace-relative file that supplied
/// the final value (e.g. `model.claude` → `capabilities/models-high-reasoning.yaml`).
pub type Provenance = BTreeMap<String, String>;

#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    pub profile: Profile,
    pub set: CapabilitySet,
    pub provenance: Provenance,
    /// Capability blocks that were resolved, in include order.
    pub includes: Vec<String>,
}

/// Resolve a profile: load it, resolve its one level of includes, and merge.
pub fn resolve(ws: &Workspace, role: &str) -> Result<ResolvedProfile, Error> {
    let profile_src = ws.load_profile(role)?;
    resolve_loaded(ws, &profile_src)
}

/// Resolve an already-loaded profile (lets `validate` reuse parse findings).
pub fn resolve_loaded(
    ws: &Workspace,
    profile_src: &SourceFile<Profile>,
) -> Result<ResolvedProfile, Error> {
    let profile = &profile_src.value;
    let mut set = CapabilitySet::default();
    let mut provenance = Provenance::new();
    let mut findings = Vec::new();

    for include in &profile.include {
        match ws.load_capability(include) {
            Ok(block_src) => {
                // ONE level of include: a capability block may not include
                // another block (design §1.3).
                if block_src.value.include.is_some() {
                    findings.push(Finding::new(
                        Code::NestedInclude,
                        format!(
                            "capability block `{include}` has an `include:` key — capability \
                             blocks may not include other blocks (one level only)"
                        ),
                        Some(block_src.rel.clone()),
                        Some("include".to_string()),
                    ));
                    continue;
                }
                merge_set(
                    &mut set,
                    &mut provenance,
                    block_src.value.capability_set(),
                    &block_src.rel,
                );
            }
            Err(Error::Validation(f)) => findings.extend(f),
            Err(other) => return Err(other),
        }
    }

    if !findings.is_empty() {
        return Err(Error::Validation(findings));
    }

    // Profile's own keys win over all blocks.
    merge_set(
        &mut set,
        &mut provenance,
        profile.capability_set(),
        &profile_src.rel,
    );

    Ok(ResolvedProfile {
        profile: profile.clone(),
        set,
        provenance,
        includes: profile.include.clone(),
    })
}

/// Merge `src` (from `src_file`) into `target`, recording provenance.
pub fn merge_set(
    target: &mut CapabilitySet,
    provenance: &mut Provenance,
    src: CapabilitySet,
    src_file: &str,
) {
    // model: map — deep-merge one level (claude/codex/effort individually).
    if let Some(model) = src.model {
        let t = target.set_model_default();
        if let Some(v) = model.claude {
            t.claude = Some(v);
            provenance.insert("model.claude".into(), src_file.into());
        }
        if let Some(v) = model.codex {
            t.codex = Some(v);
            provenance.insert("model.codex".into(), src_file.into());
        }
        if let Some(v) = model.effort {
            t.effort = Some(v);
            provenance.insert("model.effort".into(), src_file.into());
        }
    }

    // permissionMode: scalar — replace.
    if let Some(v) = src.permission_mode {
        target.permission_mode = Some(v);
        provenance.insert("permissionMode".into(), src_file.into());
    }

    // tools: map — deep-merge one level; allow/deny are lists → replace.
    if let Some(tools) = src.tools {
        let t = target.set_tools_default();
        if let Some(v) = tools.allow {
            t.allow = Some(v);
            provenance.insert("tools.allow".into(), src_file.into());
        }
        if let Some(v) = tools.deny {
            t.deny = Some(v);
            provenance.insert("tools.deny".into(), src_file.into());
        }
    }

    // mcpServers: map — deep-merge one level; each server definition replaces wholesale.
    if let Some(servers) = src.mcp_servers {
        let t = target.set_mcp_servers_default();
        for (name, server) in servers {
            provenance.insert(format!("mcpServers.{name}"), src_file.into());
            t.insert(name, server);
        }
    }

    // skills / context: lists — replace, never concat.
    if let Some(v) = src.skills {
        target.skills = Some(v);
        provenance.insert("skills".into(), src_file.into());
    }
    if let Some(v) = src.context {
        target.context = Some(v);
        provenance.insert("context".into(), src_file.into());
    }
}

impl CapabilitySet {
    fn set_model_default(&mut self) -> &mut crate::schema::types::ModelSpec {
        self.model.get_or_insert_with(Default::default)
    }
    fn set_tools_default(&mut self) -> &mut crate::schema::types::ToolsSpec {
        self.tools.get_or_insert_with(Default::default)
    }
    fn set_mcp_servers_default(
        &mut self,
    ) -> &mut BTreeMap<String, crate::schema::types::McpServer> {
        self.mcp_servers.get_or_insert_with(Default::default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::types::{Effort, McpServer, ModelSpec, PermissionMode, ToolsSpec};

    fn set(yaml: &str) -> CapabilitySet {
        serde_yaml::from_str(yaml).unwrap()
    }

    fn merged(layers: &[(&str, &str)]) -> (CapabilitySet, Provenance) {
        let mut target = CapabilitySet::default();
        let mut prov = Provenance::new();
        for (file, yaml) in layers {
            merge_set(&mut target, &mut prov, set(yaml), file);
        }
        (target, prov)
    }

    #[test]
    fn scalar_override_later_wins() {
        let (out, prov) = merged(&[
            ("a.yaml", "permissionMode: readonly"),
            ("b.yaml", "permissionMode: auto"),
        ]);
        assert_eq!(out.permission_mode, Some(PermissionMode::Auto));
        assert_eq!(prov["permissionMode"], "b.yaml");
    }

    #[test]
    fn map_deep_merges_one_level() {
        let (out, prov) = merged(&[
            ("a.yaml", "model: {claude: c1, effort: high}"),
            ("b.yaml", "model: {codex: x1, effort: medium}"),
        ]);
        let model = out.model.unwrap();
        assert_eq!(
            model,
            ModelSpec {
                claude: Some("c1".into()),
                codex: Some("x1".into()),
                effort: Some(Effort::Medium),
            }
        );
        assert_eq!(prov["model.claude"], "a.yaml");
        assert_eq!(prov["model.codex"], "b.yaml");
        assert_eq!(prov["model.effort"], "b.yaml");
    }

    #[test]
    fn lists_replace_never_concat() {
        let (out, prov) = merged(&[
            ("a.yaml", "skills: [one, two]\ntools: {allow: [Read, Grep]}"),
            ("b.yaml", "skills: [three]\ntools: {allow: [Write]}"),
        ]);
        assert_eq!(out.skills, Some(vec!["three".to_string()]));
        assert_eq!(
            out.tools,
            Some(ToolsSpec {
                allow: Some(vec!["Write".to_string()]),
                deny: None,
            })
        );
        assert_eq!(prov["skills"], "b.yaml");
        assert_eq!(prov["tools.allow"], "b.yaml");
    }

    #[test]
    fn tools_deep_merge_keeps_other_key() {
        let (out, prov) = merged(&[
            ("a.yaml", "tools: {allow: [Read], deny: [Write]}"),
            ("b.yaml", "tools: {allow: [Grep]}"),
        ]);
        let tools = out.tools.unwrap();
        assert_eq!(tools.allow, Some(vec!["Grep".to_string()]));
        assert_eq!(tools.deny, Some(vec!["Write".to_string()]));
        assert_eq!(prov["tools.allow"], "b.yaml");
        assert_eq!(prov["tools.deny"], "a.yaml");
    }

    #[test]
    fn mcp_servers_merge_per_name_replace_wholesale() {
        let (out, prov) = merged(&[
            (
                "a.yaml",
                "mcpServers: {gh: {command: old, args: [--x]}, docs: {type: http, url: u}}",
            ),
            ("b.yaml", "mcpServers: {gh: {command: new}}"),
        ]);
        let servers = out.mcp_servers.unwrap();
        // Wholesale replacement: `args` from a.yaml must not leak through.
        assert_eq!(
            servers["gh"],
            McpServer {
                command: Some("new".into()),
                ..Default::default()
            }
        );
        assert!(servers.contains_key("docs"));
        assert_eq!(prov["mcpServers.gh"], "b.yaml");
        assert_eq!(prov["mcpServers.docs"], "a.yaml");
    }

    #[test]
    fn block_order_then_profile_wins() {
        let (out, prov) = merged(&[
            ("capabilities/a.yaml", "permissionMode: plan\nskills: [a]"),
            ("capabilities/b.yaml", "permissionMode: auto"),
            ("profiles/p.yaml", "skills: [p]"),
        ]);
        // Later block wins over earlier block; profile wins over all blocks.
        assert_eq!(out.permission_mode, Some(PermissionMode::Auto));
        assert_eq!(out.skills, Some(vec!["p".to_string()]));
        assert_eq!(prov["permissionMode"], "capabilities/b.yaml");
        assert_eq!(prov["skills"], "profiles/p.yaml");
    }
}

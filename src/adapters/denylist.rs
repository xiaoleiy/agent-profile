//! Hardcoded never-touch denylist (design §5.1; recon doc §§1.5, 2).
//! Enforced below the adapters before any command consumes a plan —
//! no flag bypasses it, by construction: nothing reads a flag here.

use std::path::Path;

use crate::adapters::{Plan, PlanContext};
use crate::error::Error;

/// File names that are tool-owned state wherever they appear
/// (credentials, histories, caches, plugin registries).
const DENIED_FILE_NAMES: &[&str] = &[
    "auth.json",
    "history.jsonl",
    "session_index.jsonl",
    "models_cache.json",
    ".codex-global-state.json",
    "stats-cache.json",
    "plugin-catalog-cache.json",
    "known_marketplaces.json",
    "installed_plugins.json",
    "mcp-needs-auth-cache.json",
    ".last-update-result.json",
];

/// Is `path` on the never-touch list?
pub fn is_never_touch(path: &Path, home: &Path) -> bool {
    // ~/.claude.json wholesale and ~/.claude/settings.json (never in MVP).
    if path == home.join(".claude.json") || path == home.join(".claude").join("settings.json") {
        return true;
    }
    // Plugin runtime + registries are installer-owned.
    if path.starts_with(home.join(".claude").join("plugins")) {
        return true;
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if DENIED_FILE_NAMES.contains(&name) {
            return true;
        }
        // Codex sqlite state (`*.sqlite*`).
        if name.contains(".sqlite") {
            return true;
        }
    }
    false
}

/// Defense in depth: refuse any plan whose resolved paths hit the denylist,
/// even if an adapter (or test) planned one.
pub fn assert_plan_allowed(plan: &Plan, ctx: &PlanContext) -> Result<(), Error> {
    for action in &plan.actions {
        let abs = ctx.resolve(action.path());
        if is_never_touch(&abs, &ctx.home) {
            return Err(Error::NeverTouch { path: abs });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{Action, Scope};
    use std::path::PathBuf;

    fn home() -> PathBuf {
        PathBuf::from("/home/u")
    }

    #[test]
    fn never_touch_list_matches_recon_doc() {
        for denied in [
            "/home/u/.claude.json",
            "/home/u/.claude/settings.json",
            "/home/u/.claude/plugins/installed_plugins.json",
            "/home/u/.claude/plugins/cache/blob.json",
            "/home/u/.codex/auth.json",
            "/home/u/.codex/history.jsonl",
            "/home/u/.codex/session_index.jsonl",
            "/home/u/.codex/models_cache.json",
            "/home/u/.codex/.codex-global-state.json",
            "/home/u/.codex/state.sqlite3",
            "/home/u/.claude/stats-cache.json",
        ] {
            assert!(
                is_never_touch(Path::new(denied), &home()),
                "{denied} must be denied"
            );
        }
    }

    #[test]
    fn adapter_surfaces_are_allowed() {
        for allowed in [
            "/home/u/.codex/config.toml",
            "/home/u/.codex/agents/reviewer.toml",
            "/home/u/.claude/agents/reviewer.md",
            "/repo/.codex/agents/reviewer.toml",
            "/repo/AGENTS.md",
            "/repo/.mcp.json",
            "/repo/.claude/settings.local.json",
        ] {
            assert!(
                !is_never_touch(Path::new(allowed), &home()),
                "{allowed} must be allowed"
            );
        }
    }

    #[test]
    fn plan_hitting_denylist_is_refused() {
        let ctx = PlanContext {
            scope: Scope::Repo,
            repo_root: PathBuf::from("/repo"),
            workspace_root: PathBuf::from("/repo/.agent-profile"),
            home: home(),
            session_id: None,
        };
        let plan = Plan {
            actions: vec![Action::Create {
                path: "~/.codex/auth.json".into(),
                content: String::new(),
            }],
            ..Default::default()
        };
        let err = assert_plan_allowed(&plan, &ctx).unwrap_err();
        assert!(matches!(err, Error::NeverTouch { .. }));
    }
}

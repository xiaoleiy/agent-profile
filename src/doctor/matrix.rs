//! Versioned field-support matrix (design §5.3.2) — the adapter-churn
//! early-warning system named in `analysis.md` §5.
//!
//! # README: how to update this matrix
//!
//! Each [`Entry`] states, for one (target, profile-field) pair, whether the
//! named CLI honors the field — keyed on **documented versions** (CLI version
//! probing only supplies the version number; we never network-probe).
//!
//! When a vendor ships a release that changes field support:
//!
//! 1. Find the release note / issue establishing the change (link it in a
//!    comment next to the entry).
//! 2. `Spec::Since(v)`-gate newly honored fields with the first version that
//!    honors them; flip `Spec::Never` entries to `Since` the same way.
//! 3. For newly *dropped* fields there is deliberately no `until:` bound yet —
//!    introduce one only when a real regression demands it (YAGNI).
//! 4. Add a lookup test pinning the behavior on both sides of the boundary.
//! 5. Messages are user-facing: name the nearest workaround, always.
//!
//! `{cli}` and `{version}` placeholders in `Never` messages are substituted at
//! check time.

use crate::doctor::Version;
use crate::schema::types::Target;

/// Support spec for one (target, field) pair.
pub enum Spec {
    /// Honored from this CLI version onward.
    Since(Version),
    /// Not honored by any documented version; message names the workaround.
    Never(&'static str),
}

pub struct Entry {
    pub target: Target,
    /// Resolved-profile field path (e.g. `model.effort`, `permissionMode`).
    pub field: &'static str,
    pub spec: Spec,
}

const fn v(major: u64, minor: u64, patch: u64) -> Version {
    Version {
        major,
        minor,
        patch,
    }
}

/// Fields with no entry are either routed to a surface the adapter owns or
/// honored everywhere — no finding.
pub const ENTRIES: &[Entry] = &[
    // Custom subagents (frontmatter name/description/model/tools) shipped in
    // Claude Code 1.0.60.
    Entry {
        target: Target::ClaudeSubagent,
        field: "model.claude",
        spec: Spec::Since(v(1, 0, 60)),
    },
    Entry {
        target: Target::ClaudeSubagent,
        field: "tools",
        spec: Spec::Since(v(1, 0, 60)),
    },
    Entry {
        target: Target::ClaudeSubagent,
        field: "permissionMode",
        spec: Spec::Never(
            "frontmatter field `permissionMode` not honored by {cli} {version} subagent \
             frontmatter — use target claude-teammate (rendered via settings.local.json \
             defaultMode)",
        ),
    },
    Entry {
        target: Target::ClaudeSubagent,
        field: "mcpServers",
        spec: Spec::Never(
            "field `mcpServers` not honored by {cli} {version} subagent frontmatter — use \
             target claude-teammate, or add servers to .mcp.json yourself",
        ),
    },
    Entry {
        target: Target::ClaudeSubagent,
        field: "model.effort",
        spec: Spec::Never(
            "field `model.effort` not honored by {cli} {version} subagent frontmatter — no \
             per-subagent effort field; set effortLevel in settings yourself",
        ),
    },
    // Teammate spawn ignores per-teammate permissionMode (#23669/#24505) —
    // the claude-teammate adapter routes it via settings.local.json.
    Entry {
        target: Target::ClaudeTeammate,
        field: "permissionMode",
        spec: Spec::Never(
            "frontmatter field `permissionMode` not honored by {cli} {version} teammate spawn \
             — rendered via settings.local.json defaultMode instead (info)",
        ),
    },
    Entry {
        target: Target::ClaudeTeammate,
        field: "model.effort",
        spec: Spec::Never(
            "field `model.effort` not honored by {cli} {version} teammate spawn — no \
             per-teammate effort control",
        ),
    },
    // Codex agent TOML flat keys.
    Entry {
        target: Target::CodexAgent,
        field: "model.effort",
        spec: Spec::Since(v(0, 20, 0)),
    },
    Entry {
        target: Target::CodexAgent,
        field: "permissionMode",
        spec: Spec::Since(v(0, 13, 0)),
    },
    Entry {
        target: Target::CodexAgent,
        field: "skills",
        spec: Spec::Never(
            "field `skills` not honored by {cli} {version} agent TOML — Codex discovers \
             ~/.codex/skills/ itself; no per-agent wiring exists",
        ),
    },
];

/// Lookup result for one declared field against the installed version.
pub enum Lookup {
    /// No entry / honored at this version: no finding.
    Honored,
    /// Declared field requires a newer CLI.
    TooOld { since: Version },
    /// No documented version honors the field.
    NotHonored { message: &'static str },
}

pub fn lookup(target: Target, field: &str, version: Option<&Version>) -> Lookup {
    let entry = ENTRIES
        .iter()
        .find(|e| e.target == target && e.field == field);
    match entry.map(|e| &e.spec) {
        None => Lookup::Honored,
        Some(Spec::Never(message)) => Lookup::NotHonored { message },
        Some(Spec::Since(since)) => match version {
            // Unknown version: the caller already warned (F002) and skips
            // version-gated checks rather than guessing.
            None => Lookup::Honored,
            Some(v) if v >= since => Lookup::Honored,
            Some(_) => Lookup::TooOld { since: *since },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S3-T3 acceptance: two synthetic CLI versions with differing support.
    #[test]
    fn lookup_differs_across_versions() {
        let old = Version::parse("0.9.0").unwrap();
        let new = Version::parse("3.2.0").unwrap();
        match lookup(Target::ClaudeSubagent, "tools", Some(&old)) {
            Lookup::TooOld { since } => assert_eq!(since.to_string(), "1.0.60"),
            _ => panic!("0.9.0 must be too old for subagent tools"),
        }
        assert!(matches!(
            lookup(Target::ClaudeSubagent, "tools", Some(&new)),
            Lookup::Honored
        ));
    }

    #[test]
    fn never_entries_are_version_independent() {
        for version in [None, Some(Version::parse("99.0.0").unwrap())] {
            match lookup(Target::ClaudeTeammate, "permissionMode", version.as_ref()) {
                Lookup::NotHonored { message } => {
                    assert!(message.contains("settings.local.json defaultMode"))
                }
                _ => panic!("teammate permissionMode must be NotHonored"),
            }
        }
    }

    #[test]
    fn fields_without_entry_are_honored() {
        assert!(matches!(
            lookup(Target::ClaudeTeammate, "mcpServers", None),
            Lookup::Honored
        ));
        assert!(matches!(
            lookup(Target::CodexAgent, "tools", None),
            Lookup::Honored
        ));
    }

    #[test]
    fn unknown_version_skips_since_gates() {
        assert!(matches!(
            lookup(Target::CodexAgent, "model.effort", None),
            Lookup::Honored
        ));
    }
}

//! Round-trip tests for the design §1 example profiles (S1-T2) and
//! library-level resolution of the checked-in examples.

use std::path::{Path, PathBuf};

use agent_profile::schema::{self, Profile, Target, Workspace};

fn examples() -> Workspace {
    let dir: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/.agent-profile");
    Workspace::at(&dir).unwrap()
}

#[test]
fn example_profiles_round_trip_through_yaml() {
    let ws = examples();
    for role in ["reviewer", "implementer", "qa"] {
        let src = ws.load_profile(role).unwrap();
        let yaml = serde_yaml::to_string(&src.value).unwrap();
        let reparsed: Profile = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(src.value, reparsed, "round-trip mismatch for `{role}`");
    }
}

#[test]
fn reviewer_fields_parse_per_design() {
    let ws = examples();
    let profile = ws.load_profile("reviewer").unwrap().value;
    assert_eq!(profile.name, "reviewer");
    assert_eq!(
        profile.targets,
        vec![
            Target::ClaudeSubagent,
            Target::ClaudeTeammate,
            Target::CodexAgent
        ]
    );
    assert_eq!(
        profile.include,
        vec!["mcp-github-readonly", "perms-readonly"]
    );
    let model = profile.model.as_ref().unwrap();
    assert_eq!(model.claude.as_deref(), Some("claude-fable-5"));
    assert_eq!(model.codex.as_deref(), Some("gpt-5.3-codex"));
    let servers = profile.mcp_servers.as_ref().unwrap();
    assert_eq!(
        servers["github-readonly"].env.as_ref().unwrap()["GITHUB_TOKEN"],
        "${env:GITHUB_PAT_RO}"
    );
    assert_eq!(
        servers["docs-search"].url.as_deref(),
        Some("https://mcp.example.com/docs")
    );
    let metadata = profile.metadata.as_ref().unwrap();
    assert_eq!(
        metadata["owner"],
        serde_yaml::Value::String("xiaolei".into())
    );
}

#[test]
fn resolve_implementer_merges_included_block() {
    let ws = examples();
    let resolved = schema::resolve(&ws, "implementer").unwrap();
    // models-high-reasoning supplies effort: high; the profile overrides to medium.
    let model = resolved.set.model.as_ref().unwrap();
    assert_eq!(model.effort.map(|e| e.as_str()), Some("medium"));
    assert_eq!(
        resolved.provenance["model.effort"],
        "profiles/implementer.yaml"
    );
    assert_eq!(resolved.includes, vec!["models-high-reasoning"]);
}

#[test]
fn library_api_is_usable_without_the_cli() {
    // Fold-into-agent-loop requirement (S1-T1): the full validate path is
    // callable as plain library code.
    let ws = examples();
    let reports = agent_profile::schema::validate::validate_all(&ws).unwrap();
    assert!(reports.iter().all(|r| r.ok()), "{reports:?}");
}

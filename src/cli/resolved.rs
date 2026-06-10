//! `show --resolved` rendering: the fully merged profile with a
//! `# from: <file>` provenance comment per key (design §1.3).

use serde::Serialize;

use crate::schema::merge::ResolvedProfile;
use crate::schema::types::API_VERSION;

/// JSON shape of a resolved profile (used inside the `--json` envelope).
pub fn resolved_json(r: &ResolvedProfile) -> serde_json::Value {
    #[derive(Serialize)]
    struct Resolved<'a> {
        #[serde(rename = "apiVersion")]
        api_version: &'a str,
        name: &'a str,
        description: &'a str,
        role: &'a str,
        targets: Vec<&'a str>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        include: Vec<&'a str>,
        #[serde(flatten)]
        set: &'a crate::schema::types::CapabilitySet,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: &'a Option<std::collections::BTreeMap<String, serde_yaml::Value>>,
    }
    serde_json::to_value(Resolved {
        api_version: API_VERSION,
        name: &r.profile.name,
        description: &r.profile.description,
        role: &r.profile.role,
        targets: r.profile.targets.iter().map(|t| t.as_str()).collect(),
        include: r.includes.iter().map(String::as_str).collect(),
        set: &r.set,
        metadata: &r.profile.metadata,
    })
    .expect("resolved profile serializes")
}

/// Human YAML view with per-key provenance comments.
pub fn resolved_yaml(r: &ResolvedProfile) -> String {
    let mut out = String::new();
    let prov = |key: &str| -> String {
        match r.provenance.get(key) {
            Some(file) => format!("    # from: {file}"),
            None => String::new(),
        }
    };

    out.push_str(&format!("apiVersion: {API_VERSION}\n"));
    out.push_str(&format!("name: {}\n", yaml_scalar(&r.profile.name)));
    out.push_str(&format!(
        "description: {}\n",
        yaml_scalar(&r.profile.description)
    ));
    out.push_str(&format!("role: {}\n", yaml_scalar(&r.profile.role)));
    out.push_str(&format!(
        "targets: [{}]\n",
        r.profile
            .targets
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    if !r.includes.is_empty() {
        out.push_str(&format!("include: [{}]\n", r.includes.join(", ")));
    }

    if let Some(model) = &r.set.model {
        out.push_str("model:\n");
        if let Some(v) = &model.claude {
            out.push_str(&format!(
                "  claude: {}{}\n",
                yaml_scalar(v),
                prov("model.claude")
            ));
        }
        if let Some(v) = &model.codex {
            out.push_str(&format!(
                "  codex: {}{}\n",
                yaml_scalar(v),
                prov("model.codex")
            ));
        }
        if let Some(v) = &model.effort {
            out.push_str(&format!(
                "  effort: {}{}\n",
                v.as_str(),
                prov("model.effort")
            ));
        }
    }

    if let Some(mode) = r.set.permission_mode {
        out.push_str(&format!(
            "permissionMode: {}{}\n",
            mode.as_str(),
            prov("permissionMode")
        ));
    }

    if let Some(tools) = &r.set.tools {
        out.push_str("tools:\n");
        if let Some(allow) = &tools.allow {
            out.push_str(&format!("  allow:{}\n", prov("tools.allow")));
            for rule in allow {
                out.push_str(&format!("    - {}\n", yaml_scalar(rule)));
            }
        }
        if let Some(deny) = &tools.deny {
            out.push_str(&format!("  deny:{}\n", prov("tools.deny")));
            for rule in deny {
                out.push_str(&format!("    - {}\n", yaml_scalar(rule)));
            }
        }
    }

    if let Some(servers) = &r.set.mcp_servers {
        out.push_str("mcpServers:\n");
        for (name, server) in servers {
            out.push_str(&format!(
                "  {}:{}\n",
                yaml_scalar(name),
                prov(&format!("mcpServers.{name}"))
            ));
            out.push_str(&indent_yaml_of(server, 4));
        }
    }

    if let Some(skills) = &r.set.skills {
        out.push_str(&format!("skills:{}\n", prov("skills")));
        for skill in skills {
            out.push_str(&format!("  - {}\n", yaml_scalar(skill)));
        }
    }

    if let Some(context) = &r.set.context {
        out.push_str(&format!("context:{}\n", prov("context")));
        for fragment in context {
            out.push_str(&format!("  - {}\n", yaml_scalar(fragment)));
        }
    }

    if let Some(metadata) = &r.profile.metadata {
        out.push_str("metadata:\n");
        out.push_str(&indent_yaml_of(metadata, 2));
    }

    out
}

/// Serialize a single scalar the way serde_yaml would (handles quoting).
fn yaml_scalar(s: &str) -> String {
    serde_yaml::to_string(s)
        .expect("scalar serializes")
        .trim_end()
        .to_string()
}

fn indent_yaml_of<T: Serialize>(value: &T, indent: usize) -> String {
    let yaml = serde_yaml::to_string(value).expect("value serializes");
    let pad = " ".repeat(indent);
    yaml.lines()
        .map(|line| format!("{pad}{line}\n"))
        .collect::<String>()
}

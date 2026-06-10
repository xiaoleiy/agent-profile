//! Validation: schema, name-stem, targets sanity, include resolution, and the
//! secret-literal scan (sprint S1-T5). Exit 2 on any finding.

use serde::Serialize;

use crate::error::Error;
use crate::schema::secrets;
use crate::schema::types::{McpServer, McpServerType};
use crate::schema::workspace::Workspace;

/// Stable validation finding codes (registered here so they never collide).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// V001 — YAML/schema parse failure (incl. unknown keys, strict parsing).
    SchemaParse,
    /// V002 — unsupported `apiVersion`.
    ApiVersion,
    /// V003 — `name:` does not equal the filename stem.
    NameStemMismatch,
    /// V004 — `targets:` empty or invalid.
    TargetsInvalid,
    /// V005 — included capability block not found.
    MissingCapability,
    /// V006 — `include:` inside a capability block (one level only).
    NestedInclude,
    /// V007 — capability block invalid (e.g. `kind:` is not `capability`).
    CapabilityInvalid,
    /// V008 — MCP server shape invalid (stdio without command / http without url).
    McpServerShape,
    /// V009 — unsafe `skills:`/`context:` reference (path traversal / absolute).
    UnsafeReference,
    /// V010 — value starts with a known credential prefix.
    SecretTokenPrefix,
    /// V011 — high-entropy literal that looks like a credential.
    SecretHighEntropy,
    /// V012 — secret-named key whose value is not a `${env:…}` reference.
    SecretNamedKey,
    /// V013 — `context:` fragment file does not exist in the workspace.
    MissingFragment,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::SchemaParse => "V001",
            Code::ApiVersion => "V002",
            Code::NameStemMismatch => "V003",
            Code::TargetsInvalid => "V004",
            Code::MissingCapability => "V005",
            Code::NestedInclude => "V006",
            Code::CapabilityInvalid => "V007",
            Code::McpServerShape => "V008",
            Code::UnsafeReference => "V009",
            Code::SecretTokenPrefix => "V010",
            Code::SecretHighEntropy => "V011",
            Code::SecretNamedKey => "V012",
            Code::MissingFragment => "V013",
        }
    }

    pub fn is_secret(self) -> bool {
        matches!(
            self,
            Code::SecretTokenPrefix | Code::SecretHighEntropy | Code::SecretNamedKey
        )
    }
}

impl Serialize for Code {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub code: Code,
    pub message: String,
    /// Workspace-relative file, e.g. `profiles/reviewer.yaml`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Key path within the file, e.g. `mcpServers.gh.env.GITHUB_TOKEN`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl Finding {
    pub fn new(code: Code, message: String, file: Option<String>, key: Option<String>) -> Self {
        Self {
            code,
            message,
            file,
            key,
        }
    }
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code.as_str())?;
        if let Some(file) = &self.file {
            write!(f, " [{file}]")?;
        }
        if let Some(key) = &self.key {
            write!(f, " {key}:")?;
        }
        write!(f, " {}", self.message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UnitKind {
    Profile,
    Capability,
}

/// Validation result for one profile or capability block.
#[derive(Debug, Clone, Serialize)]
pub struct UnitReport {
    pub name: String,
    pub kind: UnitKind,
    pub findings: Vec<Finding>,
    #[serde(rename = "blocksResolved")]
    pub blocks_resolved: usize,
}

impl UnitReport {
    pub fn ok(&self) -> bool {
        self.findings.is_empty()
    }
}

/// Validate one role profile (and the capability blocks it includes).
pub fn validate_role(ws: &Workspace, role: &str) -> Result<UnitReport, Error> {
    let mut report = UnitReport {
        name: role.to_string(),
        kind: UnitKind::Profile,
        findings: Vec::new(),
        blocks_resolved: 0,
    };

    let src = match ws.load_profile(role) {
        Ok(src) => src,
        Err(Error::Validation(findings)) => {
            report.findings = findings;
            return Ok(report);
        }
        Err(other) => return Err(other),
    };
    let profile = &src.value;

    if profile.name != role {
        report.findings.push(Finding::new(
            Code::NameStemMismatch,
            format!(
                "`name: {}` must equal the filename stem `{role}`",
                profile.name
            ),
            Some(src.rel.clone()),
            Some("name".to_string()),
        ));
    }

    if profile.targets.is_empty() {
        report.findings.push(Finding::new(
            Code::TargetsInvalid,
            "`targets:` must list at least one runtime target".to_string(),
            Some(src.rel.clone()),
            Some("targets".to_string()),
        ));
    }

    if let Some(servers) = &profile.mcp_servers {
        check_mcp_shapes(servers, &src.rel, &mut report.findings);
        report
            .findings
            .extend(secrets::scan_mcp_servers(servers, &src.raw, &src.rel));
    }

    check_references(
        profile.skills.as_ref(),
        profile.context.as_ref(),
        &src.rel,
        &mut report.findings,
    );
    check_fragments(ws, profile.context.as_ref(), &src.rel, &mut report.findings);

    for include in &profile.include {
        match validate_capability(ws, include) {
            Ok(block_report) => {
                if block_report.ok() {
                    report.blocks_resolved += 1;
                } else {
                    report.findings.extend(block_report.findings);
                }
            }
            Err(other) => return Err(other),
        }
    }

    Ok(report)
}

/// Validate one capability block standalone.
pub fn validate_capability(ws: &Workspace, name: &str) -> Result<UnitReport, Error> {
    let mut report = UnitReport {
        name: name.to_string(),
        kind: UnitKind::Capability,
        findings: Vec::new(),
        blocks_resolved: 0,
    };

    let src = match ws.load_capability(name) {
        Ok(src) => src,
        Err(Error::Validation(findings)) => {
            report.findings = findings;
            return Ok(report);
        }
        Err(other) => return Err(other),
    };
    let block = &src.value;

    if block.kind != "capability" {
        report.findings.push(Finding::new(
            Code::CapabilityInvalid,
            format!("`kind: {}` must be `capability`", block.kind),
            Some(src.rel.clone()),
            Some("kind".to_string()),
        ));
    }

    if block.name != name {
        report.findings.push(Finding::new(
            Code::NameStemMismatch,
            format!(
                "`name: {}` must equal the filename stem `{name}`",
                block.name
            ),
            Some(src.rel.clone()),
            Some("name".to_string()),
        ));
    }

    if block.include.is_some() {
        report.findings.push(Finding::new(
            Code::NestedInclude,
            format!(
                "capability block `{name}` has an `include:` key — capability blocks may not \
                 include other blocks (one level only)"
            ),
            Some(src.rel.clone()),
            Some("include".to_string()),
        ));
    }

    if let Some(servers) = &block.mcp_servers {
        check_mcp_shapes(servers, &src.rel, &mut report.findings);
        report
            .findings
            .extend(secrets::scan_mcp_servers(servers, &src.raw, &src.rel));
    }

    check_references(
        block.skills.as_ref(),
        block.context.as_ref(),
        &src.rel,
        &mut report.findings,
    );
    check_fragments(ws, block.context.as_ref(), &src.rel, &mut report.findings);

    Ok(report)
}

/// Validate everything in the workspace (all profiles, all capability blocks).
pub fn validate_all(ws: &Workspace) -> Result<Vec<UnitReport>, Error> {
    let mut reports = Vec::new();
    for role in ws.profile_names()? {
        reports.push(validate_role(ws, &role)?);
    }
    for cap in ws.capability_names()? {
        reports.push(validate_capability(ws, &cap)?);
    }
    Ok(reports)
}

/// A `skills:` entry must be a bare skill name (resolves to
/// `~/.agents/skills/<name>/SKILL.md`, design §1.2) — never a path that could
/// traverse out of the skills store (the claude-teammate adapter symlinks it).
pub fn is_safe_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

/// A `context:` entry is a workspace-relative fragment path (design §1.2). It
/// may contain `/`, but must stay inside the `.agent-profile/` tree: no
/// absolute paths and no `..` components.
pub fn is_safe_context_path(p: &str) -> bool {
    use std::path::Component;
    !p.is_empty()
        && !p.contains('\0')
        && !std::path::Path::new(p).components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

/// Validate `skills:`/`context:` reference safety (path-traversal guard).
pub fn check_references(
    skills: Option<&Vec<String>>,
    context: Option<&Vec<String>>,
    rel: &str,
    findings: &mut Vec<Finding>,
) {
    if let Some(skills) = skills {
        for skill in skills {
            if !is_safe_skill_name(skill) {
                findings.push(Finding::new(
                    Code::UnsafeReference,
                    format!(
                        "skill `{skill}` is not a valid skill name — skills resolve under \
                         ~/.agents/skills/<name> and cannot contain path separators or `..`"
                    ),
                    Some(rel.to_string()),
                    Some("skills".to_string()),
                ));
            }
        }
    }
    if let Some(context) = context {
        for fragment in context {
            if !is_safe_context_path(fragment) {
                findings.push(Finding::new(
                    Code::UnsafeReference,
                    format!(
                        "context fragment `{fragment}` must be a path inside the .agent-profile \
                         tree — no absolute paths or `..` traversal"
                    ),
                    Some(rel.to_string()),
                    Some("context".to_string()),
                ));
            }
        }
    }
}

/// A `context:` fragment must resolve to a file inside the workspace
/// (design §1.2/§2 — include resolution is validate's job): the
/// claude-subagent adapter inlines the fragment at render time, so a profile
/// validate accepts must be renderable for every listed target — a missing
/// fragment is a validation failure (exit 2), never a raw I/O error
/// (regression A3-R2-2).
fn check_fragments(
    ws: &Workspace,
    context: Option<&Vec<String>>,
    rel: &str,
    findings: &mut Vec<Finding>,
) {
    let Some(context) = context else { return };
    for fragment in context {
        if !is_safe_context_path(fragment) {
            continue; // already a V009 finding
        }
        if !ws.root().join(fragment).is_file() {
            findings.push(Finding::new(
                Code::MissingFragment,
                format!(
                    "context fragment `{fragment}` does not exist (expected \
                     .agent-profile/{fragment})"
                ),
                Some(rel.to_string()),
                Some("context".to_string()),
            ));
        }
    }
}

fn check_mcp_shapes(
    servers: &std::collections::BTreeMap<String, McpServer>,
    rel: &str,
    findings: &mut Vec<Finding>,
) {
    for (name, server) in servers {
        match server.effective_type() {
            McpServerType::Stdio if server.command.is_none() => {
                findings.push(Finding::new(
                    Code::McpServerShape,
                    format!("stdio MCP server `{name}` is missing `command`"),
                    Some(rel.to_string()),
                    Some(format!("mcpServers.{name}")),
                ));
            }
            McpServerType::Http if server.url.is_none() => {
                findings.push(Finding::new(
                    Code::McpServerShape,
                    format!("http MCP server `{name}` is missing `url`"),
                    Some(rel.to_string()),
                    Some(format!("mcpServers.{name}")),
                ));
            }
            _ => {}
        }
    }
}

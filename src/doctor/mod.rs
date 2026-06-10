//! `doctor` (design §5.3): preflight a resolved profile against the
//! *installed* CLI. Probes `<cli> --version` (degrading to the
//! documented-version matrix with `--assume-version` when probing is not
//! possible), then runs the check battery. Errors → exit 4; warnings/info
//! alone → exit 0.

pub mod matrix;

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::adapters::grammar;
use crate::schema::merge::ResolvedProfile;
use crate::schema::types::{McpServer, McpServerType, Target};

/// A parsed CLI version (`X.Y[.Z]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('v');
        let mut parts = s.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = match parts.next() {
            None => 0,
            Some(p) => p.parse().ok()?,
        };
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl std::str::FromStr for Version {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Version::parse(s).ok_or_else(|| format!("`{s}` is not a version (expected X.Y[.Z])"))
    }
}

/// Extract a version from real `--version` output, e.g.
/// `2.0.34 (Claude Code)` or `codex-cli 0.21.0`.
pub fn parse_version_output(output: &str) -> Option<Version> {
    output
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .find_map(Version::parse)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warn,
    Info,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
        }
    }
}

/// Stable F-prefixed finding codes — registered in this one enum so they
/// never collide or get reused (S3-T5). Documented in the README table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingCode {
    /// F001 — CLI binary not found on PATH.
    CliNotFound,
    /// F002 — CLI version could not be determined/parsed.
    CliVersionUnknown,
    /// F012 — profile field not honored by the installed CLI version.
    FieldNotHonored,
    /// F031 — stdio MCP server command not found on PATH.
    McpCommandNotFound,
    /// F032 — http MCP server URL malformed.
    McpUrlInvalid,
    /// F044 — `${env:VAR}` reference unset in the current environment.
    EnvVarUnset,
    /// F051 — referenced skill resolves nowhere.
    SkillUnresolved,
    /// F061 — canonical tool rule untranslatable for the Codex target.
    RuleUntranslatable,
    /// F071 — state.json session older than 24h.
    StaleSession,
}

impl FindingCode {
    pub fn as_str(self) -> &'static str {
        match self {
            FindingCode::CliNotFound => "F001",
            FindingCode::CliVersionUnknown => "F002",
            FindingCode::FieldNotHonored => "F012",
            FindingCode::McpCommandNotFound => "F031",
            FindingCode::McpUrlInvalid => "F032",
            FindingCode::EnvVarUnset => "F044",
            FindingCode::SkillUnresolved => "F051",
            FindingCode::RuleUntranslatable => "F061",
            FindingCode::StaleSession => "F071",
        }
    }
}

impl Serialize for FindingCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub level: Level,
    pub code: FindingCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

#[derive(Debug)]
pub struct Report {
    /// CLI binary the target maps to (`claude` or `codex`).
    pub cli: &'static str,
    pub cli_version: Option<Version>,
    /// True when `--assume-version` supplied the version (no probe ran).
    pub assumed: bool,
    pub findings: Vec<Finding>,
    /// Passing-check summary lines (e.g. the §4.3 skills line).
    pub ok: Vec<String>,
}

impl Report {
    pub fn has_errors(&self) -> bool {
        self.findings.iter().any(|f| f.level == Level::Error)
    }
}

/// Inputs the check battery needs besides the resolved profile.
pub struct Env {
    pub home: PathBuf,
    /// The `.agent-profile/` directory (skill fallback + state.json live here).
    pub workspace_root: PathBuf,
    pub assume_version: Option<Version>,
}

pub fn cli_for(target: Target) -> &'static str {
    match target {
        Target::ClaudeSubagent | Target::ClaudeTeammate => "claude",
        Target::CodexAgent => "codex",
    }
}

/// Run the full check battery (design §5.3).
pub fn run(resolved: &ResolvedProfile, target: Target, env: &Env) -> Report {
    let cli = cli_for(target);
    let mut findings = Vec::new();
    let mut ok = Vec::new();

    // 1. CLI presence + version (§5.3.1), or the documented-version degrade.
    let (cli_version, assumed) = match env.assume_version {
        Some(v) => (Some(v), true),
        None => (probe_cli(cli, &mut findings), false),
    };

    // 2. Field-support matrix (§5.3.2).
    check_field_support(resolved, target, cli, cli_version.as_ref(), &mut findings);

    // 3. MCP server executability (§5.3.3).
    check_mcp_servers(resolved, &mut findings);

    // 4. Secret references resolvable (§5.3.4) — warn, never error.
    check_env_refs(resolved, &mut findings);

    // 5. Skill resolution (§5.3.5).
    check_skills(resolved, env, &mut findings, &mut ok);

    // 6. Permission-grammar translatability for the Codex target (§5.3.6).
    if target == Target::CodexAgent {
        check_grammar(resolved, &mut findings);
    }

    // 7. Stale sessions (§5.3.7) — activates fully when S4 lands state.
    check_stale_sessions(&env.workspace_root, &mut findings);

    Report {
        cli,
        cli_version,
        assumed,
        findings,
        ok,
    }
}

fn probe_cli(cli: &'static str, findings: &mut Vec<Finding>) -> Option<Version> {
    let path = match which::which(cli) {
        Ok(path) => path,
        Err(_) => {
            findings.push(Finding {
                level: Level::Error,
                code: FindingCode::CliNotFound,
                message: format!(
                    "`{cli}` not found on PATH — install it or pass --assume-version to run \
                     version-gated checks against the documented matrix"
                ),
                field: None,
            });
            return None;
        }
    };
    let output = std::process::Command::new(&path).arg("--version").output();
    let version = match &output {
        Ok(out) if out.status.success() => {
            parse_version_output(&String::from_utf8_lossy(&out.stdout))
        }
        _ => None,
    };
    if version.is_none() {
        findings.push(Finding {
            level: Level::Warn,
            code: FindingCode::CliVersionUnknown,
            message: format!(
                "could not determine `{cli}` version from `{cli} --version`; version-gated \
                 field-support checks skipped (use --assume-version to force a version)"
            ),
            field: None,
        });
    }
    version
}

/// Which resolved-profile fields are declared, by matrix field path.
fn declared_fields(resolved: &ResolvedProfile) -> Vec<&'static str> {
    let set = &resolved.set;
    let mut fields = Vec::new();
    if let Some(model) = &set.model {
        if model.claude.is_some() {
            fields.push("model.claude");
        }
        if model.codex.is_some() {
            fields.push("model.codex");
        }
        if model.effort.is_some() {
            fields.push("model.effort");
        }
    }
    if set.permission_mode.is_some() {
        fields.push("permissionMode");
    }
    if set.tools.is_some() {
        fields.push("tools");
    }
    if set.mcp_servers.as_ref().is_some_and(|s| !s.is_empty()) {
        fields.push("mcpServers");
    }
    if set.skills.as_ref().is_some_and(|s| !s.is_empty()) {
        fields.push("skills");
    }
    if set.context.as_ref().is_some_and(|c| !c.is_empty()) {
        fields.push("context");
    }
    fields
}

fn check_field_support(
    resolved: &ResolvedProfile,
    target: Target,
    cli: &str,
    version: Option<&Version>,
    findings: &mut Vec<Finding>,
) {
    let shown_version = version.map_or("(unknown version)".to_string(), Version::to_string);
    for field in declared_fields(resolved) {
        match matrix::lookup(target, field, version) {
            matrix::Lookup::Honored => {}
            matrix::Lookup::TooOld { since } => findings.push(Finding {
                level: Level::Error,
                code: FindingCode::FieldNotHonored,
                message: format!(
                    "field `{field}` requires {cli} >= {since} (installed {shown_version}) — \
                     upgrade {cli} or drop the field"
                ),
                field: Some(field.to_string()),
            }),
            matrix::Lookup::NotHonored { message } => findings.push(Finding {
                level: Level::Error,
                code: FindingCode::FieldNotHonored,
                message: message
                    .replace("{cli}", cli)
                    .replace("{version}", &shown_version),
                field: Some(field.to_string()),
            }),
        }
    }
}

fn check_mcp_servers(resolved: &ResolvedProfile, findings: &mut Vec<Finding>) {
    let Some(servers) = &resolved.set.mcp_servers else {
        return;
    };
    for (name, server) in servers {
        match server.server_type {
            McpServerType::Stdio => {
                if let Some(command) = &server.command
                    && which::which(command).is_err()
                {
                    findings.push(Finding {
                        level: Level::Warn,
                        code: FindingCode::McpCommandNotFound,
                        message: format!(
                            "mcp server `{name}`: command `{command}` not found on PATH"
                        ),
                        field: Some(format!("mcpServers.{name}.command")),
                    });
                }
            }
            McpServerType::Http => {
                if let Some(url) = &server.url
                    && !url_well_formed(url)
                {
                    findings.push(Finding {
                        level: Level::Error,
                        code: FindingCode::McpUrlInvalid,
                        message: format!("mcp server `{name}`: url `{url}` is not well-formed"),
                        field: Some(format!("mcpServers.{name}.url")),
                    });
                }
            }
        }
    }
}

pub fn url_well_formed(url: &str) -> bool {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"));
    matches!(rest, Some(r) if !r.is_empty() && !r.starts_with('/'))
}

fn check_env_refs(resolved: &ResolvedProfile, findings: &mut Vec<Finding>) {
    let Some(servers) = &resolved.set.mcp_servers else {
        return;
    };
    for (name, server) in servers {
        for (key, value) in env_entries(server) {
            let Some(var) = env_reference(value) else {
                continue;
            };
            if std::env::var_os(var).is_none() {
                findings.push(Finding {
                    level: Level::Warn,
                    code: FindingCode::EnvVarUnset,
                    message: format!(
                        "env reference ${{env:{var}}} is unset in current environment"
                    ),
                    field: Some(format!("mcpServers.{name}.env.{key}")),
                });
            }
        }
    }
}

fn env_entries(server: &McpServer) -> impl Iterator<Item = (&String, &String)> {
    server.env.iter().flatten()
}

/// `${env:VAR}` → `VAR`.
fn env_reference(value: &str) -> Option<&str> {
    value.strip_prefix("${env:")?.strip_suffix('}')
}

fn check_skills(
    resolved: &ResolvedProfile,
    env: &Env,
    findings: &mut Vec<Finding>,
    ok: &mut Vec<String>,
) {
    let Some(skills) = resolved.set.skills.as_ref().filter(|s| !s.is_empty()) else {
        return;
    };
    let mut via_fallback = 0usize;
    let mut resolved_count = 0usize;
    for skill in skills {
        let store = env.home.join(".agents/skills").join(skill).join("SKILL.md");
        let fallback = env
            .workspace_root
            .join("skills")
            .join(skill)
            .join("SKILL.md");
        if store.is_file() {
            resolved_count += 1;
        } else if fallback.is_file() {
            resolved_count += 1;
            via_fallback += 1;
        } else {
            findings.push(Finding {
                level: Level::Error,
                code: FindingCode::SkillUnresolved,
                message: format!(
                    "skill `{skill}` resolves nowhere (looked in ~/.agents/skills/{skill}/SKILL.md \
                     and .agent-profile/skills/{skill}/SKILL.md)"
                ),
                field: Some(format!("skills.{skill}")),
            });
        }
    }
    if resolved_count == skills.len() {
        if via_fallback == 0 {
            ok.push(format!(
                "skills: {resolved_count}/{} resolve in ~/.agents/skills/",
                skills.len()
            ));
        } else {
            ok.push(format!(
                "skills: {resolved_count}/{} resolve ({via_fallback} via repo fallback)",
                skills.len()
            ));
        }
    }
}

fn check_grammar(resolved: &ResolvedProfile, findings: &mut Vec<Finding>) {
    let Some(tools) = &resolved.set.tools else {
        return;
    };
    let lists = [("tools.allow", &tools.allow), ("tools.deny", &tools.deny)];
    for (field, rules) in lists {
        let decision = if field.ends_with("allow") {
            "allow"
        } else {
            "deny"
        };
        for rule in rules.iter().flatten() {
            if let grammar::CodexRule::Untranslatable { rule, reason } =
                grammar::to_codex(rule, decision)
            {
                findings.push(Finding {
                    level: Level::Error,
                    code: FindingCode::RuleUntranslatable,
                    message: format!("{field}: `{rule}` — {reason}"),
                    field: Some(field.to_string()),
                });
            }
        }
    }
}

const STALE_AFTER_SECS: i64 = 24 * 60 * 60;

fn check_stale_sessions(workspace_root: &Path, findings: &mut Vec<Finding>) {
    let Ok(raw) = std::fs::read_to_string(workspace_root.join("state.json")) else {
        return;
    };
    // Corrupt state.json is a session/state error owned by apply/teardown
    // (exit 5, S4); doctor stays quiet rather than double-reporting.
    let Ok(state) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let Some(sessions) = state.get("sessions").and_then(|s| s.as_object()) else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for (id, session) in sessions {
        let Some(applied) = session
            .get("appliedAt")
            .and_then(|v| v.as_str())
            .and_then(epoch_secs_from_rfc3339)
        else {
            continue;
        };
        let age = now - applied;
        if age > STALE_AFTER_SECS {
            findings.push(Finding {
                level: Level::Info,
                code: FindingCode::StaleSession,
                message: format!(
                    "session `{id}` applied {}h ago — consider `agent-profile teardown \
                     --session-id {id}`",
                    age / 3600
                ),
                field: None,
            });
        }
    }
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.frac]Z` to Unix seconds (the only timestamp
/// shape state.json contains; avoids a date-time dependency).
pub fn epoch_secs_from_rfc3339(s: &str) -> Option<i64> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let time = time.split('.').next()?;
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Howard Hinnant's days-from-civil algorithm.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parse_handles_real_cli_output_formats() {
        // Real `claude --version` shape.
        assert_eq!(
            parse_version_output("2.0.34 (Claude Code)"),
            Version::parse("2.0.34")
        );
        // Real `codex --version` shape.
        assert_eq!(
            parse_version_output("codex-cli 0.21.0"),
            Version::parse("0.21.0")
        );
        assert_eq!(parse_version_output("v1.2.3"), Version::parse("1.2.3"));
        assert_eq!(parse_version_output("claude 3.2"), Version::parse("3.2.0"));
        assert_eq!(parse_version_output("no digits here"), None);
    }

    #[test]
    fn version_ordering_and_display() {
        let a = Version::parse("1.0.60").unwrap();
        let b = Version::parse("0.9.0").unwrap();
        assert!(b < a);
        assert_eq!(a.to_string(), "1.0.60");
        assert!(Version::parse("1.2.3.4").is_none());
        assert!(Version::parse("nope").is_none());
    }

    #[test]
    fn url_well_formed_cases() {
        assert!(url_well_formed("https://mcp.example.com/docs"));
        assert!(url_well_formed("http://localhost:31126/mcp"));
        assert!(!url_well_formed("ftp://example.com"));
        assert!(!url_well_formed("https://"));
        assert!(!url_well_formed("https:///path-only"));
        assert!(!url_well_formed("example.com/no-scheme"));
    }

    #[test]
    fn env_reference_extraction() {
        assert_eq!(env_reference("${env:GITHUB_PAT_RO}"), Some("GITHUB_PAT_RO"));
        assert_eq!(env_reference("plain-value"), None);
        assert_eq!(env_reference("${env:UNCLOSED"), None);
    }

    #[test]
    fn rfc3339_epoch_parse() {
        assert_eq!(epoch_secs_from_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            epoch_secs_from_rfc3339("1970-01-02T00:00:00Z"),
            Some(86_400)
        );
        // Known constant: date -u -d @1577836800 → 2020-01-01T00:00:00Z.
        assert_eq!(
            epoch_secs_from_rfc3339("2020-01-01T00:00:00Z"),
            Some(1_577_836_800)
        );
        assert_eq!(
            epoch_secs_from_rfc3339("2026-06-10T09:14:03.123Z"),
            epoch_secs_from_rfc3339("2026-06-10T09:14:03Z")
        );
        assert_eq!(epoch_secs_from_rfc3339("not-a-date"), None);
        assert_eq!(epoch_secs_from_rfc3339("2026-06-10T09:14:03"), None);
    }
}

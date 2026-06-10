//! Secret-literal detection (design §5.4). Heuristics run over every `env`
//! value and every string in `mcpServers`. Findings are validation errors;
//! a `# agent-profile: allow-literal` comment on the same source line as the
//! value silences exactly that value.

use std::collections::BTreeMap;

use crate::schema::types::McpServer;
use crate::schema::validate::{Code, Finding};

pub const ALLOW_LITERAL_MARKER: &str = "# agent-profile: allow-literal";

const TOKEN_PREFIXES: &[&str] = &[
    "ghp_",
    "github_pat_",
    "sk-",
    "xox",
    "AKIA",
    "glpat-",
    "Bearer ",
];

const SECRET_KEY_WORDS: &[&str] = &["token", "secret", "key", "password", "credential"];

/// Scan all MCP server definitions from one source file. `raw` is the file's
/// original text (needed for the same-line allow-literal escape hatch);
/// `rel` is the workspace-relative path used in finding locations.
pub fn scan_mcp_servers(
    servers: &BTreeMap<String, McpServer>,
    raw: &str,
    rel: &str,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (name, server) in servers {
        let base = format!("mcpServers.{name}");
        if let Some(command) = &server.command {
            scan_string(command, &format!("{base}.command"), raw, rel, &mut findings);
        }
        if let Some(args) = &server.args {
            for (i, arg) in args.iter().enumerate() {
                scan_string(arg, &format!("{base}.args[{i}]"), raw, rel, &mut findings);
            }
        }
        if let Some(url) = &server.url {
            scan_string(url, &format!("{base}.url"), raw, rel, &mut findings);
        }
        if let Some(env) = &server.env {
            for (key, value) in env {
                scan_env_entry(
                    key,
                    value,
                    &format!("{base}.env.{key}"),
                    raw,
                    rel,
                    &mut findings,
                );
            }
        }
    }
    findings
}

fn scan_env_entry(
    key: &str,
    value: &str,
    key_path: &str,
    raw: &str,
    rel: &str,
    findings: &mut Vec<Finding>,
) {
    // `${env:VAR}` secret-source indirection never flags (design §1.2).
    if is_env_reference(value) {
        return;
    }
    if allow_literal(raw, value) {
        return;
    }
    if is_secret_named_key(key) {
        findings.push(Finding::new(
            Code::SecretNamedKey,
            format!(
                "env key `{key}` looks secret-bearing but its value is a literal — \
                 use ${{env:VAR}} indirection (or annotate the line with \
                 `{ALLOW_LITERAL_MARKER}` if this is a false positive)"
            ),
            Some(rel.to_string()),
            Some(key_path.to_string()),
        ));
        return;
    }
    scan_string_inner(value, key_path, rel, findings);
}

fn scan_string(value: &str, key_path: &str, raw: &str, rel: &str, findings: &mut Vec<Finding>) {
    if is_env_reference(value) || allow_literal(raw, value) {
        return;
    }
    scan_string_inner(value, key_path, rel, findings);
}

fn scan_string_inner(value: &str, key_path: &str, rel: &str, findings: &mut Vec<Finding>) {
    if let Some(prefix) = TOKEN_PREFIXES.iter().find(|p| value.starts_with(**p)) {
        findings.push(Finding::new(
            Code::SecretTokenPrefix,
            format!(
                "value starts with known credential prefix `{}` — use ${{env:VAR}} indirection \
                 (or annotate the line with `{ALLOW_LITERAL_MARKER}`)",
                prefix.trim_end()
            ),
            Some(rel.to_string()),
            Some(key_path.to_string()),
        ));
        return;
    }
    if is_high_entropy(value) {
        findings.push(Finding::new(
            Code::SecretHighEntropy,
            format!(
                "value looks like a high-entropy credential ({} chars) — use ${{env:VAR}} \
                 indirection (or annotate the line with `{ALLOW_LITERAL_MARKER}`)",
                value.len()
            ),
            Some(rel.to_string()),
            Some(key_path.to_string()),
        ));
    }
}

/// `${env:VAR_NAME}` — the only accepted secret-source indirection.
pub fn is_env_reference(value: &str) -> bool {
    let Some(inner) = value
        .strip_prefix("${env:")
        .and_then(|s| s.strip_suffix('}'))
    else {
        return false;
    };
    !inner.is_empty()
        && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !inner.starts_with(|c: char| c.is_ascii_digit())
}

fn is_secret_named_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    SECRET_KEY_WORDS.iter().any(|w| lower.contains(w))
}

/// High-entropy heuristic: ≥20 chars drawn entirely from `[A-Za-z0-9+/_-]`,
/// with Shannon entropy ≥ 3.5 bits/char (filters out ordinary identifiers).
fn is_high_entropy(value: &str) -> bool {
    if value.len() < 20 {
        return false;
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '_' | '-'))
    {
        return false;
    }
    shannon_entropy(value) >= 3.5
}

fn shannon_entropy(s: &str) -> f64 {
    let mut counts: BTreeMap<char, u32> = BTreeMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0) += 1;
    }
    let len = s.chars().count() as f64;
    -counts
        .values()
        .map(|&n| {
            let p = n as f64 / len;
            p * p.log2()
        })
        .sum::<f64>()
}

/// True if some line of the source contains both the literal value and the
/// allow-literal marker — the explicit, greppable false-positive escape.
fn allow_literal(raw: &str, value: &str) -> bool {
    raw.lines()
        .any(|line| line.contains(value) && line.contains(ALLOW_LITERAL_MARKER))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn servers(yaml: &str) -> BTreeMap<String, McpServer> {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn env_reference_never_flags() {
        let yaml = "gh:\n  command: github-mcp\n  env:\n    GITHUB_TOKEN: ${env:GITHUB_PAT_RO}\n";
        let findings = scan_mcp_servers(&servers(yaml), yaml, "p.yaml");
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn token_prefix_triggers_and_plain_value_does_not() {
        let yaml = "gh:\n  command: github-mcp\n  env:\n    SOME_VAR: ghp_abc123\n    OTHER: plain-value\n";
        let findings = scan_mcp_servers(&servers(yaml), yaml, "p.yaml");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, Code::SecretTokenPrefix);
        assert_eq!(
            findings[0].key.as_deref(),
            Some("mcpServers.gh.env.SOME_VAR")
        );
        assert_eq!(findings[0].file.as_deref(), Some("p.yaml"));
    }

    #[test]
    fn bearer_prefix_triggers() {
        let yaml = "gh:\n  command: x\n  env:\n    AUTH_HEADER: Bearer abcdef\n";
        let findings = scan_mcp_servers(&servers(yaml), yaml, "p.yaml");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, Code::SecretTokenPrefix);
    }

    #[test]
    fn high_entropy_triggers_and_ordinary_identifier_does_not() {
        let secret = "q7Zp3rX9aL1mK5vT8wB2cD6e";
        let yaml = format!(
            "gh:\n  command: x\n  env:\n    SOME_VAR: {secret}\n    NAME: chrome-devtools-mcp\n"
        );
        let findings = scan_mcp_servers(&servers(&yaml), &yaml, "p.yaml");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].code, Code::SecretHighEntropy);
    }

    #[test]
    fn url_with_scheme_never_high_entropy() {
        let yaml = "docs:\n  type: http\n  url: https://mcp.example.com/docs\n";
        let findings = scan_mcp_servers(&servers(yaml), yaml, "p.yaml");
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn secret_named_key_with_literal_triggers_with_env_ref_does_not() {
        let yaml =
            "gh:\n  command: x\n  env:\n    API_PASSWORD: hunter2\n    API_KEY: ${env:MY_KEY}\n";
        let findings = scan_mcp_servers(&servers(yaml), yaml, "p.yaml");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].code, Code::SecretNamedKey);
        assert_eq!(
            findings[0].key.as_deref(),
            Some("mcpServers.gh.env.API_PASSWORD")
        );
    }

    #[test]
    fn allow_literal_silences_exactly_the_annotated_value() {
        let yaml = "gh:\n  command: x\n  env:\n    A_TOKEN: not-actually-secret # agent-profile: allow-literal\n    B_TOKEN: also-a-literal\n";
        let findings = scan_mcp_servers(&servers(yaml), yaml, "p.yaml");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(
            findings[0].key.as_deref(),
            Some("mcpServers.gh.env.B_TOKEN")
        );
    }

    #[test]
    fn command_and_args_are_scanned() {
        let yaml = "gh:\n  command: ghp_evilbinary\n  args: [\"AKIAIOSFODNN7EXAMPLE\"]\n";
        let findings = scan_mcp_servers(&servers(yaml), yaml, "p.yaml");
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(findings.iter().all(|f| f.code == Code::SecretTokenPrefix));
    }

    #[test]
    fn env_reference_grammar() {
        assert!(is_env_reference("${env:GITHUB_PAT_RO}"));
        assert!(!is_env_reference("${env:}"));
        assert!(!is_env_reference("${env:1BAD}"));
        assert!(!is_env_reference("$env:VAR"));
        assert!(!is_env_reference("prefix ${env:VAR}"));
    }
}

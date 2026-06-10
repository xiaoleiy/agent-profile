//! Canonical tool-rule grammar (the Claude rule grammar, design §1.2) and its
//! Codex `prefix_rule(...)` translation (design §3.3). Untranslatable rules
//! are classified with a reason — consumed by render (SKIPPED lines) and by
//! doctor in S3.

/// A parsed canonical tool rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRule {
    /// `Read`, `Grep` — a bare tool name.
    Bare { tool: String },
    /// `Bash(git diff:*)` (command prefix) or `Bash(git status)` (exact).
    Bash { words: Vec<String>, prefix: bool },
    /// `Tool(specifier)` for any non-Bash tool, e.g. `Read(/path/**)`.
    Specifier { tool: String, spec: String },
    /// `mcp__server` or `mcp__server__tool`.
    Mcp {
        server: String,
        tool: Option<String>,
    },
}

/// Parse one canonical rule. `Err` carries a human-readable reason.
pub fn parse(rule: &str) -> Result<ToolRule, String> {
    let rule = rule.trim();
    if rule.is_empty() {
        return Err("empty rule".to_string());
    }
    if let Some(rest) = rule.strip_prefix("mcp__") {
        let (server, tool) = match rest.split_once("__") {
            Some((s, t)) => (s, Some(t.to_string())),
            None => (rest, None),
        };
        if server.is_empty() || tool.as_deref() == Some("") {
            return Err(format!("`{rule}`: malformed mcp__server__tool rule"));
        }
        return Ok(ToolRule::Mcp {
            server: server.to_string(),
            tool,
        });
    }
    if let Some(open) = rule.find('(') {
        if !rule.ends_with(')') {
            return Err(format!("`{rule}`: unbalanced parentheses"));
        }
        let tool = &rule[..open];
        let spec = &rule[open + 1..rule.len() - 1];
        if tool.is_empty() || spec.is_empty() {
            return Err(format!("`{rule}`: empty tool name or specifier"));
        }
        if tool == "Bash" {
            // `cmd:*` is the documented prefix form; `cmd *` appears in the
            // wild (recon §1.2) and means the same thing.
            let (body, prefix) = match spec.strip_suffix(":*") {
                Some(b) => (b, true),
                None => match spec.strip_suffix(" *") {
                    Some(b) => (b, true),
                    None => (spec, false),
                },
            };
            let words: Vec<String> = body.split_whitespace().map(str::to_string).collect();
            if words.is_empty() {
                return Err(format!("`{rule}`: empty Bash command"));
            }
            return Ok(ToolRule::Bash { words, prefix });
        }
        return Ok(ToolRule::Specifier {
            tool: tool.to_string(),
            spec: spec.to_string(),
        });
    }
    if rule.contains(char::is_whitespace) {
        return Err(format!("`{rule}`: bare tool names cannot contain spaces"));
    }
    Ok(ToolRule::Bare {
        tool: rule.to_string(),
    })
}

/// Shell metacharacters that turn a `Bash(...)` specifier into a compound
/// command (no single-command prefix → untranslatable to a Codex prefix_rule).
fn is_shell_metachar(c: char) -> bool {
    matches!(c, '|' | '&' | ';' | '<' | '>' | '`' | '$' | '(' | ')')
}

/// Result of translating one canonical rule for the Codex target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexRule {
    PrefixRule(String),
    Untranslatable { rule: String, reason: String },
}

/// Translate a canonical rule to a Codex `prefix_rule(...)` string
/// (`decision` is `allow` or `deny`). Only Bash rules translate.
pub fn to_codex(rule: &str, decision: &str) -> CodexRule {
    let untranslatable = |reason: String| CodexRule::Untranslatable {
        rule: rule.to_string(),
        reason,
    };
    match parse(rule) {
        Ok(ToolRule::Bash { words, .. }) => {
            // A Codex prefix_rule matches a single command's argv prefix. A
            // compound/piped specifier (pipes, redirects, `&&`/`;`/subshells,
            // command/variable substitution) has no single-command prefix —
            // tokenizing the operator into the pattern array yields a rule
            // Codex could never match, so flag it untranslatable (design
            // §3.3/§5.3.6) instead of emitting garbage.
            if words.iter().any(|w| w.chars().any(is_shell_metachar)) {
                return untranslatable(
                    "compound/piped shell specifiers (pipes, redirects, &&/;/subshells, \
                     substitutions) have no single-command Codex prefix_rule equivalent"
                        .to_string(),
                );
            }
            let pattern = words
                .iter()
                .map(|w| format!("\"{}\"", w.replace('\\', "\\\\").replace('"', "\\\"")))
                .collect::<Vec<_>>()
                .join(",");
            CodexRule::PrefixRule(format!(
                "prefix_rule(pattern=[{pattern}], decision=\"{decision}\")"
            ))
        }
        Ok(ToolRule::Bare { tool }) => untranslatable(format!(
            "Codex permissions express only shell prefix rules; bare tool `{tool}` has no prefix_rule equivalent"
        )),
        Ok(ToolRule::Mcp { server, .. }) => untranslatable(format!(
            "MCP tool rules have no Codex prefix_rule equivalent; expose server `{server}` via [mcp_servers.{server}] instead"
        )),
        Ok(ToolRule::Specifier { tool, .. }) => untranslatable(format!(
            "`{tool}(...)` specifier rules have no Codex prefix_rule equivalent"
        )),
        Err(reason) => untranslatable(format!("unparseable rule: {reason}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_grammar_form_parses() {
        let cases: &[(&str, ToolRule)] = &[
            (
                "Read",
                ToolRule::Bare {
                    tool: "Read".into(),
                },
            ),
            (
                "Bash(git diff:*)",
                ToolRule::Bash {
                    words: vec!["git".into(), "diff".into()],
                    prefix: true,
                },
            ),
            (
                "Bash(edb pg *)",
                ToolRule::Bash {
                    words: vec!["edb".into(), "pg".into()],
                    prefix: true,
                },
            ),
            (
                "Bash(git status)",
                ToolRule::Bash {
                    words: vec!["git".into(), "status".into()],
                    prefix: false,
                },
            ),
            (
                "Skill(update-config)",
                ToolRule::Specifier {
                    tool: "Skill".into(),
                    spec: "update-config".into(),
                },
            ),
            (
                "Read(//Users/u/.claude/**)",
                ToolRule::Specifier {
                    tool: "Read".into(),
                    spec: "//Users/u/.claude/**".into(),
                },
            ),
            (
                "mcp__chrome-devtools__take_snapshot",
                ToolRule::Mcp {
                    server: "chrome-devtools".into(),
                    tool: Some("take_snapshot".into()),
                },
            ),
            (
                "mcp__github",
                ToolRule::Mcp {
                    server: "github".into(),
                    tool: None,
                },
            ),
        ];
        for (rule, expected) in cases {
            assert_eq!(&parse(rule).unwrap(), expected, "rule: {rule}");
        }
    }

    #[test]
    fn malformed_rules_error() {
        for bad in ["", "Bash(git diff", "Bash()", "(x)", "Read Write"] {
            assert!(parse(bad).is_err(), "`{bad}` must not parse");
        }
    }

    #[test]
    fn bash_rules_translate_to_exact_design_strings() {
        // Exact strings from design §3.3.
        assert_eq!(
            to_codex("Bash(git diff:*)", "allow"),
            CodexRule::PrefixRule(
                r#"prefix_rule(pattern=["git","diff"], decision="allow")"#.into()
            )
        );
        assert_eq!(
            to_codex("Bash(git log:*)", "allow"),
            CodexRule::PrefixRule(r#"prefix_rule(pattern=["git","log"], decision="allow")"#.into())
        );
        assert_eq!(
            to_codex("Bash(git push:*)", "deny"),
            CodexRule::PrefixRule(r#"prefix_rule(pattern=["git","push"], decision="deny")"#.into())
        );
    }

    #[test]
    fn non_bash_rules_are_untranslatable_with_reasons() {
        match to_codex("Read", "allow") {
            CodexRule::Untranslatable { rule, reason } => {
                assert_eq!(rule, "Read");
                assert!(reason.contains("bare tool `Read`"), "{reason}");
            }
            other => panic!("expected untranslatable, got {other:?}"),
        }
        match to_codex("mcp__x__y", "allow") {
            CodexRule::Untranslatable { reason, .. } => {
                assert!(reason.contains("[mcp_servers.x]"), "{reason}");
            }
            other => panic!("expected untranslatable, got {other:?}"),
        }
        match to_codex("Skill(verify)", "allow") {
            CodexRule::Untranslatable { reason, .. } => {
                assert!(reason.contains("Skill(...)"), "{reason}");
            }
            other => panic!("expected untranslatable, got {other:?}"),
        }
    }

    /// Regression (AREA2-03): a piped/compound Bash specifier must be flagged
    /// untranslatable, never emitted as a `prefix_rule` with the shell
    /// operator naively tokenized into the pattern array.
    #[test]
    fn piped_or_compound_bash_specifier_is_untranslatable() {
        for rule in [
            "Bash(git diff | grep foo:*)",
            "Bash(ls && rm -rf x:*)",
            "Bash(echo $(whoami):*)",
            "Bash(cat a > b:*)",
            "Bash(a; b:*)",
        ] {
            match to_codex(rule, "allow") {
                CodexRule::Untranslatable { reason, .. } => {
                    assert!(
                        reason.contains("compound") || reason.contains("prefix"),
                        "{reason}"
                    );
                }
                CodexRule::PrefixRule(s) => {
                    panic!("`{rule}` must not translate to a prefix_rule, got {s}");
                }
            }
        }
        // A plain prefix rule still translates.
        assert!(matches!(
            to_codex("Bash(git diff:*)", "allow"),
            CodexRule::PrefixRule(_)
        ));
    }
}

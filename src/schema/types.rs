use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The only accepted `apiVersion` value (design §1.2).
pub const API_VERSION: &str = "agent-profile/v1";

/// Runtime targets a profile may be rendered for (design §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    #[serde(rename = "claude-subagent")]
    ClaudeSubagent,
    #[serde(rename = "claude-teammate")]
    ClaudeTeammate,
    #[serde(rename = "codex-agent")]
    CodexAgent,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Target::ClaudeSubagent => "claude-subagent",
            Target::ClaudeTeammate => "claude-teammate",
            Target::CodexAgent => "codex-agent",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude-subagent" => Some(Target::ClaudeSubagent),
            "claude-teammate" => Some(Target::ClaudeTeammate),
            "codex-agent" => Some(Target::CodexAgent),
            _ => None,
        }
    }
}

/// Profile-level permission postures (design §1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    #[serde(rename = "readonly")]
    Readonly,
    #[serde(rename = "plan")]
    Plan,
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "unrestricted")]
    Unrestricted,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionMode::Readonly => "readonly",
            PermissionMode::Plan => "plan",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Auto => "auto",
            PermissionMode::Unrestricted => "unrestricted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpServerType {
    #[default]
    Stdio,
    Http,
}

/// MCP server definition — same shapes as Claude `.mcp.json` (design §1.2);
/// we deliberately do not invent an MCP bundle format.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServer {
    #[serde(rename = "type", default, skip_serializing_if = "is_default_type")]
    pub server_type: McpServerType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

fn is_default_type(t: &McpServerType) -> bool {
    *t == McpServerType::Stdio
}

/// The subset of profile keys a capability block may carry, and the unit the
/// merge algorithm operates on (design §1.3).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CapabilitySet {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSpec>,
    #[serde(
        rename = "permissionMode",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsSpec>,
    #[serde(
        rename = "mcpServers",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub mcp_servers: Option<BTreeMap<String, McpServer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Vec<String>>,
}

/// A role profile: `.agent-profile/profiles/<name>.yaml` (design §1.2).
///
/// `deny_unknown_fields` enforces strict parsing; the capability keys are
/// spelled out (not flattened) because serde's `flatten` is incompatible with
/// `deny_unknown_fields`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub name: String,
    pub description: String,
    pub role: String,
    pub targets: Vec<Target>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSpec>,
    #[serde(
        rename = "permissionMode",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsSpec>,
    #[serde(
        rename = "mcpServers",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub mcp_servers: Option<BTreeMap<String, McpServer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Vec<String>>,
    /// Opaque to rendering; carried into provenance comments (design §1.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, serde_yaml::Value>>,
}

impl Profile {
    /// The profile's own mergeable keys, applied last in the merge order.
    pub fn capability_set(&self) -> CapabilitySet {
        CapabilitySet {
            model: self.model.clone(),
            permission_mode: self.permission_mode,
            tools: self.tools.clone(),
            mcp_servers: self.mcp_servers.clone(),
            skills: self.skills.clone(),
            context: self.context.clone(),
        }
    }
}

/// A capability block: `.agent-profile/capabilities/<name>.yaml` (design §1.3).
///
/// Restricted to the [`CapabilitySet`] keys. `include` is parsed (rather than
/// rejected as an unknown field) so validation can emit the distinct
/// nested-include error the design requires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityBlock {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<serde_yaml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSpec>,
    #[serde(
        rename = "permissionMode",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsSpec>,
    #[serde(
        rename = "mcpServers",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub mcp_servers: Option<BTreeMap<String, McpServer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Vec<String>>,
}

impl CapabilityBlock {
    pub fn capability_set(&self) -> CapabilitySet {
        CapabilitySet {
            model: self.model.clone(),
            permission_mode: self.permission_mode,
            tools: self.tools.clone(),
            mcp_servers: self.mcp_servers.clone(),
            skills: self.skills.clone(),
            context: self.context.clone(),
        }
    }
}

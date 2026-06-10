# Provider Config Surface Recon (macOS field survey, 2026-06-10)

Ground-truth survey of real config surfaces for the agents `agent-profile` must
render into. Collected from a live developer machine running Claude Code,
Codex CLI, Cursor, Gemini CLI, and OpenCode. All secret values redacted.

This document is the design reference for the adapter layer: what each adapter
may own outright, what it must merge surgically, and what it must never touch.

---

## 1. Claude Code

### 1.1 Global files (`~/.claude/`)

| File | Format | Role |
|---|---|---|
| `~/.claude/settings.json` | JSON | User-managed settings — **primary adapter target** |
| `~/.claude/settings.local.json` | JSON | Machine-local permission overrides (same schema, typically `permissions` only) |
| `~/.claude/CLAUDE.md` | Markdown | Global instructions; supports `@file` include syntax (e.g. content of just `@RTK.md`) |
| `~/.claude.json` | JSON | **Tool-owned state file** with one adapter-relevant island: top-level `mcpServers` |
| `~/.claude/config.json` | JSON | `primaryApiKey` — secret, never adapter-owned |
| `~/.claude/skills/` | dir of symlinks | Mostly symlinks into `~/.agents/skills/<name>/` (cross-tool skill store); each target contains `SKILL.md` |
| `~/.claude/agents/` | dir | Custom subagent `.md` files (may not exist) |
| `~/.claude/plugins/` | dir | Plugin runtime + registries (installer-owned) |
| `~/.claude/hooks/` | dir | Hook script payloads referenced from settings |
| `~/.claude/output-styles/` | dir | Custom output styles (`outputStyle` key in settings) |

### 1.2 `settings.json` schema elements

Project-level files declare `"$schema": "https://json.schemastore.org/claude-code-settings.json"` — adapters should emit this.

- **Model**: `"model": "claude-fable-5[1m]"`, plus `"effortLevel": "high"`
- **Permissions**:
  ```json
  "permissions": {
    "allow": ["WebSearch", "mcp__sketch__run_code", "Skill(update-config)",
              "Read(//Users/xiaoleiyu/.claude/**)", "Bash(edb pg:*)"],
    "defaultMode": "auto"
  }
  ```
  Rule grammar: `Tool`, `Tool(specifier)`, `Bash(cmd:*)` / `Bash(cmd *)`,
  `mcp__server__tool`, `Skill(name)`, `Read(path-glob)`. `settings.local.json`
  carries `permissions.allow` / `deny` arrays.
- **Hooks** (events observed: `PreToolUse`, `PostToolUse`, `UserPromptSubmit`, `SessionStart`, `Stop`):
  ```json
  "hooks": { "PreToolUse": [ { "matcher": "Bash",
      "hooks": [ { "type": "command", "command": "rtk hook claude", "timeout": 60 } ] } ] }
  ```
  Matcher is a tool-name regex (`"Write|Edit|MultiEdit"`, `""`/`"*"` for all).
  Project hooks use `$CLAUDE_PROJECT_DIR` expansion.
- **Plugins**:
  ```json
  "enabledPlugins": { "superpowers@claude-plugins-official": true },
  "extraKnownMarketplaces": { "claude-hud": { "source": { "source": "github", "repo": "jarrodwatts/claude-hud" } } }
  ```
  Key format is `plugin@marketplace`. Marketplace `source.source` ∈ `github` | `git` (url) | `directory` (path).
- **Other keys present in the wild**: `env` (per-scope env vars, e.g. `OTEL_*`),
  `statusLine` (`{"type":"command","command":"..."}`), `theme`, `voice`,
  `skipAutoPermissionPrompt`, `outputStyle`.

### 1.3 Plugin registries (`~/.claude/plugins/`)

- `known_marketplaces.json` — `{ "<name>": { "source": {...}, "installLocation", "lastUpdated", "autoUpdate" } }`. Tool-maintained mirror of `extraKnownMarketplaces` + auto-installed official marketplace. Private git marketplaces appear here too.
- `installed_plugins.json` — `{ "version": int, "plugins": { "name@marketplace": [ { "scope": "user", "installPath", "version", "installedAt", "lastUpdated", "gitCommitSha" } ] } }`.
- `cache/`, `marketplaces/`, `plugin-catalog-cache.json` — tool-owned blobs.

**Adapter stance**: own `enabledPlugins`/`extraKnownMarketplaces` in settings.json; treat the registry JSONs as installer-owned (let `claude` CLI materialize them).

### 1.4 MCP

- **Global (user scope)**: `mcpServers` inside `~/.claude.json`:
  ```json
  "sketch":      { "type": "http",  "url": "http://localhost:31126/mcp" },
  "prod-grafana":{ "type": "stdio", "command": "mcp-grafana", "args": [],
                   "env": { "GRAFANA_URL": "<REDACTED>", "GRAFANA_API_KEY": "<REDACTED>" } },
  "sonarqube":   { "command": "sonar", "args": ["run", "mcp"] }   // type omitted = stdio
  ```
- **Project scope**: `<repo>/.mcp.json`, top-level `mcpServers` of the same shape. Real-world files carry API keys inline in `env` — and secrets-scanning PreToolUse hooks may actively block agents reading such files.
- Per-project enable/disable of `.mcp.json` servers is tracked tool-side in `~/.claude.json` → `projects.<path>.enabledMcpjsonServers` / `disabledMcpjsonServers`.

### 1.5 Layering & churn

- Layering: enterprise managed → CLI args → `<proj>/.claude/settings.local.json` → `<proj>/.claude/settings.json` → `~/.claude/settings.json`. Memory files (`~/.claude/CLAUDE.md`, repo `CLAUDE.md`) are additive, with `@include` support.
- **Never own**: `~/.claude.json` as a whole (constantly rewritten; only surgical edits to `mcpServers` are safe, ideally via `claude mcp add`), `history.jsonl`, `stats-cache.json`, `plugin-catalog-cache.json`, `known_marketplaces.json`, `installed_plugins.json`, `mcp-needs-auth-cache.json`, `security_warnings_state_*.json`, `.last-update-result.json`.
- Evidence of third-party collisions: `settings.json.bak` / `settings.json.muxy-backup` siblings exist (other tools also edit settings) — back up before writing.

---

## 2. Codex CLI (`~/.codex/`)

| File | Format | Notes |
|---|---|---|
| `~/.codex/config.toml` | TOML | Main config — **heavily co-written by Codex itself** (see churn) |
| `~/.codex/AGENTS.md` | Markdown | Global instructions; same `@include` convention. Repo-root `AGENTS.md` layered on top |
| `~/.codex/hooks.json` | JSON | Claude-like shape: `{ "hooks": { "Notification": [...], "Stop": [ { "hooks": [ { "type":"command", "command":"..." } ] } ] } }` |
| `~/.codex/rules/default.rules` | custom DSL | Permission allowlist: `prefix_rule(pattern=["git","add"], decision="allow")` per line |
| `~/.codex/skills/` | dirs | Same SKILL.md convention; shared installs |
| `~/.codex/auth.json` | JSON | Credentials — never touch |
| `<repo>/.codex/config.toml` + `<repo>/.codex/hooks.json` | TOML/JSON | Project scope (e.g. `personality = "friendly"` + `[features] hooks = true`; hooks with `matcher: "apply_patch|Edit|Write|MultiEdit"`) |

### config.toml schema elements

```toml
model = "gpt-5.5"
model_reasoning_effort = "medium"
notify = ["<path-to-app>", "turn-ended"]

[features]
memories = true

[mcp_servers.figma]
url = "https://mcp.figma.com/mcp"            # remote server: just url

[mcp_servers.open-design]
command = ".../node"
args = [".../cli.js", "mcp"]
startup_timeout_sec = 120                     # optional
[mcp_servers.open-design.env]
OD_DATA_DIR = "..."

[plugins."superpowers@claude-plugins-official"]
enabled = true                                # same plugin@marketplace key format as Claude!

[marketplaces.claude-plugins-official]        # tool-maintained: last_updated, last_revision, source_type, source
source_type = "git"
source = "https://github.com/anthropics/claude-plugins-official.git"

[projects."/Users/xiaoleiyu/Repos/edt-backend"]
trust_level = "trusted"                       # tool-maintained trust ledger

[shell_environment_policy]
inherit = "core"
[shell_environment_policy.set]                # env injection — may contain plaintext tokens in the wild
ANTHROPIC_BASE_URL = "..."

[hooks.state."<file>:<event>:<i>:<j>"]        # tool-maintained hook trust hashes
trusted_hash = "sha256:..."
```

- **`[profiles.*]` native profile mechanism exists** (`[profiles.NAME]` overriding model/approval keys, selected via `codex --profile NAME`) but was **not used on the surveyed machine** — adapters can emit either profiles or flat keys.
- `shell_environment_policy.set` in the wild contains plaintext API tokens — agent-profile must support env-var/secret-source indirection instead of reproducing this pattern.

### Layering & churn

- Layering: `~/.codex/config.toml` → `<repo>/.codex/config.toml`; `~/.codex/AGENTS.md` → repo `AGENTS.md`; hooks merge from global + project + plugin hooks.
- **Churn risk is severe: Codex rewrites `config.toml` itself** — `[marketplaces.*].last_updated/last_revision`, `[projects.*].trust_level`, `[hooks.state.*].trusted_hash`, `[tui.*]`, `[desktop.*]`. An adapter must do **surgical TOML key merges, never full-file ownership**.
- Never touch: `auth.json`, `*.sqlite*`, `history.jsonl`, `session_index.jsonl`, `models_cache.json`, `.codex-global-state.json`.

---

## 3. Cursor

| File | Format | Notes |
|---|---|---|
| `~/.cursor/mcp.json` | JSON | `{ "mcpServers": { ... } }` — same shape as Claude project `.mcp.json`; remote entries `{ "url": ..., "headers": {} }`, stdio `{ "command", "args" }` or full `{ "type": "stdio", ..., "env": {...} }` |
| `~/.cursor/cli-config.json` | JSON | Cursor CLI agent settings — **mixed user-intent + tool cache** (gets corrupted/rewritten; `.bad` siblings observed) |
| `~/.cursor/hooks.json` | JSON | Hook events differ from Claude: `beforeShellExecution`, `beforeMCPExecution`, `preToolUse` (with `matcher: "Shell"`), `stop`; entries `{ "command": "..." }` + top-level `"version": 1` |
| `<proj>/.cursor/rules/*.mdc` | Markdown + YAML frontmatter | Frontmatter keys: `description`, `globs`, `alwaysApply`; markdown body |
| `<proj>/.cursor/mcp.json` | JSON | Project MCP scope |
| `~/.cursor/skills/`, `<proj>/.cursor/skills/` | dirs | SKILL.md convention again |

### cli-config.json adapter-relevant keys

```json
{
  "version": 1,
  "permissions": { "allow": ["Shell(ls)", "Shell(yarn run)", "Shell(grep)"], "deny": [] },
  "model": { "modelId": "default", "displayName": "Auto", "maxMode": false },
  "approvalMode": "allowlist",
  "sandbox": { "mode": "disabled", "networkAccess": "user_config_with_defaults" },
  "editor": { "vimMode": false }
}
```

Permission grammar is `Shell(cmd)` (cf. Claude's `Bash(cmd:*)`). Tool-owned keys in the **same file**: `privacyCache`, `serverConfigCache`, statsig counters, `authInfo`, streak counters — surgical merge only.

### Layering & churn

- Layering: `~/.cursor/` global → `<proj>/.cursor/` (rules, mcp.json, hooks). `.mdc` rules attach by `globs`/`alwaysApply`.
- Never own: `statsig-cache.json`, `prompt_history.json`, `chats/`, `state/`, `ide_state.json`, `agent-cli-state.json`; `cli-config.json` only via key-level merge. Third-party tools (Muxy) also rewrite `hooks.json` — three writers on one file; coordinate, don't clobber.

---

## 4. Others (existence check)

- **Gemini CLI** (`~/.gemini/`): `settings.json` embeds `mcpServers` directly (`{ "theme", "selectedAuthType", "mcpServers": {} }`); `GEMINI.md` is the instructions analogue.
- **OpenCode**: `~/.config/opencode/opencode.json` (`$schema: https://opencode.ai/config.json`), `plugins/` npm-managed; legacy `~/.opencode/` data dir also exists.

---

## 5. Cross-cutting observations for adapter design

1. **Convergent vocabulary, divergent encodings.** The same concepts appear everywhere: MCP server = `{command,args,env}` or `{url}` (JSON in Claude/Cursor/Gemini, TOML tables in Codex); plugins keyed as `name@marketplace` in **both** Claude (`enabledPlugins` JSON map) and Codex (`[plugins."..."] enabled = true`); hooks as `{event: [{matcher, hooks:[{type:"command", command}]}]}` in Claude and Codex (different event names) vs Cursor's flatter `{event: [{command}]}`; permissions as `Bash(x:*)` (Claude) vs `Shell(x)` (Cursor) vs `prefix_rule(...)` DSL (Codex). A YAML profile can normalize all of these.
2. **A shared skill store already exists in the wild**: `~/.agents/skills/` with per-tool symlink farms (`~/.claude/skills/* → ../../.agents/skills/*`). agent-profile should adopt/extend this rather than copying skills per tool.
3. **Secrets live inline in env maps** (`GRAFANA_API_KEY`, `GITHUB_PERSONAL_ACCESS_TOKEN` in `.mcp.json`; tokens in Codex `shell_environment_policy.set`; `primaryApiKey` in `~/.claude/config.json`). The profile format needs secret references (env/keychain), and secrets-scanning hooks may actively block agents reading files with inline tokens.
4. **Files the adapter must NEVER own** (rewritten by tools): `~/.claude.json` (whole file), `~/.claude/plugins/*.json` registries, all caches/history/state files; **Codex `config.toml` requires key-level merge** (trust ledgers, marketplace revisions, hook hashes live in it); Cursor `cli-config.json` and `hooks.json` likewise.
5. **Safe full-ownership targets**: `~/.claude/settings.json` (with backup), project `.claude/settings.json`, `.mcp.json`, `.cursor/rules/*.mdc`, `.cursor/mcp.json`, `CLAUDE.md`/`AGENTS.md`/`GEMINI.md` instruction files — all support tiny `@include` stubs pointing at a canonical file (a pattern this machine already uses).

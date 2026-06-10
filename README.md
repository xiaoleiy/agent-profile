# agent-profile

`agent-profile` is a proposed local-first profile control plane for multi-agent
engineering workflows.

It helps users define, switch, launch, and govern combinations of:

- agent providers and runtimes
- LLM models
- agent roles
- skills, rules, and prompts
- MCP servers and tools
- permissions and approval modes
- environments and secret sources
- loop topologies and verifier contracts

The first design target is integration with [`agent-loop`](https://github.com/xiaoleiy/agent-loop),
while keeping the profile format portable enough to render into tools such as
Claude Code, Cursor, Codex, Continue, Aider, CrewAI, LangGraph, and MCP
gateways.

## Product Direction

See [`docs/roadmap.md`](docs/roadmap.md) for the roadmap, major feature areas,
and product design principles.

## Status

Sprints 1–3 of the MVP are implemented (see `docs/product/sprints.md`).
Working today, end-to-end on the fixtures in `examples/.agent-profile/`:

- **Profile schema v1** (`apiVersion: agent-profile/v1`): strict parsing
  (unknown keys are errors), capability blocks restricted to
  `model|permissionMode|tools|mcpServers|skills|context`.
- **Include resolution + merge**: one level of capability includes; blocks in
  listed order, profile wins; scalars replace, maps deep-merge one level,
  lists replace. Per-key provenance is tracked.
- **Secret-literal scanner**: known token prefixes, high-entropy literals,
  secret-named keys not using `${env:VAR}` indirection; silence false
  positives with a same-line `# agent-profile: allow-literal` comment.
- **Commands**: `validate [--role <r>]`, `list`, `show --role <r>
  [--resolved]` (with `# from: <file>` provenance comments). All support
  `--json` (`schemaVersion: 1`) and `--dir`.
- **`render --role <r> --target <t> [--scope repo|user] [--out <dir>]`** for
  all three targets, dry-run by default — prints the plan and SKIPPED lines
  for anything the target cannot express. There is deliberately no `--dry-run`
  flag; the only way render touches disk is `--out <sandbox-dir>`.
  - `codex-agent`: owned agent TOML, key-level `[mcp_servers.*]` merge into
    `~/.codex/config.toml` via `toml_edit`, marked `@include` block for
    `AGENTS.md`.
  - `claude-subagent`: single owned `.claude/agents/<role>.md` (frontmatter
    `name`/`description`/`model`/`tools`, fragment contents + per-skill
    pointer lines in the body); `mcpServers`/`permissionMode`/deny rules are
    SKIPPED with the named workaround.
  - `claude-teammate`: agent `.md` (own) + `.mcp.json` merge (added
    `mcpServers.*` keys only) + `.claude/settings.local.json` merge
    (`permissions.allow`/`deny` tracked individually,
    `permissions.defaultMode` from `permissionMode`) + per-skill symlinks
    into `~/.agents/skills/` + marked `@include` block in `CLAUDE.md`. Render
    output carries the honest limitation note: settings.local.json is
    project-wide, not per-teammate — give each worker its own worktree.
- **`diff --role <r> --target <t>`**: rendered plan vs what is on disk;
  unified diff per path (symlinks diff on their target); exit 3 when
  differences exist (script-gate semantics, like `git diff --exit-code`).
- **`doctor --role <r> --target <t> [--assume-version X.Y.Z]`**: preflights
  the profile against the *installed* CLI — probes `claude --version` /
  `codex --version` (or trusts `--assume-version`), then checks a versioned
  field-support matrix, MCP `command` on PATH / `url` shape, `${env:VAR}`
  resolvability (warn only), skill resolution
  (`~/.agents/skills/<name>/SKILL.md` then the repo fallback), Codex
  permission-grammar translatability, and stale `state.json` sessions.
  Errors → exit 4; warnings/info alone → exit 0.
- **Canonical tool grammar** (`Tool`, `Tool(spec)`, `Bash(cmd:*)`,
  `mcp__server__tool`) with `Bash(...)` → Codex `prefix_rule(...)` translation;
  untranslatable rules are surfaced, never silently dropped.
- **Never-touch denylist**: `~/.claude.json`, `~/.claude/settings.json`, plugin
  registries, `auth.json`, caches/history — hardcoded below the adapters; no
  flag bypasses it.

Try it (writes only into the sandbox directory you name):

```bash
cargo run -- validate --dir examples/.agent-profile
cargo run -- show --role reviewer --resolved --dir examples/.agent-profile
cargo run -- render --role implementer --target codex-agent --dir examples/.agent-profile
cargo run -- render --role implementer --target codex-agent \
  --dir examples/.agent-profile --out /tmp/agent-profile-out
cargo run -- diff --role implementer --target codex-agent --dir examples/.agent-profile
cargo run -- render --role reviewer --target claude-teammate --dir examples/.agent-profile
cargo run -- doctor --role reviewer --target claude-teammate --dir examples/.agent-profile
```

### Doctor finding codes

Stable F-prefixed codes (`doctor --json` exposes them as `findings[].code`;
codes are never reused):

| Code | Level | Meaning |
|---|---|---|
| F001 | error | CLI binary (`claude`/`codex`) not found on PATH |
| F002 | warn | CLI version could not be determined; version-gated checks skipped |
| F012 | error | Profile field not honored by the installed CLI version (workaround named) |
| F031 | warn | stdio MCP server `command` not found on PATH |
| F032 | error | http MCP server `url` not well-formed |
| F044 | warn | `${env:VAR}` reference unset in the current environment |
| F051 | error | Referenced skill resolves nowhere (store + repo fallback checked) |
| F061 | error | Tool rule untranslatable to a Codex `prefix_rule(...)` |
| F071 | info | `state.json` session older than 24h — consider teardown |

Not yet implemented (exit 1 with a "not implemented" message): `apply`,
`teardown`, `current`, `completions` — these land in sprints S4–S5 per the
sprint plan.

Exit codes follow the design contract: 0 success, 1 internal error,
2 validation failure, 3 drift, 4 doctor errors, 5 session/state error.

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

Sprints 1–2 of the MVP are implemented (see `docs/product/sprints.md`).
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
- **`render --role <r> --target codex-agent [--scope repo|user] [--out <dir>]`**:
  dry-run by default — prints the plan (owned agent TOML, key-level
  `[mcp_servers.*]` merge into `~/.codex/config.toml` via `toml_edit`, marked
  `@include` block for `AGENTS.md`) and SKIPPED lines for anything the target
  cannot express (e.g. non-Bash tool rules, skills). There is deliberately no
  `--dry-run` flag; the only way render touches disk is `--out <sandbox-dir>`.
- **`diff --role <r> --target codex-agent`**: rendered plan vs what is on disk;
  unified diff per path; exit 3 when differences exist (script-gate semantics,
  like `git diff --exit-code`).
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
```

Not yet implemented (exit 1 with a "not implemented" message): the
`claude-subagent`/`claude-teammate` render targets, `doctor`, `apply`,
`teardown`, `current`, `completions` — these land in sprints S3–S5 per the
sprint plan.

Exit codes follow the design contract: 0 success, 1 internal error,
2 validation failure, 3 drift, 4 doctor errors, 5 session/state error.

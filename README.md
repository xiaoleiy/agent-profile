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

Sprint 1 of the MVP is implemented (see `docs/product/sprints.md`). Working
today, end-to-end on the fixtures in `examples/.agent-profile/`:

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

```bash
cargo run -- validate --dir examples/.agent-profile
cargo run -- show --role reviewer --resolved --dir examples/.agent-profile
```

Not yet implemented (exit 1 with a "not implemented" message): `doctor`,
`render`, `diff`, `apply`, `teardown`, `current`, `completions` — these land
in sprints S2–S5 per the sprint plan.

Exit codes follow the design contract: 0 success, 1 internal error,
2 validation failure, 3 drift, 4 doctor errors, 5 session/state error.

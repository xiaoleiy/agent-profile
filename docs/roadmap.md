# Agent Profile Roadmap

`agent-profile` is a proposed capability/profile layer for multi-agent
engineering. It helps users define, switch, launch, and govern combinations of
agents, models, tools, skills, MCP servers, environments, permissions, and loop
topologies.

The goal is not to replace Claude Code, Cursor, Codex, Continue, CrewAI,
LangGraph, `agent-loop`, or MCP gateways. The goal is to provide a portable
profile spec that can compile into those systems and keep multi-agent workflows
repeatable.

## Problem

As agent ecosystems grow, each provider manages configuration differently:

- Claude Code has layered settings, MCP scopes, skills, subagents, hooks, and
  permissions.
- Cursor has rules, MCP configuration, permissions, run modes, local/cloud
  agents, and SDK agents.
- Codex has `config.toml` profiles, `AGENTS.md`, MCP config, sandbox and
  approval settings.
- Continue, Cline-style tools, Aider, CrewAI, AutoGen, and LangGraph each have
  their own model, role, tool, memory, and workflow definitions.
- MCP gateways help with tool governance but usually do not manage local agent
  roles, prompts, models, IDE settings, or loop topology.

This creates profile sprawl. A user may need one setup at home, another at the
office, another for CI, another for a designer role, another for QA, and another
for a high-autonomy engineering loop. Without a profile layer, each setup is a
set of fragile local edits, shell aliases, copied JSON files, and implicit
knowledge.

## Product Thesis

`agent-profile` should be the profile control plane for Looping Engineering.

It should let users describe an intended agent team once, then render and launch
the correct provider-specific configuration for each target runtime.

Useful mental model:

```text
agent-profile = direnv + kubeconfig + mise + AGENTS.md + MCP registry
for multi-agent engineering
```

The core product should stay local-first, inspectable, and file-based. Team and
enterprise features should grow from the same spec rather than introduce a
separate platform-only model.

## Product Design Principles

### Local-first, then governed

Profiles should work from files in a repository before any server exists. A
single engineer should be able to define, render, diff, and run profiles
offline. Team features should add synchronization, policy, approval, and audit
without making the local workflow feel remote-controlled.

### Declarative over imperative

Users should describe the desired agent capability set, not hand-script every
provider config write. The tool should render Cursor, Claude Code, Codex, and
framework-specific files from a stable profile schema.

### Compose small capability blocks

Profiles should be built from reusable blocks:

- environments
- model policies
- agent roles
- skills and rules
- MCP bundles
- permission bundles
- loop templates
- verifier bundles

This avoids giant profiles and lets teams reuse safe pieces across workflows.

### Make safety visible

Every profile should make risk obvious before launch:

- which tools are available
- which tools auto-run
- which commands require approval
- which secrets are mounted
- which files or systems can be changed
- which agent has write access
- which loop stop conditions apply

`agent-profile diff` and `agent-profile doctor` should be first-class features,
not afterthoughts.

### Compile, do not hide

Generated provider config should be readable and explainable. Users should be
able to inspect what will be written before applying it, and the tool should
record where each rendered setting came from.

### Separate profile intent from provider adapters

The profile schema should capture stable intent. Provider-specific rendering
should live behind adapters so new tools can be added without changing the core
concepts.

### Treat multi-agent loops as teams

Looping Engineering profiles should model teams, not just single-agent settings.
Each agent needs an explicit role, model, tool surface, permission level,
workspace strategy, memory policy, and handoff contract.

### Prefer progressive autonomy

The product should support a path from manual runs to supervised loops to
high-autonomy loops. Autonomy should increase only when verifiers, permissions,
budgets, and audit trails are explicit.

## Core Concepts

### Profile

A named configuration bundle for a scenario, such as:

- `home/fullstack`
- `office/looping-engineering`
- `client/readonly-review`
- `ci/fix-ci`
- `design/prototype`
- `qa/browser-regression`

### Capability Block

A reusable unit included by profiles. Examples:

- `mcp.github-readonly`
- `mcp.linear-triage`
- `permissions.readonly`
- `permissions.implementation-safe`
- `rules.rust-service`
- `skills.review-and-verify`
- `models.high-reasoning`
- `env.office`

### Agent Role

A role in an agent team. Example roles:

- `planner`
- `implementer`
- `reviewer`
- `qa`
- `designer`
- `release-manager`
- `security-reviewer`

### Runtime Target

A concrete system that receives rendered configuration:

- Claude Code
- Cursor IDE or Cursor CLI/SDK
- Codex CLI
- Continue
- Aider
- Cline-style tools
- CrewAI
- LangGraph
- MCP gateway

### Loop Topology

The multi-agent workflow structure:

- single agent loop
- implementer plus reviewer
- planner, implementer, verifier
- designer, implementer, QA
- committee review
- CI babysitting loop
- release readiness loop

## Example Profile Shape

```yaml
name: office/looping-engineering/fullstack
description: Full-stack implementation loop for trusted office projects.

environment:
  name: office
  secrets: onepassword:engineering
  network: internal

defaults:
  approval: supervised
  workspace: git-worktree
  maxIterations: 5
  maxWallTime: 90m

agents:
  planner:
    target: claude-code
    model: claude-opus-4-8-thinking-high
    role: planner
    skills:
      - brainstorming
      - writing-plans
    permissions: readonly
    mcps:
      - github-readonly
      - docs-search

  implementer:
    target: cursor
    model: gpt-5.5-medium
    role: implementer
    rules:
      - repo-conventions
      - tdd-light
    permissions: implementation-safe
    mcps:
      - github
      - linear
      - postgres-readonly

  reviewer:
    target: codex
    model: gpt-5.3-codex
    role: reviewer
    permissions: readonly
    mcps:
      - github-readonly

loop:
  template: implement-review-verify
  verifier:
    command: cargo test
  stopWhen:
    - verifierPasses
    - noBlockingReviewFindings
```

## Roadmap

### Phase 0: Product Specification

Goal: define the product direction without changing runtime behavior.

Major features:

- Write the `agent-profile` product spec and roadmap.
- Define the initial vocabulary: profile, capability block, runtime target,
  agent role, loop topology, policy, registry, approval.
- Document the gap between provider-native profiles and cross-agent profiles.
- Identify the first supported targets: Claude Code, Cursor, and Codex.

Exit criteria:

- The repository has a clear product direction.
- The first schema draft can be reviewed before implementation.

### Phase 1: Local Profile Schema and CLI Skeleton

Goal: make profiles real as local files.

Major features:

- Add `.agent-profile/profiles/*.yaml` as the default local profile location.
- Add a JSON Schema or Rust validation layer for profiles.
- Add commands:
  - `agent-profile list`
  - `agent-profile show <name>`
  - `agent-profile validate <name>`
  - `agent-profile diff <a> <b>`
- Support reusable capability blocks under `.agent-profile/capabilities/`.
- Add profile examples for:
  - `local/fix-ci`
  - `local/readonly-review`
  - `local/implement-review`

Design notes:

- Validation should detect unknown agents, missing capability blocks,
  unsupported targets, duplicate MCP names, and unsafe permission combinations.
- Diff output should be human-readable first, machine-readable later.

Exit criteria:

- Users can define and validate local profiles.
- No provider config is written yet.

### Phase 2: Dry-run Rendering

Goal: show exactly what a profile would change.

Major features:

- Add `agent-profile render <name> --target <target>`.
- Render provider-specific configuration to stdout or a temporary directory.
- Start with:
  - Claude Code settings and MCP fragments.
  - Cursor MCP and permissions fragments.
  - Codex profile and `AGENTS.md` overlay fragments.
- Add provenance comments or sidecar metadata explaining which profile block
  produced each rendered setting.
- Add `agent-profile doctor <name>` to report missing commands, missing MCP
  server binaries, missing environment variables, and target incompatibilities.

Design notes:

- Rendering should be dry-run by default.
- The tool should never overwrite existing provider files silently.
- The renderer should prefer fragments or patches over owning whole files when
  the provider supports it.

Exit criteria:

- Users can see a complete provider-specific plan before applying it.

### Phase 3: Local Apply and Switch

Goal: make profile switching useful for one developer.

Major features:

- Add `agent-profile use <name>`.
- Add backup and restore for touched provider files.
- Add `agent-profile current`.
- Add `agent-profile reset`.
- Support explicit apply modes:
  - `--dry-run`
  - `--write`
  - `--backup`
  - `--no-backup`
- Add lockfile metadata under `.agent-profile/state.json`.

Design notes:

- Applying a profile should be reversible.
- A profile should record the active environment and target adapters.
- If a provider file has drifted since the last apply, the tool should stop and
  ask for explicit reconciliation.

Exit criteria:

- A single developer can switch between home, office, review, and implementation
  profiles safely.

### Phase 4: Loop Integration

Goal: make profiles drive `agent-loop` execution.

Major features:

- Allow `agent-loop` specs to reference a profile:

  ```yaml
  profile: office/looping-engineering/fullstack
  ```

- Support `agent-loop loop run <template> --profile <name>`.
- Resolve agent adapter, model, permissions, MCP bundle, worktree strategy,
  verifier, and budgets from the selected profile.
- Add profile metadata to every journal and markdown report.
- Add per-agent run journals for multi-agent loops.
- Add loop topology templates:
  - `fix-ci`
  - `implement-review-verify`
  - `qa-regression`
  - `design-to-implementation`

Design notes:

- Profile resolution should happen once at loop start and be recorded as an
  immutable run snapshot.
- A running loop should not change behavior if the source profile file changes.

Exit criteria:

- Profiles are not just setup helpers; they become the launch contract for
  Looping Engineering runs.

### Phase 5: Shared Registry

Goal: let teams share approved profiles and capability blocks.

Major features:

- Add registry support:
  - local filesystem registry
  - git registry
  - later HTTP registry
- Add commands:
  - `agent-profile registry add`
  - `agent-profile registry sync`
  - `agent-profile registry search`
  - `agent-profile pull`
  - `agent-profile publish`
- Support signed or checksum-pinned profile versions.
- Add semantic versioning for profiles and capability blocks.
- Add ownership metadata: team, maintainer, review status.

Design notes:

- Registry content should remain plain files.
- Teams should be able to vendor a profile into a repo for stability.

Exit criteria:

- A team can share profile bundles without copy-pasting config files.

### Phase 6: Policy Enforcement

Goal: make profiles safe for organizations.

Major features:

- Add policy files that constrain what profiles may declare.
- Support policy rules for:
  - allowed providers and models
  - allowed MCP servers
  - required approval modes
  - forbidden shell commands
  - secret source restrictions
  - maximum autonomy budgets
  - environment-specific restrictions
- Add `agent-profile policy-check <name>`.
- Add required policy checks before `use` and before loop launch.
- Support managed policy overlays for office and CI environments.

Design notes:

- Policy should fail closed.
- Local personal profiles can be flexible, but team/enterprise profiles should
  be explicitly approved.
- The policy engine should explain violations in terms users can fix.

Exit criteria:

- Organizations can prevent unsafe profile combinations before agents run.

### Phase 7: Audit Logs and Run Evidence

Goal: provide accountability for profile-driven agent loops.

Major features:

- Record profile version, rendered target config, policy decisions, and approval
  decisions in every run journal.
- Add tamper-evident audit records for team profiles.
- Add audit inspection commands.
- Add export formats:
  - JSONL
  - markdown
  - SARIF-like security report
  - CI artifact bundle
- Link tool usage, verifier results, review findings, and final changes back to
  the profile snapshot.

Design notes:

- Audit should be useful for humans first: what ran, why it was allowed, what it
  changed, and how it was verified.
- Machine-readable exports should support later dashboards.

Exit criteria:

- Teams can review not only what an agent changed, but which profile allowed it
  to act.

### Phase 8: Profile Approval Workflow

Goal: make high-autonomy profiles reviewable before use.

Major features:

- Add profile states:
  - draft
  - proposed
  - approved
  - deprecated
  - blocked
- Add approval metadata:
  - approvers
  - approval timestamp
  - policy version
  - profile hash
  - allowed environments
- Add commands:
  - `agent-profile propose`
  - `agent-profile approve`
  - `agent-profile deprecate`
- Require approval for profiles that include high-risk capabilities, such as:
  - write access to production systems
  - unrestricted shell
  - broad MCP auto-run
  - long unattended loops
  - secret-bearing environments

Design notes:

- Approval should bind to the profile hash, not just the profile name.
- Any material change should require reapproval.

Exit criteria:

- Teams can safely promote local profiles into approved shared workflows.

### Phase 9: Enterprise Control Plane

Goal: centralize discovery, policy, audit, and approvals without breaking local
developer ergonomics.

Major features:

- Hosted or self-hosted registry service.
- Web UI for profile search, review, approval, and audit.
- SSO-backed identity and team membership.
- Central policy distribution.
- MCP gateway integration.
- Usage dashboards by profile, agent role, model, MCP server, and repository.
- Drift detection between local rendered config and approved profile state.

Design notes:

- Enterprise control should extend the local file model.
- The CLI should remain the source of truth for developer workflows.
- The server should help teams govern and observe, not force every loop through
  a remote dependency.

Exit criteria:

- Organizations can standardize multi-agent workflows across teams while keeping
  the local CLI useful for day-to-day work.

## Initial CLI Surface

```bash
agent-profile list
agent-profile show <name>
agent-profile validate <name>
agent-profile diff <a> <b>
agent-profile render <name> --target cursor
agent-profile doctor <name>
agent-profile use <name>
agent-profile current
agent-profile reset

agent-loop loop run fix-ci --profile local/fix-ci
agent-loop loop run implement-review-verify --profile office/fullstack
```

## Major Feature Areas

### Profile Authoring

- YAML profile schema.
- Capability block imports.
- Example profile library.
- Validation and linting.
- Human-readable diffs.

### Provider Rendering

- Claude Code adapter.
- Cursor adapter.
- Codex adapter.
- Later Continue, Aider, Cline-style tools, CrewAI, and LangGraph adapters.
- Render provenance.
- Safe apply and rollback.

### MCP and Tool Management

- Named MCP bundles.
- Environment-specific MCP enablement.
- Tool allowlists.
- MCP gateway compatibility.
- Local command and binary checks.

### Permissions and Autonomy

- Read-only, implementation-safe, QA, design, release, and unrestricted bundles.
- Approval mode mapping per provider.
- Autonomy budgets.
- Stop conditions.
- Safety summaries before launch.

### Multi-agent Looping

- Agent team definitions.
- Role-specific model/tool/permission settings.
- Handoff contracts.
- Per-agent journals.
- Loop topology templates.
- Immutable profile snapshots per run.

### Team and Enterprise Governance

- Shared profile registry.
- Policy checks.
- Profile approval.
- Audit logs.
- Drift detection.
- Usage reporting.

## Risks and Mitigations

### Provider config drift

Providers will keep changing config formats. Keep adapters thin, versioned, and
well-tested. Treat rendered output as an adapter concern, not a schema concern.

### Unsafe abstraction

A profile layer can hide risk if the UI is too magical. Mitigate this with
dry-run rendering, diffs, doctor checks, explicit safety summaries, and
reversible apply.

### Over-scoping the MVP

The product can become too broad if it starts with every provider and framework.
Start with local profiles, validation, dry-run rendering, and the three most
important targets: Claude Code, Cursor, and Codex.

### Secret leakage

Profiles should reference secret sources, not contain secret values. Validation
should detect literal secrets and block publishing when suspicious values appear.

### Registry lock-in

Team and enterprise features should use the same file schema as local mode.
Registries should distribute files and metadata, not trap users behind a server.

## Recommended Next Steps

1. Define the initial profile schema in `docs/schema.md`.
2. Add example local profiles under `examples/profiles/`.
3. Implement `agent-profile validate`.
4. Implement dry-run rendering for one target, preferably Codex because it has
   named profiles and a compact `config.toml` surface.
5. Add Cursor and Claude Code renderers.
6. Wire `--profile` into `agent-loop` once validation and dry-run rendering are
   stable.

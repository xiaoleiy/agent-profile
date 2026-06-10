# agent-profile — Product Investigation & Analysis

Status: 2026-06-10. Output of a multi-agent market validation. Business verdict:
**PARK**. What survives is a narrow OSS probe — this document scopes that probe.
agent-profile is a free Rust CLI utility for Looping Engineering practitioners,
explicitly **not** a control plane, registry, or business.

One sentence: **a spawn-time role-profile resolver** — one declarative YAML role
spec, rendered (dry-run / diff / doctor, reversibly applied) into Claude Code
subagent/teammate config and Codex agent TOML at the moment an orchestrator
spawns a worker.

---

## 1. Target users

### Primary persona: the Looping Engineering practitioner

An engineer who runs multi-agent teams of CLI coding agents (Claude Code
subagents/teammates, Codex CLI workers) via an orchestrator — agent-loop, CAO
(awslabs/cli-agent-orchestrator), crew-code, or hand-rolled scripts. They:

- spawn workers with distinct roles (planner, implementer, reviewer, qa) and
  want each role to get the right model, tool allowlist, MCP servers,
  permission mode, and context docs — *per worker, at spawn time*;
- have hit the Claude Code Agent Teams gap firsthand: teammates do not honor
  `skills`/`mcpServers` frontmatter, and there is no per-teammate
  `permissionMode` at spawn (#23669, #24505);
- often run a mixed fleet (Claude Code + Codex) and resent maintaining the same
  role intent in two encodings (JSON/Markdown frontmatter vs TOML);
- are comfortable with YAML, diffs, and dotfile hygiene; allergic to hosted
  anything.

### Secondary personas

- **Orchestrator authors** — maintainers of CAO/crew-code/agent-loop-class
  tools who want a leaf utility to call (`agent-profile render --role reviewer
  --target claude-teammate`) instead of embedding per-provider config logic.
- **Codex-leaning teams** — small teams who want role configs versioned in a
  repo because Codex has no good org-distribution story for agent config
  (surgical `config.toml` merges are hostile to hand maintenance).
- **Dotfile-driven solo engineers** with elaborate subagent setups who want
  `doctor`-style validation that their profile fields are actually supported
  by the installed CLI version.

### Explicitly NOT targets

- Engineers wanting **scenario switching** (home/office/client machine
  profiles) — cc-switch owns that, free. Parked v1 framing; out of scope.
- **Platform/security teams** wanting policy, audit, approval workflows — no
  buyer interviews done; nothing here is for them yet.
- **Cursor users** — Cursor exposes no per-role spawn primitive to target.
- Anyone wanting an **orchestrator**. agent-profile never spawns, schedules,
  or supervises agents. It is a leaf utility orchestrators call.
- Anyone wanting a **profile marketplace/registry** — no demand evidence.

---

## 2. Core value proposition

### The issue we fix

**Per-worker capability provisioning at spawn time.** Today the moment an
orchestrator spawns a worker is exactly the moment with the least config
leverage:

1. **Verified Agent Teams gap (Claude Code).** Teammate frontmatter for
   `skills` and `mcpServers` is ignored; there is no per-teammate
   `permissionMode` at spawn. Roles that need different MCP servers or
   permission postures cannot be expressed natively.
2. **Codex org/repo distribution weakness.** Codex role intent lives in
   `~/.codex/config.toml` — a file Codex itself rewrites (trust ledgers,
   marketplace revisions, hook hashes). There is no clean way to keep
   declarative per-role config in a repo and merge it in safely.
3. **Mixed-fleet inconsistency.** The same role concept ("reviewer:
   read-only, high-reasoning model, github-readonly MCP") must be hand-encoded
   twice in divergent grammars (`Bash(cmd:*)` vs `prefix_rule(...)`;
   JSON `mcpServers` vs TOML `[mcp_servers.*]`). Drift is silent.
4. **No preflight validation anywhere.** No existing tool checks a profile
   against the *installed CLI version's actually-supported fields*. Vendors
   silently ignore unknown frontmatter; users learn at runtime. `doctor` is
   differentiated DX nobody ships.

### Why existing options don't cover it — and where they honestly do

- **Native subagent frontmatter (Claude Code `~/.claude/agents/*.md`)** —
  covers single-session subagents reasonably well (model, tools, prompt).
  Does NOT cover teammates (the gap above), Codex, or preflight validation.
  If Anthropic closes #23669/#24505, the Claude half of our pitch shrinks to
  doctor + mixed-fleet consistency. We say so plainly.
- **wshobson/agents and similar agent-pack repos** — cover *content* (good
  role prompts, many of them). They are static Markdown libraries: no
  rendering, no Codex target, no spawn-time apply/teardown, no validation.
  Complementary — a profile can reference their prompts as context fragments.
- **awslabs/cli-agent-orchestrator (CAO), crew-code, agent-loop** — cover
  *orchestration* (spawning, sequencing, supervision). They each embed ad-hoc
  per-provider config plumbing; none expose a reusable role-resolution layer.
  They are our integration targets and most plausible users, not competitors.
- **cc-switch** — covers machine/scenario profile switching for Claude Code,
  free and established. It is whole-environment, not per-worker/per-spawn,
  and Claude-only. We deliberately ceded that ground.
- **Plugins/marketplaces (Claude plugins, `name@marketplace`)** — cover
  capability *distribution* (skills, MCP servers, hooks as installable
  units). They do not bind capabilities to roles or to a spawn event. A
  profile references installed plugins; we never replace the installer.

What we add that none of them have: one YAML role spec → rendered, diffable,
doctor-validated, reversibly applied per-worker config, for two runtimes, at
the one moment orchestrators need it.

---

## 3. How it works

### Short term — the MVP probe (spawn-time resolver flow)

Scope = roadmap Phase 1 + Phase 2 + a thin slice of Phase 3 + the Phase 4
interface, narrowed to roles and two targets.

1. **Author** a role profile in `.agent-profile/profiles/*.yaml`: per-role
   `mcpServers`, tools allowlist, model, `permissionMode`, context-doc
   fragments (CLAUDE.md/AGENTS.md overlays), skills references. We reference
   the existing SKILL.md standard and existing MCP server definitions — we do
   not invent a skills format or an MCP bundle format.
2. **Validate**: `agent-profile validate` (schema) and `agent-profile doctor
   --role reviewer --target claude-teammate` (probe the installed CLI version;
   flag fields it will silently ignore, missing MCP binaries, missing env).
3. **Render**: `agent-profile render --role reviewer --target
   claude-teammate` — dry-run by default, provenance noted. Exactly two
   targets: Claude Code subagent/teammate config and Codex agent TOML
   (surgical key merge, never full-file ownership — see
   `docs/research/provider-config-surfaces.md`).
4. **Diff**: `agent-profile diff` against what's on disk.
5. **Apply/teardown, spawn-scoped**: apply a rendered role config for a
   specific worker session; tear it down after. Reversible, backed up,
   restricted to spawn-time — this is the wedge into the Agent Teams gap,
   not a general config manager.
6. **Orchestrator hook recipe**: documented one-liner so CAO / crew-code /
   agent-loop call agent-profile at spawn. agent-profile stays a leaf.

Rust CLI (clap). TUI/GUI deferred; Tauri 2 reserved for a possible future GUI
phase. No hosted component, no accounts, no telemetry, no per-seat anything.

### Long term — conditional futures, gated on the metrics (§6)

- **Most likely outcome (stated up front): fold into agent-loop** as an
  internal module if the probe fails. The code is written to make that cheap.
- **Loop integration deepening** (roadmap Phase 4) — only if an orchestrator
  integration or the spawn-time issue volume materializes.
- **Registry/sharing** (Phase 5) — only on concrete demand evidence (users
  asking to share profiles, not us projecting it).
- **Policy/audit/governance** (Phases 6–8) — only after real buyer
  interviews. Currently zero.
- **More render targets** — Cursor only if it ever ships a per-role spawn
  primitive; others only on ≥3 independent user requests (the Codex bar).

---

## 4. Competitive landscape

| Option | Covers | Doesn't cover | Relationship |
|---|---|---|---|
| Native Claude Code subagent frontmatter | Single-session subagent model/tools/prompt | Teammates (skills/mcpServers ignored, no per-teammate permissionMode), Codex, validation | Substrate we render into; vendor may absorb us |
| wshobson/agents (agent packs) | Role prompt content, breadth | Rendering, spawn-time apply, Codex, validation | Complementary content source |
| awslabs/cli-agent-orchestrator (CAO) | Orchestration: spawn/sequence/supervise | Reusable per-role config resolution | Integration target, prospective caller |
| crew-code / agent-loop | Same as CAO; agent-loop is our own | Same | Integration target; agent-loop is the fold-back destination |
| cc-switch | Machine/scenario profile switching (Claude), free | Per-worker spawn-time provisioning, Codex, doctor | Ceded its ground; not competing |
| Claude plugins / marketplaces | Capability distribution (skills, MCP, hooks) | Binding capabilities to roles at spawn | Referenced, never replaced |
| Codex native `[profiles.*]` | Flat model/approval overrides via `--profile` | Per-role MCP/tools composition, repo distribution, cross-runtime | Render target detail |

---

## 5. Risks and what the probe does about each

| Risk | Detail | Probe response |
|---|---|---|
| Vendor absorption | Anthropic's measured close cadence on gaps like this is ~7 weeks; #23669/#24505 could land natively | Treated as a success-metric condition, not a surprise (§6). Ship fast, keep Codex + doctor + mixed-fleet value that survives a native fix. If closed before traction: fold into agent-loop, declare probe answered |
| Free-incumbent overlap | cc-switch (switching), wshobson (content), plugins (distribution) are free and adjacent | Hard non-goals carve them out explicitly; README and this doc name them and route users to them for their use cases |
| Adapter churn concentration | Both targets rewrite their own config files (Codex `config.toml` churn is severe; `~/.claude.json` tool-owned) — adapters are the whole maintenance cost | Only two adapters; surgical merges per the field survey; `doctor` doubles as a churn detector; reversible apply with backups limits blast radius |
| Zero monetization | Validation found no buyer; this cannot become a business as scoped | Accepted by design: it's an OSS probe with falsifiable kill criteria and a named fold-back path, not a startup. No infra costs to strand |

---

## 6. 90-day falsifiable success metrics

The probe **succeeds** only if, within 90 days of public release:

1. **Demand**: ≥50 GitHub stars **and** ≥5 non-author issues/PRs describing
   the spawn-time use case in their own words — **or** one real orchestrator
   integration (CAO, crew-code, or external agent-loop user calling
   agent-profile at spawn).
2. **Window stays open**: Anthropic has not natively closed the Agent Teams
   gap (#23669 / #24505) first. Measured vendor close cadence is ~7 weeks, so
   this is a live coin-flip, monitored weekly.
3. **Codex pull**: ≥3 independent users request or use the Codex target
   (evidence the mixed-fleet story is real, not projected).

**Failure** on these terms → stop, fold the code into agent-loop as an
internal module, write up what was learned. No pivot, no zombie maintenance.

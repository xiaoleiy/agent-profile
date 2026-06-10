# Probe Scorecard

agent-profile is a **90-day falsifiable probe**, not a committed product. This
file is the scorecard. It is updated by hand (the binary makes zero network
calls and carries no telemetry — all signals below are public GitHub artifacts
or manual observations).

Probe clock starts at the v0.1.0 release announcement.

- **T0 (release):** 2026-06-10
- **T+30 checkpoint:** 2026-07-10
- **T+60 checkpoint:** 2026-08-09
- **T+90 decision:** 2026-09-08

## The three metrics (pre-committed, from the 2026-06-09/10 validation)

### M1 — External adoption

≥50 GitHub stars **and** ≥5 issues/PRs from non-author users describing the
spawn-time role-provisioning use case in their own words, **or** one existing
orchestrator (CAO, crew-code, ide-agent-kit, ruflo, an agent-loop external
user) integrating the render CLI.

| Checkpoint | Stars | Non-author issues/PRs on-topic | Orchestrator integrations | Notes |
|---|---|---|---|---|
| T+30 | | | | |
| T+60 | | | | |
| T+90 | | | | |

### M2 — Gap survival vs vendor cadence

Anthropic does **not** ship native per-teammate skills/mcpServers/permissionMode
(closing claude-code #23669 / #24505) before M1's adoption bar is reached.
Measured close rate on the adjacent gap (#24316) was ~7 weeks — this is a live
clock. Record the date either way.

Watch at every Claude Code release:

| Item | Status at T0 | T+30 | T+60 | T+90 |
|---|---|---|---|---|
| anthropics/claude-code#23669 (per-teammate workdir/CLAUDE.md/MCP) | open | | | |
| anthropics/claude-code#24505 (per-teammate hooks) | open | | | |
| anthropics/claude-code#24316 (subagent defs as teammates) | partially shipped (tools+model honored; skills/mcpServers not) | | | |
| Agent Teams docs "Limitations" section | skills/mcpServers frontmatter not applied; no per-teammate permission mode at spawn | | | |
| Codex org-level agent distribution | per-user TOML only, no registry/pull | | | |

### M3 — Heterogeneous-fleet demand

≥3 independent users request or use the **Codex target** (or ask for a mixed
Claude+Codex fleet workflow) within 90 days. Zero such requests falsifies the
cross-vendor premise entirely.

| Checkpoint | Codex-target requests/uses (link each) | Mixed-fleet asks |
|---|---|---|
| T+30 | | |
| T+60 | | |
| T+90 | | |

## Pre-committed decision rule (T+90, or earlier if M2 falsifies)

- **All three metrics met** → continue: deepen loop integration; registry
  (roadmap Phase 5) becomes discussable only if users explicitly ask to share
  profiles across machines/teams; policy/audit (Phase 6) stays gated on 5–10
  platform-engineering/security buyer interviews.
- **Any metric falsified** → **fold**: the code retires into agent-loop as its
  internal role-bundling module (per the agent-loop + agent-profile merger
  decision), the repo is archived with a closing note, and the idea is parked
  permanently. No pivots, no extensions, no "one more month".

Honest prior recorded at T0: the most likely outcome is the fold, with
reputation and a reusable loop component as the return.

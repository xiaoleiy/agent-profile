# agent-profile

**One YAML role spec → provider-native agent config, at spawn time.**

agent-profile is a spawn-time role-profile resolver: a free Rust CLI that
renders one declarative role profile (model, tools allowlist, permission
mode, MCP servers, skills, instructions) into Claude Code subagent/teammate
config or Codex agent TOML at the moment your orchestrator spawns a worker —
and reverses it cleanly when the worker exits.

It is a leaf utility your orchestrator calls. It is deliberately **not** a
control plane, registry, scenario switcher, or business (see
[Non-goals](#non-goals)).

Free & open source · zero network calls · no accounts, no telemetry ·
dry-run by default.

## The problem

Orchestrators (CAO, crew-code, agent-loop, your shell script) got good at
spawning workers. Nothing got good at provisioning them:

1. **Claude Code teammates don't honor the config you give them.** Teammate
   frontmatter ignores `skills` and `mcpServers`, and there is no
   per-teammate `permissionMode` at spawn (claude-code issues #23669,
   #24505). Your reviewer teammate gets the same blast radius as your
   implementer.
2. **Every provider speaks a different permission grammar.**
   `Bash(git diff:*)` in Claude, `prefix_rule(...)` in Codex. Run a mixed
   fleet and you maintain the same role twice, by hand, and they drift.
3. **Config files are shared mutable state.** `.mcp.json`,
   `settings.local.json`, `~/.codex/config.toml` — files the CLIs themselves
   rewrite, that secrets leak into. Editing them per-spawn means surgical
   merges and guaranteed reversal, or it means breakage.
4. **Nothing validates a profile against the CLI you actually have
   installed.** Vendors ship weekly; frontmatter fields appear and vanish.
   You find out a field stopped being honored when a worker silently runs
   with the wrong permissions.

agent-profile fixes per-worker capability provisioning at spawn time: one
YAML role spec → rendered, diffable, doctor-validated, reversibly applied
per-worker config, for two runtimes, at the one moment orchestrators need
it. If Anthropic closes the Agent Teams gap natively, the Claude half of the
pitch shrinks to doctor + mixed-fleet consistency — we say so plainly.

## Install

Until the v0.1.0 release lands on Homebrew and crates.io (sprint S6), build
from source:

```bash
git clone https://github.com/xiaoleiy/agent-profile
cd agent-profile
cargo install --path .       # or: cargo build --release
agent-profile --version      # agent-profile 0.1.0 (<git sha>)
```

Planned distribution (S6): `brew install xiaoleiy/tap/agent-profile` and
`cargo install agent-profile`.

Shell completions:

```bash
agent-profile completions zsh  > ~/.zfunc/_agent-profile   # bash/zsh/fish
```

## Five-minute quickstart

In any git repo (or try it on this repo's `examples/` first — see below):

```bash
mkdir -p .agent-profile/profiles
$EDITOR .agent-profile/profiles/reviewer.yaml   # see the schema below
agent-profile validate --role reviewer
# ✓ reviewer: schema OK, 2 capability blocks resolved, no secret literals

agent-profile render --role reviewer --target claude-subagent   # dry-run, always
agent-profile doctor --role reviewer --target claude-teammate   # preflight vs the INSTALLED CLI
agent-profile apply  --role reviewer --target claude-teammate --session-id run1-reviewer
# … spawn your worker …
agent-profile teardown --session-id run1-reviewer
```

A role profile is plain YAML — no new skills format (SKILL.md is the
standard), no new MCP bundle format (server definitions keep their existing
shape), secrets only by `${env:…}` reference (literals fail `validate`):

```yaml
# .agent-profile/profiles/reviewer.yaml
apiVersion: agent-profile/v1
name: reviewer
description: Read-only code reviewer for implement-review loops.
role: reviewer
targets: [claude-subagent, claude-teammate, codex-agent]
include: [mcp-github-readonly, perms-readonly]   # reusable capability blocks

model:
  claude: claude-fable-5
  codex: gpt-5.3-codex
  effort: high
permissionMode: readonly

tools:
  allow: [Read, Grep, "Bash(git diff:*)", "Bash(git log:*)"]
  deny:  [Write, Edit]

mcpServers:
  github-readonly:
    command: github-mcp
    args: ["--readonly"]
    env: { GITHUB_TOKEN: "${env:GITHUB_PAT_RO}" }

skills: [code-review]                # resolves ~/.agents/skills/<name>/SKILL.md
context: [fragments/reviewer-instructions.md]
```

Try everything against the checked-in examples without writing a profile:

```bash
cargo run -- validate --dir examples/.agent-profile
cargo run -- show --role reviewer --resolved --dir examples/.agent-profile
cargo run -- render --role implementer --target codex-agent --dir examples/.agent-profile
scripts/smoke.sh    # the whole cycle, all targets, sandboxed HOME
```

See [`examples/README.md`](examples/README.md) for the full walkthrough.

## How it works

- **`render` is dry-run by default** — there is no `--dry-run` flag to
  forget. The only ways to touch disk are `--out <sandbox-dir>` and `apply`.
- **`apply` is spawn-scoped by construction**: `--session-id` is mandatory,
  every mutation is atomic (write-then-rename), backed up, and recorded with
  sha256 hashes in `.agent-profile/state.json`. **`teardown`** replays the
  session in reverse — key-level, so foreign edits made while the session was
  active survive. apply→teardown round-trips the repo byte-identically.
- **Drift refuses, never clobbers** (exit 3): an owned file without our
  provenance header, a merge-key collision, or a post-apply edit stops the
  command; `--force` is the human-decided override and still backs up first.
- **A hardcoded never-touch denylist** (`~/.claude.json`, `auth.json`,
  plugin registries, caches/history) sits below the adapters; no flag
  bypasses it, and the executor re-checks per action right before mutating.
- **Anything a target cannot express is reported as `SKIPPED`** with the
  nearest workaround named. Nothing is silently dropped.
- **Two runtimes, three targets, deliberately** (grounded in a
  [field survey](docs/research/provider-config-surfaces.md) of what each CLI
  reads, rewrites, and ignores):
  - `claude-subagent` — a single generated `.claude/agents/<role>.md`.
  - `claude-teammate` — routes each profile section to the nearest surface
    Claude Code *does* honor: agent .md (owned) + `.mcp.json` merge (only
    keys we add) + `settings.local.json` merge (allow/deny/defaultMode) +
    skill symlinks into `~/.agents/skills/` + a marked `@include` block in
    `CLAUDE.md`. *Honest limitation, printed in render output:*
    settings.local.json is project-wide, not per-teammate — give each worker
    its own worktree (orchestrators already do) and project scope ≈ worker
    scope.
  - `codex-agent` — owned `.codex/agents/<role>.toml` + surgical
    `[mcp_servers.*]` merge into `config.toml` via toml_edit (Codex rewrites
    that file constantly; we never own it) + marked block in `AGENTS.md`;
    `Bash(...)` rules translated to `prefix_rule(...)`.

### The orchestrator contract

Two shell calls around the worker lifecycle — concrete recipes for CAO,
crew-code, and agent-loop (trap-based teardown, worktree pattern, jq
parsing) in [`docs/integrations.md`](docs/integrations.md):

```bash
agent-profile apply --role "$ROLE" --target claude-teammate \
  --session-id "$LOOP_ID-$ROLE" --json || exit $?
# … spawn/run the worker …
agent-profile teardown --session-id "$LOOP_ID-$ROLE" --json   # in a trap
```

Exit codes are uniform and scriptable: 0 success · 1 internal error ·
2 validation failure · 3 drift · 4 doctor errors · 5 session/state error.
`--json` envelopes are frozen at `schemaVersion: 1` and golden-tested —
contract + real captured examples in [`docs/json-api.md`](docs/json-api.md).

### Doctor: the preflight check nobody ships

`doctor --role <r> --target <t>` validates a profile against the *installed*
CLI version's actually-supported fields — not against documentation. It
probes `claude --version` / `codex --version` (or trusts
`--assume-version`), then checks a versioned field-support matrix, MCP
`command`/`url` reachability, `${env:VAR}` resolvability, skill resolution,
Codex grammar translatability, and stale sessions. Errors → exit 4;
warnings alone → exit 0, so orchestrators can gate on it.

Stable finding codes (`doctor --json` exposes them as `findings[].code`;
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

## Non-goals

Load-bearing — these define the tool as much as the features do
(design §6, [analysis](docs/product/analysis.md)):

- **Not a registry.** No `publish`, no `pull`, no marketplace, no sharing
  hub. No demand evidence for one, so it doesn't exist.
- **Not a scenario switcher.** Home/office/client machine switching is
  cc-switch's job, and cc-switch is free and good at it. There is no
  persistent `use` mode — apply is spawn-scoped by construction.
- **Not a platform.** No server, no accounts, no telemetry, no per-seat
  anything. The binary makes zero network calls. No policy engine or audit
  trail beyond the local state ledger.
- **Not an orchestrator.** It never spawns, schedules, supervises, or
  monitors agents. CAO, crew-code, agent-loop, your shell script — they
  spawn; agent-profile provisions.
- **Not a new format factory.** SKILL.md is the skills standard; existing
  MCP server definitions are the MCP standard. agent-profile references both
  and invents neither. No Cursor adapter either — Cursor exposes no per-role
  spawn primitive to target.
- **Not a business.** A prior market validation said: don't build a company
  here. This is the sanctioned narrow OSS probe that came out of it, with
  falsifiable kill criteria.

## The probe, stated plainly

This project succeeds or folds on 90-day falsifiable metrics, measured from
the v0.1.0 release announcement:

1. **Demand**: ≥50 GitHub stars **and** ≥5 non-author issues/PRs describing
   the spawn-time use case in their own words — **or** one real orchestrator
   integration calling agent-profile at spawn.
2. **Window stays open**: Anthropic has not natively closed the Agent Teams
   gap (#23669 / #24505) first. Measured vendor close cadence is ~7 weeks,
   so this is a live coin-flip, monitored weekly.
3. **Codex pull**: ≥3 independent users request or use the Codex target.

Failure on these terms → stop, fold the code into an orchestrator as an
internal module (the single-crate, library-first architecture is built for
exactly that), write up what was learned. No pivot, no zombie maintenance.
Either outcome is fine. We'd rather know.

## Status

Sprints 1–5 of the MVP are implemented (plan: `docs/product/sprints.md`;
build contract: `docs/product/design.md`). Working today, end-to-end:

- `validate` / `list` / `show [--resolved]` — schema v1, capability-block
  includes with per-key provenance, secret-literal scanning.
- `render` / `diff` for all three targets, dry-run by default; `diff` exits
  3 on differences (`git diff --exit-code` semantics).
- `doctor` with the versioned field-support matrix and the finding codes
  above.
- `apply` / `teardown` / `current` — the reversible spawn-time cycle:
  atomic mutations, per-session backups, sha256-verified state ledger,
  drift refusal, key-level reversal that preserves foreign edits,
  secret-hygiene gate (`git check-ignore`) for materialized `${env:VAR}`.
- `completions <shell>` (bash/zsh/fish and friends) and `--version` with
  git-sha build metadata.
- `--json` everywhere, frozen at `schemaVersion: 1`, golden-tested against
  `docs/json-api.md` so the documented examples cannot rot.
- Full-cycle smoke test (`scripts/smoke.sh`) in a sandboxed HOME; a real
  captured render/apply/teardown walkthrough in
  [`docs/dogfood.md`](docs/dogfood.md).

CI is live (`.github/workflows/ci.yml`): fmt, clippy `-D warnings`, the full
test suite, the sandboxed smoke cycle, and a zsh-completions load check, on
Ubuntu and macOS. Tag-triggered release builds
(`.github/workflows/release.yml`: macOS arm64/x86_64, Linux x86_64/arm64
tarballs + sha256 checksums) and GitHub Pages deploys of `site/`
(`.github/workflows/pages.yml`) are wired but no release tag has been cut yet.

Not yet done (sprint S6): first tagged release, Homebrew/crates.io
distribution, probe scorecard (`docs/probe.md`).

## Docs

| Doc | What's in it |
|---|---|
| [`docs/json-api.md`](docs/json-api.md) | Frozen `--json` contracts (schemaVersion 1) with real captured envelopes |
| [`docs/integrations.md`](docs/integrations.md) | Orchestrator hook recipes: CAO, crew-code, agent-loop, trap teardown, worktrees, jq |
| [`docs/dogfood.md`](docs/dogfood.md) | A real render→apply→teardown cycle, captured verbatim from a sandbox |
| [`examples/README.md`](examples/README.md) | Walkthrough of the example roles (reviewer/implementer/qa) |
| [`docs/product/design.md`](docs/product/design.md) | The build contract: schema, CLI surface, adapters, safety model |
| [`docs/product/analysis.md`](docs/product/analysis.md) | Market validation: target users, competitive landscape, risks, metrics |
| [`docs/product/sprints.md`](docs/product/sprints.md) | Sprint plan + recorded deviations |
| [`docs/research/provider-config-surfaces.md`](docs/research/provider-config-surfaces.md) | Field survey grounding every adapter file-ownership decision |
| [`docs/roadmap.md`](docs/roadmap.md) | Long-term direction (conditional on the probe metrics) |

## License

MIT.

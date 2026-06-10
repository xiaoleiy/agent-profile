# Example workspace walkthrough

`examples/.agent-profile/` is a complete, validated workspace — the same
fixtures the integration tests and `scripts/smoke.sh` run against. It
follows the design-doc walkthroughs (design §4) and contains:

```text
.agent-profile/
  profiles/
    reviewer.yaml       # read-only reviewer; all three targets; includes 2 blocks
    implementer.yaml    # write access + TDD context; claude-teammate + codex-agent
    qa.yaml             # plan-mode verifier; claude-subagent + claude-teammate
  capabilities/
    mcp-github-readonly.yaml    # reusable MCP server block (env-ref secret)
    perms-readonly.yaml         # readonly posture + tool allow/deny
    models-high-reasoning.yaml  # effort: high
  fragments/
    reviewer-instructions.md    # context fragments referenced by `context:`
    implementer-instructions.md
    qa-instructions.md
```

All commands below run from the repo root of an agent-profile checkout with
`--dir examples/.agent-profile` (or copy the directory into any git repo and
drop the `--dir` flag).

## 1. Validate and inspect (design §4.1)

```bash
cargo run -- validate --dir examples/.agent-profile
# ✓ reviewer: schema OK, 2 capability blocks resolved, no secret literals …

cargo run -- list --dir examples/.agent-profile
cargo run -- show --role reviewer --resolved --dir examples/.agent-profile
# fully merged profile, one `# from: <file>` provenance comment per key
```

Things to try breaking: put a `ghp_…` literal in a capability's `env:` map
(`validate` exits 2 with the secret-literal finding); rename a profile file
without changing `name:` (stem mismatch); add `include:` to a capability
block (one include level only).

## 2. Render and diff — nothing is written (design §4.1, §4.6)

```bash
cargo run -- render --role reviewer    --target claude-subagent --dir examples/.agent-profile
# note the `skipped mcpServers:` line — subagents can't express them; the
# hint names the workaround (claude-teammate)

cargo run -- render --role implementer --target codex-agent     --dir examples/.agent-profile
cargo run -- render --role implementer --target codex-agent \
  --dir examples/.agent-profile --out /tmp/agent-profile-out   # sandbox materialization

cargo run -- diff --role implementer --target codex-agent --dir examples/.agent-profile
echo $?   # 3 = differences exist (script-gate semantics)
```

## 3. Doctor — preflight against your installed CLIs (design §4.3)

```bash
cargo run -- doctor --role reviewer --target claude-teammate --dir examples/.agent-profile
echo $?   # 4 on this fixture is HONEST: e.g. `github-mcp` is not on your
          # PATH and the reviewer skills aren't in your ~/.agents/skills
```

Pass `--assume-version 2.0.34` to skip the CLI probe and check against the
documented field-support matrix.

## 4. The spawn-time cycle (design §4.2, §4.4, §4.5)

Run this in a *scratch* git repo (apply writes provider files there; state
and backups land in `.agent-profile/` of that repo):

```bash
SCRATCH=$(mktemp -d) && cp -R examples/.agent-profile "$SCRATCH/" && cd "$SCRATCH"
git init -q . && printf '.agent-profile/state.json\n.agent-profile/backups/\n' > .gitignore

agent-profile apply --role reviewer --target claude-teammate --session-id run1-reviewer --json
agent-profile current
agent-profile teardown --session-id run1-reviewer
git status --porcelain   # empty: byte-identical round trip
```

To see the drift refusal (design §4.5): create `.claude/agents/qa.md` by
hand first, then `apply --role qa --target claude-teammate
--session-id run1-qa` — exit 3, with the inspect/override hints.

Or let the smoke test do the whole tour (all three targets, sandboxed HOME,
exit codes asserted at every step):

```bash
scripts/smoke.sh
```

## Where to go next

- Orchestrator wiring (trap-based teardown, worktrees, jq):
  [`docs/integrations.md`](../docs/integrations.md)
- A captured real cycle with annotated output:
  [`docs/dogfood.md`](../docs/dogfood.md)
- The frozen `--json` envelopes these fixtures produce:
  [`docs/json-api.md`](../docs/json-api.md)

# Orchestrator integration recipes

agent-profile never spawns anything. The integration contract (design §3.5) is
two shell calls around the worker lifecycle:

```bash
# before spawning the worker
agent-profile apply --role "$ROLE" --target claude-teammate \
  --session-id "$LOOP_ID-$ROLE" --json || exit $?

# … orchestrator spawns/runs the worker in its worktree …

# after the worker exits (success OR failure — put it in a trap)
agent-profile teardown --session-id "$LOOP_ID-$ROLE" --json
```

Everything below is that contract written out concretely. Each recipe was
copy-pasted and run against a sandbox fixture repo (the `examples/` workspace
inside a scratch git repo with HOME overridden) on 2026-06-10 with
agent-profile 0.1.0: apply → worker → teardown verified on both the success
and the failure path, repo byte-clean afterwards (`git status --porcelain`
empty), `current` empty, and the duplicate-session collision verified to exit
5. See also [`docs/dogfood.md`](dogfood.md) for a captured full cycle and
[`docs/json-api.md`](json-api.md) for the exact `--json` envelopes parsed
below.

## Exit codes you gate on

| Code | Meaning | What your orchestrator should do |
|---|---|---|
| 0 | applied / torn down cleanly | proceed |
| 1 | internal error | abort the run, report a bug |
| 2 | validation failure (schema, secret literal, unknown role/target, secret-to-tracked-file refusal) | fix the profile; do not spawn |
| 3 | drift (a file changed underneath; apply/teardown refused) | inspect with `diff`; a human decides on `--force` |
| 4 | doctor found errors | the profile will not behave as declared on this machine |
| 5 | session/state error (duplicate session id, second active session for the same role+target, unknown id, corrupt state.json) | your bookkeeping and agent-profile's disagree — reconcile before spawning |

`apply … --json || exit $?` propagates these to your loop; never swallow
exit 3 or 5 silently.

## The core recipe: trap-based teardown wrapper

This is the shape every orchestrator integration reduces to — a wrapper your
orchestrator uses to launch each worker. Verified against the sandbox fixture
repo (see header).

```bash
#!/usr/bin/env bash
# run-worker.sh — provision → spawn → teardown, teardown guaranteed by a trap.
set -euo pipefail

ROLE="$1"                       # e.g. reviewer
TARGET="$2"                     # e.g. claude-teammate
LOOP_ID="$3"                    # orchestrator-supplied run identifier
SESSION_ID="$LOOP_ID-$ROLE"

# Optional preflight. Exit 4 means the profile will not behave as declared
# on this machine (field not honored by the installed CLI, unresolvable
# skill, untranslatable rule). Gate hard once your profiles pass doctor on
# your fleet; while iterating, set PREFLIGHT=0.
if [ "${PREFLIGHT:-1}" = "1" ]; then
  agent-profile doctor --role "$ROLE" --target "$TARGET" --json > /dev/null || exit $?
fi

# Apply, propagating agent-profile's exit code on failure (3 = drift,
# 5 = session collision, 2 = validation/secret refusal).
APPLY_JSON="$(agent-profile apply --role "$ROLE" --target "$TARGET" \
  --session-id "$SESSION_ID" --json)" || exit $?

# Teardown runs on EVERY exit path — worker success, worker failure, or a
# signal. --force is deliberately absent: post-apply drift should fail
# loudly (exit 3) and reach a human, not be steamrolled by automation.
trap 'agent-profile teardown --session-id "$SESSION_ID" --json' EXIT

BACKUP_DIR="$(printf '%s' "$APPLY_JSON" | jq -r '.backupDir')"
ACTION_COUNT="$(printf '%s' "$APPLY_JSON" | jq '.actions | length')"
echo "provisioned $SESSION_ID: $ACTION_COUNT actions (backups in $BACKUP_DIR)"

# … spawn the worker here, e.g.:
# claude --agent "$ROLE" -p "$TASK_PROMPT"
"${WORKER_CMD[@]}"
```

Notes on the trap:

- `trap … EXIT` is installed *after* apply succeeds — a refused apply (exit
  2/3/5) must not trigger a teardown of a session that never existed.
- If teardown itself exits 3 (a file changed *after* apply — the worker or a
  human edited something we own), the session stays in `state.json` and
  `agent-profile current` will show it. That is deliberate: re-run
  `teardown --session-id … --force` once a human has looked.
- `agent-profile current --json` at loop start is a cheap way to detect
  sessions a crashed previous run left behind; `teardown --all` clears them.

## Parsing the envelopes with jq

All `--json` envelopes are stable (`schemaVersion: 1`, frozen — see
[`docs/json-api.md`](json-api.md)). Hints go to stderr, so stdout is always
one parseable JSON document.

```bash
# Which files did apply touch? (op + path per action)
agent-profile apply --role reviewer --target claude-teammate \
  --session-id "$SID" --json | jq -r '.actions[] | "\(.op)\t\(.path)"'

# Did anything get skipped that I care about?
… | jq -r '.skipped[]? | "\(.field): \(.reason)"'

# Sessions older than my loop's generation (stale-worker sweep):
agent-profile current --json | jq -r --arg loop "$LOOP_ID" \
  '.sessions[] | select(.sessionId | startswith($loop) | not) | .sessionId'

# Doctor: fail only on errors, log warnings (mirrors the exit-code rule):
agent-profile doctor --role qa --target claude-teammate --json \
  | jq -r '.findings[] | select(.level == "warn") | .message' >&2

# Teardown drift handling: list paths a human must look at.
agent-profile teardown --session-id "$SID" --json \
  | jq -r '.drift[]?.paths[]?' || true
```

## Per-worker worktrees (read this before running teammates in parallel)

The honest limitation, carried verbatim from the design (§3.2.3):

> **Honest limitation, documented in `render` output:** settings are
> project-wide, not per-teammate. The session scope means "while this worker
> session is active in this repo/worktree." Orchestrators that need true
> isolation should give each worker its own worktree (which they already do)
> — then project scope ≈ worker scope. This is the best wedge the current
> surface allows; we say so rather than pretend otherwise.

Concretely: `claude-teammate` writes `permissions.defaultMode` into
`<repo>/.claude/settings.local.json`. Two roles whose `permissionMode` maps
to different modes **cannot coexist in one checkout** — the second apply is a
key collision, exit 3, by design. The fix is the pattern orchestrators
already use — one worktree per worker:

```bash
git worktree add "../wt-$ROLE" -b "work/$LOOP_ID-$ROLE"
(
  cd "../wt-$ROLE"
  ./run-worker.sh "$ROLE" claude-teammate "$LOOP_ID"
)
git worktree remove "../wt-$ROLE" --force
```

Verified (2026-06-10, sandbox): `reviewer` and `qa` applied simultaneously to
`claude-teammate` in two worktrees of one repo, no collision, both torn down,
both worktrees byte-clean. This works because `.agent-profile/state.json` and
`backups/` are gitignored, so each worktree carries its own session ledger,
and every repo-scoped file (`.claude/`, `.mcp.json`, `CLAUDE.md`) is
per-worktree.

Caveats that survive worktrees:

- **`~/.codex/config.toml` is shared across worktrees** (the `codex-agent`
  MCP merge is home-scoped). Different roles add different
  `[mcp_servers.*]` keys and coexist; the *same* role applied from two
  worktrees collides on its own key — second apply exits 3. Serialize
  same-role Codex workers, or scope MCP servers per role name.
- `--scope user` targets (`~/.claude/agents/`, `~/.codex/agents/`) are also
  shared. Use repo scope (the default) for parallel workers.
- `state.json` is per-worktree, so `current` in one worktree does not see
  another worktree's sessions. Sweep per worktree, or keep a worktree
  registry in your orchestrator.

## CAO (awslabs/cli-agent-orchestrator)

CAO spawns each agent as a process it supervises. Wrap the worker command it
launches with the core recipe — CAO supplies the role and a unique run id;
agent-profile provisions, CAO spawns, the trap tears down:

```bash
# cao-worker-wrapper.sh — configure CAO to launch workers through this.
#!/usr/bin/env bash
set -euo pipefail
ROLE="${CAO_AGENT_ROLE:?}"            # however your CAO config names the role
SESSION_ID="${CAO_RUN_ID:?}-$ROLE"

agent-profile apply --role "$ROLE" --target claude-teammate \
  --session-id "$SESSION_ID" --json > /dev/null || exit $?
trap 'agent-profile teardown --session-id "$SESSION_ID" --json' EXIT

exec_worker() { "$@"; }
exec_worker "$@"                       # the original CAO worker command line
```

Note `exec` is *not* used: an `exec` would replace the shell and the trap
with it. The wrapper must stay alive as the worker's parent so the trap can
fire.

## crew-code

crew-code-style crews define members with roles; give each member a profile
of the same name under `.agent-profile/profiles/` and provision in the
member's pre-spawn hook (or wrap the member command exactly like the CAO
recipe):

```bash
# pre-spawn (per member)
agent-profile apply --role "$MEMBER_ROLE" --target claude-teammate \
  --session-id "$CREW_RUN_ID-$MEMBER_ROLE" --json || exit $?

# post-exit (per member, must run on failure too — register it as the
# member's cleanup/finally hook, not as a success callback)
agent-profile teardown --session-id "$CREW_RUN_ID-$MEMBER_ROLE" --json
```

Mixed fleets are the point: a crew can send `implementer` to
`--target codex-agent` and `reviewer`/`qa` to `--target claude-teammate`
from the same three YAML files.

## agent-loop

agent-loop runs iterations of implement → review cycles in worktrees. Wire
the two calls into the loop body, one session per worker per iteration:

```bash
for role in implementer reviewer qa; do
  git worktree add "../wt-$role" -b "loop/$LOOP_ID-$role" 2>/dev/null || true
  (
    cd "../wt-$role"
    SESSION_ID="$LOOP_ID-$role"
    target=claude-teammate
    [ "$role" = implementer ] && target=codex-agent   # mixed fleet
    agent-profile apply --role "$role" --target "$target" \
      --session-id "$SESSION_ID" --json > /dev/null || exit $?
    trap 'agent-profile teardown --session-id "$SESSION_ID" --json' EXIT
    run_worker "$role"                # agent-loop's own spawn
  )
done
```

Session ids should encode loop id + role (`loop-42-reviewer`) so a crashed
loop's leftovers are identifiable: `agent-profile current --json | jq` at
loop start, `teardown --session-id <stale>` (or `--all` if the repo is only
used by the loop) before the first apply.

## Things integrations should NOT do

- Do not pass `--force` from automation. Force is the human-decided override
  for inspected drift; in a loop it converts safety refusals into silent
  clobbering.
- Do not parse the human output. Everything machine-readable is in `--json`
  (frozen, `schemaVersion: 1`); human output may change wording.
- Do not reuse session ids across iterations — duplicate ids exit 5.
- Do not gitignore-then-commit `.agent-profile/state.json` or `backups/`;
  they are local mutable session state (apply prints a one-time hint if they
  are not gitignored).
- Do not point two parallel same-role Codex workers at one HOME (see the
  worktree caveats above).

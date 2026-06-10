#!/usr/bin/env bash
# Full-cycle smoke test (sprint S4-T6): validate → doctor → render → diff →
# apply → current → teardown for all three targets, asserting exit codes at
# every step. Runs entirely inside a sandbox (HOME overridden) — it never
# touches the real ~/.claude, ~/.codex, or ~/.agents. Wired for CI; < 60s.
#
# Usage: scripts/smoke.sh [path-to-agent-profile-binary]
set -euo pipefail

cd "$(dirname "$0")/.."
BIN="${1:-}"
if [[ -z "$BIN" ]]; then
  cargo build --quiet
  BIN="$PWD/target/debug/agent-profile"
fi
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
EXAMPLES="$PWD/examples/.agent-profile"

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

# Sandbox the environment: nothing may reach the real provider config.
export HOME="$SANDBOX/home"
export XDG_CONFIG_HOME="$HOME/.config"
unset CLAUDE_CONFIG_DIR CODEX_HOME GITHUB_PAT_RO GITHUB_PAT_RW || true
mkdir -p "$HOME"

REPO="$SANDBOX/repo"
mkdir -p "$REPO"
cp -R "$EXAMPLES" "$REPO/.agent-profile"
cd "$REPO"
git init -q .
printf '.agent-profile/state.json\n.agent-profile/backups/\n' > .gitignore
git add -A
git -c user.name=smoke -c user.email=smoke@local commit -qm baseline

FAILURES=0
expect() {
  local want="$1"; shift
  local got=0
  "$@" > /dev/null 2>&1 || got=$?
  if [[ "$got" -ne "$want" ]]; then
    echo "FAIL (exit $got, want $want): $*" >&2
    FAILURES=$((FAILURES + 1))
  else
    echo "ok   (exit $got) $*"
  fi
}

expect 0 "$BIN" validate

run_cycle() {
  local role="$1" target="$2" session="smoke-$1-$2"
  # Doctor exits 4 in this sandbox: honest findings (unsupported fields,
  # unresolvable skills/commands) — proving the preflight gate works.
  expect 4 "$BIN" doctor --role "$role" --target "$target" --assume-version 2.0.34
  expect 0 "$BIN" render --role "$role" --target "$target"
  expect 3 "$BIN" diff   --role "$role" --target "$target"   # creates pending
  expect 0 "$BIN" apply  --role "$role" --target "$target" --session-id "$session" --json
  expect 0 "$BIN" current
  expect 5 "$BIN" apply  --role "$role" --target "$target" --session-id "$session" --json
  expect 0 "$BIN" teardown --session-id "$session"
  expect 5 "$BIN" teardown --session-id "$session"            # already gone
}

run_cycle reviewer    claude-subagent
run_cycle reviewer    claude-teammate
run_cycle implementer codex-agent

expect 0 "$BIN" current

# The §3.5 contract: after teardown the repo is byte-clean (state/backups are
# gitignored; everything else must be restored exactly).
if [[ -n "$(git status --porcelain)" ]]; then
  echo "FAIL: repo not clean after full cycle:" >&2
  git status --porcelain >&2
  FAILURES=$((FAILURES + 1))
fi
# And the sandbox home carries no leftover provider config either.
LEFTOVER="$(find "$HOME" -type f -not -path "$HOME/.config/*" 2>/dev/null || true)"
if [[ -n "$LEFTOVER" ]]; then
  echo "FAIL: leftover files under sandbox HOME:" >&2
  echo "$LEFTOVER" >&2
  FAILURES=$((FAILURES + 1))
fi

if [[ "$FAILURES" -gt 0 ]]; then
  echo "smoke: $FAILURES failure(s)" >&2
  exit 1
fi
echo "smoke: all checks passed"

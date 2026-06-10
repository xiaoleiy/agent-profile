# agent-profile — MVP Product Design

Status: 2026-06-10. Build contract for the MVP probe scoped in
`docs/product/analysis.md`. Vocabulary follows `docs/roadmap.md` (profile,
capability block, runtime target, agent role), narrowed to the probe scope:
**a spawn-time role-profile resolver** with exactly two runtime targets.

Ground truth for every adapter decision: `docs/research/provider-config-surfaces.md`
(referred to below as "the recon doc"). Where this design and the recon doc
disagree, the recon doc wins.

---

## 1. Profile schema

### 1.1 File layout

```text
<repo>/
  .agent-profile/
    profiles/            # one YAML file per agent role
      reviewer.yaml
      implementer.yaml
      qa.yaml
    capabilities/        # reusable capability blocks (flat, one level)
      mcp-github-readonly.yaml
      perms-readonly.yaml
      models-high-reasoning.yaml
    fragments/           # context-doc fragments referenced by profiles
      reviewer-instructions.md
    state.json           # apply/teardown ledger (created on first apply)
    backups/             # per-session backups of touched provider files
```

A profile's identity is its filename stem (`reviewer.yaml` → role profile
`reviewer`). `name:` inside the file must match the stem; `validate` enforces
this.

### 1.2 Schema (v1)

```yaml
# .agent-profile/profiles/reviewer.yaml
apiVersion: agent-profile/v1        # required; only "agent-profile/v1" accepted
name: reviewer                      # required; must equal filename stem
description: Read-only code reviewer for implement-review loops.   # required
role: reviewer                      # required; free-form role label, surfaced in renders

# Which runtime targets this profile may be rendered for. render/doctor/apply
# reject targets not listed here.
targets: [claude-subagent, claude-teammate, codex-agent]

# Capability block includes. ONE level only: a capability block may not
# include another block. Merge order: blocks in listed order, then the
# profile's own keys win on conflict (scalar replace; maps deep-merge one
# level; lists replace, not concat — predictable beats clever).
include:
  - mcp-github-readonly
  - perms-readonly

model:
  claude: claude-fable-5            # model id passed to Claude targets
  codex: gpt-5.3-codex              # model id for the Codex target
  effort: high                      # optional: low|medium|high
                                    #   claude → "effortLevel", codex → "model_reasoning_effort"

permissionMode: readonly            # one of the profile-level postures:
                                    #   readonly | plan | acceptEdits | auto | unrestricted
                                    # adapters map this per target (see §3); doctor
                                    # warns where a target cannot express it.

tools:
  allow:                            # canonical grammar = Claude rule grammar
    - Read                          #   (Tool, Tool(specifier), Bash(cmd:*), mcp__server__tool)
    - Grep                          # the Codex adapter translates to prefix_rule(...)
    - "Bash(git diff:*)"            # where possible; doctor flags untranslatable rules.
    - "Bash(git log:*)"
  deny:
    - Write
    - Edit

mcpServers:                         # same shapes as Claude/.mcp.json — we do NOT
  github-readonly:                  # invent an MCP bundle format
    type: stdio                     # stdio (default if omitted) | http
    command: github-mcp
    args: ["--readonly"]
    env:
      GITHUB_TOKEN: ${env:GITHUB_PAT_RO}   # secret-source indirection ONLY.
                                           # Literal token-like values fail validate.
  docs-search:
    type: http
    url: https://mcp.example.com/docs

skills:                             # references to existing SKILL.md dirs; we do NOT
  - code-review                     # invent a skills format. Resolution order:
  - verification-before-completion  #   ~/.agents/skills/<name>/SKILL.md  (cross-tool store)
                                    #   then <repo>/.agent-profile/skills/<name>/SKILL.md
                                    # doctor errors if a referenced skill resolves nowhere.

context:                            # instruction fragments layered into the role's
  - fragments/reviewer-instructions.md   # context doc (CLAUDE.md/AGENTS.md overlay or
                                         # subagent system prompt body, per target)

metadata:                           # optional, opaque to rendering; carried into
  owner: xiaolei                    # provenance comments
  tags: [review, readonly]
```

Secret indirection: the **only** accepted forms inside `env` values are
`${env:VAR_NAME}` (resolved from the environment at apply time) and plain
non-secret strings. `validate` runs secret-literal heuristics (§5.4) over all
`env` maps and refuses profiles that embed credentials — the recon doc shows
inline tokens in the wild (`.mcp.json`, Codex `shell_environment_policy.set`);
we must not reproduce that pattern.

### 1.3 Capability blocks

A capability block is a YAML file containing any subset of the profile keys
`model`, `permissionMode`, `tools`, `mcpServers`, `skills`, `context`:

```yaml
# .agent-profile/capabilities/mcp-github-readonly.yaml
apiVersion: agent-profile/v1
kind: capability
name: mcp-github-readonly
mcpServers:
  github-readonly:
    type: stdio
    command: github-mcp
    args: ["--readonly"]
    env:
      GITHUB_TOKEN: ${env:GITHUB_PAT_RO}
```

Rules (kept deliberately minimal):

- One level of include. `include:` inside a capability block is a validation
  error. No inheritance chains, no overrides DSL.
- Conflict resolution is mechanical: later block wins over earlier block,
  profile wins over all blocks; maps deep-merge one level, lists replace.
- `show --role <r> --resolved` prints the fully merged profile with a
  provenance comment per key (`# from: capabilities/mcp-github-readonly.yaml`).

### 1.4 Example profiles

**implementer.yaml**

```yaml
apiVersion: agent-profile/v1
name: implementer
description: Implementation worker with write access and TDD context.
role: implementer
targets: [claude-teammate, codex-agent]
include: [models-high-reasoning]
model:
  claude: claude-fable-5
  codex: gpt-5.5
  effort: medium
permissionMode: acceptEdits
tools:
  allow: [Read, Grep, Glob, Write, Edit, "Bash(cargo test:*)", "Bash(cargo build:*)", "Bash(git:*)"]
  deny: ["Bash(git push:*)"]
mcpServers:
  github:
    type: stdio
    command: github-mcp
    env:
      GITHUB_TOKEN: ${env:GITHUB_PAT_RW}
skills: [test-driven-development]
context: [fragments/implementer-instructions.md]
```

**qa.yaml**

```yaml
apiVersion: agent-profile/v1
name: qa
description: Verification worker — runs tests and browser checks, no source edits.
role: qa
targets: [claude-subagent, claude-teammate]
permissionMode: plan
model:
  claude: claude-fable-5
tools:
  allow: [Read, Grep, "Bash(cargo test:*)", "Bash(npm test:*)", mcp__chrome-devtools__take_snapshot]
  deny: [Write, Edit]
mcpServers:
  chrome-devtools:
    type: stdio
    command: npx
    args: ["chrome-devtools-mcp"]
skills: [verify]
context: [fragments/qa-instructions.md]
```

---

## 2. CLI surface

Single binary `agent-profile` (clap, derive API). Global flags: `--json`
(machine-readable output for orchestrators — every command supports it),
`--dir <path>` (override `.agent-profile/` discovery; default: walk up from
cwd), `-q/--quiet`, `-v/--verbose`.

```text
agent-profile
├── list                              # list role profiles + capability blocks
├── show --role <r> [--resolved]      # print profile; --resolved = after includes, with provenance
├── validate [--role <r>]             # schema + include resolution + secret-literal scan
│                                     #   (all profiles when --role omitted)
├── doctor --role <r> --target <t>    # preflight against the INSTALLED CLI (see §5.3)
├── render --role <r> --target <t>    # dry-run by DEFAULT: prints planned files/merges
│        [--out <dir>]                #   write rendered artifacts to a directory instead
│        [--scope repo|user]          #   where target files would live (default: repo)
├── diff --role <r> --target <t>      # rendered output vs what is on disk right now
│        [--scope repo|user]
├── apply --role <r> --target <t>     # spawn-time apply; REQUIRES --session-id
│        --session-id <id>            #   orchestrator-supplied worker/session identifier
│        [--scope repo|user]
│        [--force]                    #   override drift refusal (records the override)
├── teardown (--session-id <id> | --all)   # reverse a session's actions from state.json
│        [--force]                    #   tear down even if post-apply drift detected
├── current                           # list active sessions from state.json
└── completions <shell>               # shell completions
```

Notes:

- `render` is dry-run by default — there is **no** `--dry-run` flag to forget;
  the only ways to touch disk are `--out` (sandbox directory) and `apply`.
- `apply` always backs up; there is no `--no-backup` in the MVP.
- `--json` output contracts (stable, versioned with `"schemaVersion": 1`):
  - `render --json`: `{ "role", "target", "actions": [ { "path", "op": "create|merge-keys|symlink|append-block", "keys": [...], "content"? } ] }`
  - `apply --json`: same plus `"sessionId"`, `"backupDir"`, per-action `"status"`.
  - `doctor --json`: `{ "target", "cliVersion", "findings": [ { "level": "error|warn|info", "code", "message", "field"? } ] }`
  - `diff --json`: unified diffs per path.

Exit codes (uniform across commands):

| Code | Meaning |
|---|---|
| 0 | Success / no findings / no diff |
| 1 | Internal or I/O error |
| 2 | Validation failure (schema, includes, secret literal, unknown role/target) |
| 3 | Drift detected (apply/teardown refused; diff found differences) |
| 4 | Doctor found errors (warnings alone still exit 0) |
| 5 | Session/state error (unknown session id, corrupt state.json, apply over an active session for the same role+target) |

`diff` exiting 3 on differences makes it usable as a check in scripts
(mirrors `git diff --exit-code` semantics, shifted to our code space).

---

## 3. Render targets

Exactly two runtimes, three target names. Per the recon doc, every file an
adapter touches is classified **own** (we may create/replace it, with backup),
**merge** (surgical key-level edits only), or **never** (hands off).

### 3.0 Ownership table (normative)

| File | claude-subagent | claude-teammate | codex-agent |
|---|---|---|---|
| `<repo>/.claude/agents/<name>.md` | own | own | — |
| `~/.claude/agents/<name>.md` (`--scope user`) | own | own | — |
| `<repo>/.mcp.json` | — | merge (keys we add) | — |
| `<repo>/.claude/settings.local.json` | — | merge (keys we add) | — |
| `<repo>/.claude/skills/<skill>` symlink | — | own (per link) | — |
| `<repo>/CLAUDE.md` | — | append marked block | — |
| `<repo>/.codex/agents/<name>.toml` | — | — | own |
| `~/.codex/agents/<name>.toml` (`--scope user`) | — | — | own |
| `~/.codex/config.toml` / `<repo>/.codex/config.toml` | — | — | merge (only `[mcp_servers.*]` keys we add) |
| `<repo>/AGENTS.md` | — | — | append marked block |
| `~/.claude.json` | never | never | never |
| `~/.claude/settings.json` | never (MVP) | never (MVP) | — |
| `~/.codex/auth.json`, caches, history, plugin registries | never | never | never |

"Own" files we create carry a provenance header
(`<!-- generated by agent-profile vX.Y — role: reviewer — do not hand-edit -->`
or `# generated by agent-profile…` in TOML). If an owned path already exists
and was not generated by us, apply refuses without `--force`, and `--force`
still backs the original up.

### 3.1 `claude-subagent`

Emits a single file: `.claude/agents/<role>.md` (repo scope) or
`~/.claude/agents/<role>.md` (user scope).

```markdown
---
name: reviewer
description: Read-only code reviewer for implement-review loops.
model: claude-fable-5
tools: Read, Grep, Bash(git diff:*), Bash(git log:*)
---
<!-- generated by agent-profile v0.1 — role: reviewer -->
{contents of fragments/reviewer-instructions.md}
{for each skill: "Use the <name> skill (see ~/.agents/skills/<name>/SKILL.md)."}
```

Only fields the installed CLI actually honors in subagent frontmatter are
emitted; `doctor` is the authority on that set per version (§5.3). Fields the
profile declares but the target cannot express (e.g. `mcpServers` on a plain
subagent, `permissionMode` if unsupported) are reported by `render` as
`SKIPPED (target cannot express)` lines — never silently dropped.

### 3.2 `claude-teammate` — the spawn-time injection strategy

This target exists because teammate frontmatter ignores `skills`/`mcpServers`
and there is no per-teammate `permissionMode` at spawn (#23669/#24505). The
adapter routes each profile section to the nearest surface Claude Code *does*
honor, scoped to a session and reversed at teardown:

1. **Agent file** — own `.claude/agents/<role>.md` as in §3.1 (prompt, model,
   description; `tools` line included where honored).
2. **MCP servers** — surgical merge of the profile's `mcpServers` entries into
   `<repo>/.mcp.json` (`mcpServers.<key>` additions only; existing keys we did
   not create are never modified — a name collision is a drift error, exit 3).
   `${env:VAR}` is resolved at apply time into the written value **only if**
   the target file is gitignored; otherwise apply refuses and tells the user to
   keep the env reference pattern Claude itself supports or gitignore the file.
3. **Permissions / permissionMode** — surgical merge into
   `<repo>/.claude/settings.local.json`: `permissions.allow` / `permissions.deny`
   entries appended (tracked individually), `permissions.defaultMode` set from
   `permissionMode`. **Honest limitation, documented in `render` output:**
   settings are project-wide, not per-teammate. The session scope means "while
   this worker session is active in this repo/worktree." Orchestrators that
   need true isolation should give each worker its own worktree (which they
   already do) — then project scope ≈ worker scope. This is the best wedge the
   current surface allows; we say so rather than pretend otherwise.
4. **Skills** — create symlinks `.claude/skills/<skill> → ~/.agents/skills/<skill>`
   for each referenced skill, adopting the existing cross-tool symlink-farm
   convention from the recon doc. Never copy skill content.
5. **Context fragments** — append a marked block to `<repo>/CLAUDE.md`:

   ```markdown
   <!-- agent-profile:begin session=loop-42-reviewer role=reviewer -->
   @.agent-profile/fragments/reviewer-instructions.md
   <!-- agent-profile:end session=loop-42-reviewer -->
   ```

   Using the `@include` stub pattern the instruction files already support
   keeps the diff tiny and teardown unambiguous (delete the marked block).

Every action above is recorded in `state.json` (§3.4) with a backup, so
teardown restores the exact pre-apply state.

### 3.3 `codex-agent`

1. **Agent TOML** — own `<repo>/.codex/agents/<role>.toml` (or
   `~/.codex/agents/<role>.toml` with `--scope user`):

   ```toml
   # generated by agent-profile v0.1 — role: reviewer
   name = "reviewer"
   description = "Read-only code reviewer for implement-review loops."
   model = "gpt-5.3-codex"
   model_reasoning_effort = "high"

   [permissions]
   # translated from canonical grammar; untranslatable rules listed by doctor
   allow = [ "prefix_rule(pattern=[\"git\",\"diff\"], decision=\"allow\")",
             "prefix_rule(pattern=[\"git\",\"log\"], decision=\"allow\")" ]
   ```

2. **MCP servers** — surgical merge of `[mcp_servers.<name>]` tables into the
   relevant `config.toml` via `toml_edit` (preserves comments, formatting, and
   every table we don't own). **Never full-file ownership** — the recon doc
   shows Codex itself rewrites `config.toml` constantly (`[marketplaces.*]`,
   `[projects.*].trust_level`, `[hooks.state.*]`, `[tui.*]`). The adapter
   touches only the exact `mcp_servers` keys it adds and records them by name.
3. **Context fragments** — append the same marked-block `@include` stub to
   `<repo>/AGENTS.md`.
4. **permissionMode mapping** — `readonly`/`plan` → `approval_policy` +
   restrictive rules; `acceptEdits`/`auto` → looser approval keys. The exact
   key set is whatever the installed Codex version supports — `doctor` probes
   it; the adapter emits flat keys, not `[profiles.*]` (native profiles exist
   but are orthogonal to per-role agent files; revisit only on user request).

### 3.4 State model and reversibility

`.agent-profile/state.json`:

```json
{
  "schemaVersion": 1,
  "sessions": {
    "loop-42-reviewer": {
      "role": "reviewer",
      "target": "claude-teammate",
      "scope": "repo",
      "appliedAt": "2026-06-10T09:14:03Z",
      "agentProfileVersion": "0.1.0",
      "actions": [
        { "op": "create",       "path": ".claude/agents/reviewer.md",          "hashAfter": "sha256:…" },
        { "op": "merge-keys",   "path": ".mcp.json",                           "keys": ["mcpServers.github-readonly"],
          "backup": ".agent-profile/backups/loop-42-reviewer/.mcp.json",       "hashBefore": "sha256:…", "hashAfter": "sha256:…" },
        { "op": "merge-keys",   "path": ".claude/settings.local.json",         "keys": ["permissions.allow[+2]", "permissions.defaultMode"],
          "backup": "…", "hashBefore": "…", "hashAfter": "…" },
        { "op": "symlink",      "path": ".claude/skills/code-review",          "linkTarget": "~/.agents/skills/code-review" },
        { "op": "append-block", "path": "CLAUDE.md", "marker": "session=loop-42-reviewer",
          "backup": "…", "hashBefore": "…", "hashAfter": "…" }
      ]
    }
  }
}
```

Teardown replays `actions` in reverse:

- `create` → delete the file (after confirming its hash still equals
  `hashAfter`; mismatch = post-apply drift → refuse without `--force`).
- `merge-keys` → remove exactly the recorded keys; if other keys changed
  meanwhile, those changes are preserved (key-level reversal, not file
  restore). Full file restore from backup only under `--force` when key-level
  reversal is impossible.
- `symlink` → remove the link only if it still points at `linkTarget`.
- `append-block` → delete the marked block by marker, wherever it now sits.

Backups live under `.agent-profile/backups/<session-id>/` and are deleted on
clean teardown. `state.json` and `backups/` belong in the user's `.gitignore`
(`apply` prints a one-time hint if they aren't).

### 3.5 Orchestrator hook recipe

agent-profile never spawns anything. The integration contract is two shell
calls around the worker lifecycle, documented in the README for CAO,
crew-code, and agent-loop:

```bash
# before spawning the worker
agent-profile apply --role "$ROLE" --target claude-teammate \
  --session-id "$LOOP_ID-$ROLE" --json || exit $?

# … orchestrator spawns/runs the worker in its worktree …

# after the worker exits (success OR failure — put it in a trap)
agent-profile teardown --session-id "$LOOP_ID-$ROLE" --json
```

---

## 4. User interaction flows

### 4.1 Define a reviewer role and render it for Claude Code

```text
$ mkdir -p .agent-profile/profiles && $EDITOR .agent-profile/profiles/reviewer.yaml
$ agent-profile validate --role reviewer
✓ reviewer: schema OK, 2 capability blocks resolved, no secret literals

$ agent-profile render --role reviewer --target claude-subagent
PLAN (dry-run — nothing written):
  create  .claude/agents/reviewer.md          (47 lines)
  skipped mcpServers: claude-subagent cannot express per-subagent MCP servers
          → use target claude-teammate, or add servers to .mcp.json yourself
Run `agent-profile render … --out <dir>` to inspect files, or `apply` to write.
```

### 4.2 Orchestrator spawns 3 workers with different profiles

```text
$ for role in implementer reviewer qa; do
    agent-profile apply --role $role --target claude-teammate \
      --session-id run7-$role --json
  done
{"schemaVersion":1,"sessionId":"run7-implementer","status":"applied","actions":5,"backupDir":".agent-profile/backups/run7-implementer"}
{"schemaVersion":1,"sessionId":"run7-reviewer","status":"applied","actions":5,...}
{"schemaVersion":1,"sessionId":"run7-qa","status":"applied","actions":4,...}

$ agent-profile current
ACTIVE SESSIONS
  run7-implementer   implementer → claude-teammate   applied 09:14:03  (5 actions)
  run7-reviewer      reviewer    → claude-teammate   applied 09:14:04  (5 actions)
  run7-qa            qa          → claude-teammate   applied 09:14:05  (4 actions)
```

(Each worker runs in its own worktree, so the project-scope settings caveat in
§3.2.3 collapses to per-worker scope.)

### 4.3 Doctor catches an unsupported field after a CLI update

```text
$ agent-profile doctor --role reviewer --target claude-teammate
claude --version → 3.2.0
FINDINGS
  error  F012  frontmatter field `permissionMode` not honored by claude 3.2.0
               teammate spawn — rendered via settings.local.json defaultMode instead (info)
  warn   F031  mcp server `github-readonly`: command `github-mcp` not found on PATH
  warn   F044  env reference ${env:GITHUB_PAT_RO} is unset in current environment
  ok     skills: 2/2 resolve in ~/.agents/skills/
exit code 4
```

### 4.4 Teardown after a loop run

```text
$ agent-profile teardown --session-id run7-reviewer
  restore  .mcp.json                    (removed key mcpServers.github-readonly)
  restore  .claude/settings.local.json  (removed 2 allow rules, defaultMode)
  delete   .claude/agents/reviewer.md
  unlink   .claude/skills/code-review
  remove   CLAUDE.md block session=run7-reviewer
✓ session run7-reviewer torn down, backups deleted
```

### 4.5 Drift refusal before apply

```text
$ agent-profile apply --role qa --target claude-teammate --session-id run8-qa
✗ drift: .claude/agents/qa.md exists and was not generated by agent-profile
  (no provenance header). Refusing to overwrite.
  → inspect: agent-profile diff --role qa --target claude-teammate
  → override (backs up the file first): apply --force
exit code 3
```

### 4.6 Codex target with surgical TOML merge

```text
$ agent-profile render --role implementer --target codex-agent
PLAN (dry-run — nothing written):
  create      .codex/agents/implementer.toml      (19 lines)
  merge-keys  ~/.codex/config.toml                + [mcp_servers.github]
              (key-level merge via toml_edit; comments/format preserved;
               existing tables untouched)
  append      AGENTS.md                           marked block (3 lines, @include stub)

$ agent-profile diff --role implementer --target codex-agent
--- ~/.codex/config.toml
+++ rendered
+[mcp_servers.github]
+command = "github-mcp"
+[mcp_servers.github.env]
+GITHUB_TOKEN = "${env:GITHUB_PAT_RW}"
exit code 3   # differences exist
```

---

## 5. Error and safety model

### 5.1 Never overwrite silently

- `render` never writes outside `--out`. `apply` is the only command touching
  provider files, always with backup + state record.
- Owned files: refuse if the existing file lacks our provenance header
  (exit 3); `--force` overrides but still backs up and records the override in
  `state.json`.
- Merged files: refuse on key collision with keys we didn't create (exit 3).
- The recon doc's "never" list (`~/.claude.json` wholesale, plugin registries,
  `auth.json`, caches/history/state) is enforced in the adapter layer as a
  hardcoded denylist — no flag bypasses it.

### 5.2 Drift detection

- Before apply: target files hashed and compared against the rendered plan's
  expectations (and against `state.json` if a previous session touched them).
- Before teardown: each action's `hashAfter` re-verified; mismatch (the file
  changed after we applied) → refuse, list the drifted paths, require
  `--force` (which falls back to backup restore for owned files and key-level
  best-effort for merges).
- `diff` is the standalone view of the same computation.

### 5.3 What doctor checks

Doctor is the differentiated DX feature; it validates a profile against the
**installed** CLI, not against documentation:

1. CLI presence + version (`claude --version`, `codex --version`).
2. Field support matrix: a versioned, in-crate table of which frontmatter /
   TOML / settings fields each CLI version honors (updated as vendors ship;
   this table is the adapter-churn early-warning system named in
   `analysis.md` §5). Unknown-to-target profile fields → error with the
   nearest workaround named.
3. MCP server executability: stdio `command` on PATH; http `url` well-formed.
4. Secret references: every `${env:VAR}` resolvable in the current env (warn,
   not error — CI may inject later).
5. Skill resolution: each `skills:` entry has a `SKILL.md` in
   `~/.agents/skills/<name>/` or the repo fallback.
6. Permission grammar translatability (codex-agent): canonical rules that
   cannot map to `prefix_rule(...)` → error listing each.
7. Stale sessions: state.json sessions older than 24h get an info nudge.

Errors → exit 4; warnings alone → exit 0 (orchestrators gate on the code).

### 5.4 Secret-literal detection (validate)

Heuristics over every `env` value and every string in `mcpServers`:
known token prefixes (`ghp_`, `github_pat_`, `sk-`, `xox`, `AKIA`, `glpat-`,
`Bearer `), high-entropy strings ≥ 20 chars matching `[A-Za-z0-9+/_-]+`, and
anything assigned to a key matching `(?i)(token|secret|key|password|credential)`
that is not a `${env:…}` reference. Findings are validation **errors**
(exit 2). False positives are silenced per-value with
`# agent-profile: allow-literal` on the same line — explicit and greppable.

### 5.5 Resolved-secret hygiene at apply

When apply must materialize `${env:VAR}` into a written file (Claude
`.mcp.json` env maps), it requires the target file to be gitignored (checked
via `git check-ignore`), otherwise refuses with the explanation. Backup files
containing resolved secrets live only under `.agent-profile/backups/` (also
required-gitignored) and are deleted on teardown.

---

## 6. Non-goals (load-bearing — do not design these in)

- **Scenario/machine switching** (home/office/client) — cc-switch owns it,
  free. Parked v1 framing.
- **Profile registry/marketplace/sharing** — no demand evidence. No
  `publish`/`pull` commands, no registry config keys reserved.
- **Policy, audit, approval workflows** — gated on buyer interviews that have
  not happened. No policy file format, no audit log beyond `state.json`.
- **Cursor adapter** — Cursor exposes no per-role spawn primitive to target.
- **Hosted anything** — no server component, no accounts, no telemetry, no
  per-seat licensing. The binary makes zero network calls.
- **Orchestration** — agent-profile never spawns, schedules, supervises, or
  monitors agents. It is a leaf utility orchestrators call.
- **New formats** — no skills format (SKILL.md is the standard, referenced),
  no MCP bundle format (existing server-definition shapes, referenced).
- **General-purpose config management** — apply is spawn-scoped by
  construction (`--session-id` is mandatory). There is no `agent-profile use`
  persistent-switch mode in the MVP.

---

## 7. Architecture sketch

Single crate, module-per-concern (cheap to fold into agent-loop as a library
if the probe fails — `cli` is a thin shell over a public `lib.rs` API):

```text
src/
  main.rs            # clap entry, exit-code mapping
  cli/               # command definitions, output formatting (human + --json)
  schema/            # profile + capability structs (serde), include resolution,
                     # validation, secret-literal scan
  adapters/
    mod.rs           # trait RenderTarget { plan(&Profile) -> Vec<Action>; }
    claude.rs        # claude-subagent + claude-teammate (shared plumbing)
    codex.rs         # codex-agent
  state/             # state.json ledger, backups, hashing, drift checks,
                     # apply/teardown executors (Action interpreter)
  doctor/            # CLI probing, field-support matrix (versioned tables)
```

Key dependencies:

- `clap` (derive) — CLI.
- `serde`, `serde_yaml`, `serde_json` — schema + JSON surfaces. JSON merges
  (`.mcp.json`, `settings.local.json`) operate key-level on
  `serde_json::Value`, preserving all keys we don't own (JSON has no comments,
  so re-serialization is lossless there).
- `toml_edit` — the load-bearing choice for Codex: format/comment-preserving
  key-level TOML merges, never full-file rewrites.
- `sha2` — action hashing; `similar` — diffs; `anyhow`/`thiserror` — errors;
  `which` — doctor PATH probes; `tempfile` — atomic write-then-rename for
  every file mutation.

TUI/GUI: out of scope for the MVP. Tauri 2 is reserved for a possible future
GUI phase **only if** the 90-day probe metrics (`analysis.md` §6) are met; no
code in the MVP should anticipate it beyond keeping rendering logic in the
library, not the CLI shell.

# `--json` API contracts (schemaVersion 1)

Status: frozen as of sprint S5. Every `agent-profile` command accepts the
global `--json` flag and prints exactly one pretty-printed JSON envelope to
stdout. Every envelope carries `"schemaVersion": 1`. **Any breaking change to
these shapes bumps `schemaVersion`** (design §2) — additive optional fields do
not.

The sole exception is `completions <shell>`: it always emits the raw shell
completion script (the bytes you source into your shell), never a JSON
envelope, regardless of `--json`. A completion script wrapped in JSON would be
unusable, so `--json` is intentionally a no-op there.

The examples below are real binary output, captured from the golden fixtures
in `tests/golden/` and verified against the binary on every test run
(`tests/json_contracts.rs`); they cannot rot. They were produced in a sandbox
repo containing the example workspace from `examples/.agent-profile/`.
Regenerate after an intentional change with:

```bash
UPDATE_GOLDENS=1 cargo test --test json_contracts
```

Conventions that hold across all envelopes:

- stdout carries exactly one JSON document; human-readable hints (like the
  one-time gitignore reminder) go to stderr, so stdout is always parseable.
- Paths are relative to the repo root, except home-scoped paths, which are
  `~`-abbreviated (e.g. `~/.codex/config.toml`).
- Exit codes carry the semantics (0/1/2/3/4/5 per design §2); the envelope
  carries the detail. Orchestrators should gate on the exit code and parse
  the envelope for specifics.
- Errors: when a command fails with a library error, the envelope is
  `{ "schemaVersion": 1, "error": { "exitCode": <int>, "message": <string> } }`
  and the same message is printed to stderr.
- In `current`, `appliedAt` is `YYYY-MM-DDTHH:MM:SSZ` (UTC). The examples pin
  it to a fixed value; the wire value is the real apply time.

## `list --json`

Profiles and capability blocks discovered in the workspace. Invalid files
appear with an `error` field instead of their metadata rather than failing
the whole listing.

```json
{
  "schemaVersion": 1,
  "profiles": [
    {
      "name": "implementer",
      "role": "implementer",
      "description": "Implementation worker with write access and TDD context.",
      "targets": [
        "claude-teammate",
        "codex-agent"
      ],
      "include": [
        "models-high-reasoning"
      ]
    },
    {
      "name": "qa",
      "role": "qa",
      "description": "Verification worker — runs tests and browser checks, no source edits.",
      "targets": [
        "claude-subagent",
        "claude-teammate"
      ]
    },
    {
      "name": "reviewer",
      "role": "reviewer",
      "description": "Read-only code reviewer for implement-review loops.",
      "targets": [
        "claude-subagent",
        "claude-teammate",
        "codex-agent"
      ],
      "include": [
        "mcp-github-readonly",
        "perms-readonly"
      ]
    }
  ],
  "capabilities": [
    {
      "name": "mcp-github-readonly"
    },
    {
      "name": "models-high-reasoning"
    },
    {
      "name": "perms-readonly"
    }
  ]
}
```

## `show --role <r> --resolved --json`

The fully merged profile (includes applied, profile wins) plus per-key
provenance — which file supplied each resolved key. Without `--resolved`, the
envelope is `{ "schemaVersion": 1, "role": <r>, "profile": <parsed profile> }`.

```json
{
  "schemaVersion": 1,
  "role": "reviewer",
  "resolved": {
    "apiVersion": "agent-profile/v1",
    "name": "reviewer",
    "description": "Read-only code reviewer for implement-review loops.",
    "role": "reviewer",
    "targets": [
      "claude-subagent",
      "claude-teammate",
      "codex-agent"
    ],
    "include": [
      "mcp-github-readonly",
      "perms-readonly"
    ],
    "model": {
      "claude": "claude-fable-5",
      "codex": "gpt-5.3-codex",
      "effort": "high"
    },
    "permissionMode": "readonly",
    "tools": {
      "allow": [
        "Read",
        "Grep",
        "Bash(git diff:*)",
        "Bash(git log:*)"
      ],
      "deny": [
        "Write",
        "Edit"
      ]
    },
    "mcpServers": {
      "docs-search": {
        "type": "http",
        "url": "https://mcp.example.com/docs"
      },
      "github-readonly": {
        "type": "stdio",
        "command": "github-mcp",
        "args": [
          "--readonly"
        ],
        "env": {
          "GITHUB_TOKEN": "${env:GITHUB_PAT_RO}"
        }
      }
    },
    "skills": [
      "code-review",
      "verification-before-completion"
    ],
    "context": [
      "fragments/reviewer-instructions.md"
    ],
    "metadata": {
      "owner": "xiaolei",
      "tags": [
        "review",
        "readonly"
      ]
    }
  },
  "provenance": {
    "context": "profiles/reviewer.yaml",
    "mcpServers.docs-search": "profiles/reviewer.yaml",
    "mcpServers.github-readonly": "profiles/reviewer.yaml",
    "model.claude": "profiles/reviewer.yaml",
    "model.codex": "profiles/reviewer.yaml",
    "model.effort": "profiles/reviewer.yaml",
    "permissionMode": "profiles/reviewer.yaml",
    "skills": "profiles/reviewer.yaml",
    "tools.allow": "profiles/reviewer.yaml",
    "tools.deny": "profiles/reviewer.yaml"
  }
}
```

## `validate [--role <r>] --json`

One result per validated unit. `ok` is the aggregate; exit 0 when true,
exit 2 when false. Each `findings` entry is a structured object —
`{ "code", "message", "file"?, "key"? }` — where `code` is the stable `Vxxx`
code, `message` is the human-readable explanation, and `file`/`key` (present
when known) name the workspace-relative file and the dotted key path. For
example, a profile that embeds a literal token yields:

```json
{
  "code": "V012",
  "message": "env key `GITHUB_TOKEN` looks secret-bearing…",
  "file": "profiles/leaky.yaml",
  "key": "mcpServers.gh.env.GITHUB_TOKEN"
}
```

The golden example below shows the all-clean case (every `findings` array
empty).

```json
{
  "schemaVersion": 1,
  "ok": true,
  "results": [
    {
      "name": "implementer",
      "kind": "profile",
      "findings": [],
      "blocksResolved": 1
    },
    {
      "name": "qa",
      "kind": "profile",
      "findings": [],
      "blocksResolved": 0
    },
    {
      "name": "reviewer",
      "kind": "profile",
      "findings": [],
      "blocksResolved": 2
    },
    {
      "name": "mcp-github-readonly",
      "kind": "capability",
      "findings": [],
      "blocksResolved": 0
    },
    {
      "name": "models-high-reasoning",
      "kind": "capability",
      "findings": [],
      "blocksResolved": 0
    },
    {
      "name": "perms-readonly",
      "kind": "capability",
      "findings": [],
      "blocksResolved": 0
    }
  ]
}
```

## `render --role <r> --target <t> --json`

The dry-run plan: `actions` is the ordered list of file operations apply
would perform; nothing has been written. `op` is one of
`create | merge-keys | symlink | append-block`. `skipped` lists profile
fields the target cannot express (never silently dropped), each with the
nearest workaround in `hint`. `notes` (optional) carries honest-limitation
notes. With `--out <dir>`, an `outDir` field is added.

`symlink` actions carry `linkTarget`: the location the referenced skill
actually resolved to, following the design §1.2 order — usually
`~/.agents/skills/<name>` (cross-tool store), or a repo-relative path like
`.agent-profile/skills/<name>` when the skill resolves only via the repo
fallback.

```json
{
  "schemaVersion": 1,
  "role": "implementer",
  "target": "codex-agent",
  "actions": [
    {
      "op": "create",
      "path": ".codex/agents/implementer.toml",
      "content": "# generated by agent-profile v0.1.0 — role: implementer — do not hand-edit\nname = \"implementer\"\ndescription = \"Implementation worker with write access and TDD context.\"\nmodel = \"gpt-5.5\"\nmodel_reasoning_effort = \"medium\"\napproval_policy = \"on-failure\"\nsandbox_mode = \"workspace-write\"\n\n[permissions]\nallow = [\n  'prefix_rule(pattern=[\"cargo\",\"test\"], decision=\"allow\")',\n  'prefix_rule(pattern=[\"cargo\",\"build\"], decision=\"allow\")',\n  'prefix_rule(pattern=[\"git\"], decision=\"allow\")',\n]\ndeny = [\n  'prefix_rule(pattern=[\"git\",\"push\"], decision=\"deny\")',\n]\n"
    },
    {
      "op": "merge-keys",
      "path": "~/.codex/config.toml",
      "keys": [
        "mcp_servers.github"
      ],
      "content": "[mcp_servers.github]\ncommand = \"github-mcp\"\n\n[mcp_servers.github.env]\nGITHUB_TOKEN = \"${env:GITHUB_PAT_RW}\"\n"
    },
    {
      "op": "append-block",
      "path": "AGENTS.md",
      "marker": "role=implementer",
      "content": "<!-- agent-profile:begin role=implementer -->\n@.agent-profile/fragments/implementer-instructions.md\n<!-- agent-profile:end role=implementer -->\n"
    }
  ],
  "skipped": [
    {
      "field": "tools.allow",
      "reason": "`Read` — Codex permissions express only shell prefix rules; bare tool `Read` has no prefix_rule equivalent",
      "hint": "only Bash prefix rules translate to Codex prefix_rule(...)"
    },
    {
      "field": "tools.allow",
      "reason": "`Grep` — Codex permissions express only shell prefix rules; bare tool `Grep` has no prefix_rule equivalent",
      "hint": "only Bash prefix rules translate to Codex prefix_rule(...)"
    },
    {
      "field": "tools.allow",
      "reason": "`Glob` — Codex permissions express only shell prefix rules; bare tool `Glob` has no prefix_rule equivalent",
      "hint": "only Bash prefix rules translate to Codex prefix_rule(...)"
    },
    {
      "field": "tools.allow",
      "reason": "`Write` — Codex permissions express only shell prefix rules; bare tool `Write` has no prefix_rule equivalent",
      "hint": "only Bash prefix rules translate to Codex prefix_rule(...)"
    },
    {
      "field": "tools.allow",
      "reason": "`Edit` — Codex permissions express only shell prefix rules; bare tool `Edit` has no prefix_rule equivalent",
      "hint": "only Bash prefix rules translate to Codex prefix_rule(...)"
    },
    {
      "field": "skills",
      "reason": "codex-agent cannot express per-agent skill references (test-driven-development)",
      "hint": "Codex discovers ~/.codex/skills/ itself; no per-agent wiring exists"
    }
  ]
}
```

## `diff --role <r> --target <t> --json`

Unified diff per path between the rendered plan and what is on disk right
now. Empty `diffs` ⇒ exit 0; any difference ⇒ exit 3 (script-gate semantics,
like `git diff --exit-code`). Symlink actions diff on their link target.

```json
{
  "schemaVersion": 1,
  "role": "implementer",
  "target": "codex-agent",
  "diffs": [
    {
      "path": ".codex/agents/implementer.toml",
      "diff": "--- .codex/agents/implementer.toml\n+++ rendered\n@@ -0,0 +1,17 @@\n+# generated by agent-profile v0.1.0 — role: implementer — do not hand-edit\n+name = \"implementer\"\n+description = \"Implementation worker with write access and TDD context.\"\n+model = \"gpt-5.5\"\n+model_reasoning_effort = \"medium\"\n+approval_policy = \"on-failure\"\n+sandbox_mode = \"workspace-write\"\n+\n+[permissions]\n+allow = [\n+  'prefix_rule(pattern=[\"cargo\",\"test\"], decision=\"allow\")',\n+  'prefix_rule(pattern=[\"cargo\",\"build\"], decision=\"allow\")',\n+  'prefix_rule(pattern=[\"git\"], decision=\"allow\")',\n+]\n+deny = [\n+  'prefix_rule(pattern=[\"git\",\"push\"], decision=\"deny\")',\n+]\n"
    },
    {
      "path": "~/.codex/config.toml",
      "diff": "--- ~/.codex/config.toml\n+++ rendered\n@@ -0,0 +1,5 @@\n+[mcp_servers.github]\n+command = \"github-mcp\"\n+\n+[mcp_servers.github.env]\n+GITHUB_TOKEN = \"${env:GITHUB_PAT_RW}\"\n"
    },
    {
      "path": "AGENTS.md",
      "diff": "--- AGENTS.md\n+++ rendered\n@@ -0,0 +1,3 @@\n+<!-- agent-profile:begin role=implementer -->\n+@.agent-profile/fragments/implementer-instructions.md\n+<!-- agent-profile:end role=implementer -->\n"
    }
  ]
}
```

## `doctor --role <r> --target <t> --json`

Preflight findings against the installed CLI (`cliVersion` is probed, or
taken from `--assume-version`; `null` when undeterminable). `level` is
`error | warn | info`; finding `code`s are stable and never reused (table in
the README). Errors ⇒ exit 4; warnings/info alone ⇒ exit 0.

```json
{
  "schemaVersion": 1,
  "role": "reviewer",
  "target": "claude-teammate",
  "cliVersion": "2.0.34",
  "findings": [
    {
      "level": "error",
      "code": "F012",
      "message": "field `model.effort` not honored by claude 2.0.34 teammate spawn — no per-teammate effort control",
      "field": "model.effort"
    },
    {
      "level": "error",
      "code": "F012",
      "message": "frontmatter field `permissionMode` not honored by claude 2.0.34 teammate spawn — rendered via settings.local.json defaultMode instead (info)",
      "field": "permissionMode"
    },
    {
      "level": "warn",
      "code": "F031",
      "message": "mcp server `github-readonly`: command `github-mcp` not found on PATH",
      "field": "mcpServers.github-readonly.command"
    },
    {
      "level": "warn",
      "code": "F044",
      "message": "env reference ${env:GITHUB_PAT_RO} is unset in current environment",
      "field": "mcpServers.github-readonly.env.GITHUB_TOKEN"
    }
  ]
}
```

## `apply --role <r> --target <t> --session-id <id> --json`

The render envelope plus `sessionId`, `backupDir`, and a per-action `status`
(`applied` | `unchanged`). On pre-apply drift the envelope is instead
`{ "schemaVersion": 1, "sessionId": <id>, "status": "drift", "drift": [<string>…] }`
with exit 3.

```json
{
  "schemaVersion": 1,
  "sessionId": "run7-reviewer",
  "role": "reviewer",
  "target": "claude-teammate",
  "status": "applied",
  "backupDir": ".agent-profile/backups/run7-reviewer",
  "actions": [
    {
      "op": "create",
      "path": ".claude/agents/reviewer.md",
      "content": "---\nname: reviewer\ndescription: Read-only code reviewer for implement-review loops.\nmodel: claude-fable-5\ntools: Read, Grep, Bash(git diff:*), Bash(git log:*)\n---\n<!-- generated by agent-profile v0.1.0 — role: reviewer — do not hand-edit -->\n",
      "status": "applied"
    },
    {
      "op": "merge-keys",
      "path": ".mcp.json",
      "keys": [
        "mcpServers.docs-search",
        "mcpServers.github-readonly"
      ],
      "content": "{\n  \"mcpServers\": {\n    \"docs-search\": {\n      \"type\": \"http\",\n      \"url\": \"https://mcp.example.com/docs\"\n    },\n    \"github-readonly\": {\n      \"type\": \"stdio\",\n      \"command\": \"github-mcp\",\n      \"args\": [\n        \"--readonly\"\n      ],\n      \"env\": {\n        \"GITHUB_TOKEN\": \"${env:GITHUB_PAT_RO}\"\n      }\n    }\n  }\n}\n",
      "status": "applied"
    },
    {
      "op": "merge-keys",
      "path": ".claude/settings.local.json",
      "keys": [
        "permissions.allow[+4]",
        "permissions.deny[+2]",
        "permissions.defaultMode"
      ],
      "content": "{\n  \"permissions\": {\n    \"allow\": [\n      \"Read\",\n      \"Grep\",\n      \"Bash(git diff:*)\",\n      \"Bash(git log:*)\"\n    ],\n    \"deny\": [\n      \"Write\",\n      \"Edit\"\n    ],\n    \"defaultMode\": \"plan\"\n  }\n}\n",
      "status": "applied"
    },
    {
      "op": "symlink",
      "path": ".claude/skills/code-review",
      "linkTarget": "~/.agents/skills/code-review",
      "status": "applied"
    },
    {
      "op": "symlink",
      "path": ".claude/skills/verification-before-completion",
      "linkTarget": "~/.agents/skills/verification-before-completion",
      "status": "applied"
    },
    {
      "op": "append-block",
      "path": "CLAUDE.md",
      "marker": "session=run7-reviewer",
      "content": "<!-- agent-profile:begin session=run7-reviewer role=reviewer -->\n@.agent-profile/fragments/reviewer-instructions.md\n<!-- agent-profile:end session=run7-reviewer -->\n",
      "status": "applied"
    }
  ],
  "skipped": [
    {
      "field": "model.effort",
      "reason": "claude-teammate has no per-teammate effort control",
      "hint": "set effortLevel in your own settings if needed"
    }
  ],
  "notes": [
    "settings.local.json permissions are project-wide, not per-teammate; the session scope means \"while this worker session is active in this repo/worktree\" — give each worker its own worktree for true isolation"
  ]
}
```

## `current --json`

Active sessions from `state.json`. `actions` is the action count.

```json
{
  "schemaVersion": 1,
  "sessions": [
    {
      "sessionId": "run7-reviewer",
      "role": "reviewer",
      "target": "claude-teammate",
      "scope": "repo",
      "appliedAt": "2026-06-10T09:14:03Z",
      "actions": 6
    }
  ]
}
```

## `teardown (--session-id <id> | --all) --json`

One entry per torn-down session with the reversal steps performed (`op` is
`delete | restore | unlink | remove`). On post-apply drift, sessions that
were refused appear in a top-level `drift` array
(`[{ "sessionId": <id>, "paths": [<string>…] }]`) and the exit code is 3;
already-clean sessions in the same `--all` run still appear under
`sessions`.

```json
{
  "schemaVersion": 1,
  "sessions": [
    {
      "sessionId": "run7-reviewer",
      "status": "torn-down",
      "steps": [
        {
          "op": "remove",
          "path": "CLAUDE.md",
          "detail": "block session=run7-reviewer"
        },
        {
          "op": "unlink",
          "path": ".claude/skills/verification-before-completion"
        },
        {
          "op": "unlink",
          "path": ".claude/skills/code-review"
        },
        {
          "op": "restore",
          "path": ".claude/settings.local.json",
          "detail": "removed 4 allow rules, 2 deny rules, defaultMode"
        },
        {
          "op": "restore",
          "path": ".mcp.json",
          "detail": "removed keys mcpServers.docs-search, mcpServers.github-readonly"
        },
        {
          "op": "delete",
          "path": ".claude/agents/reviewer.md"
        }
      ]
    }
  ]
}
```

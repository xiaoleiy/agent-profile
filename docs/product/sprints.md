# agent-profile — MVP Sprint Plan

Status: 2026-06-10. Execution plan for the build contract in
`docs/product/design.md` (the design doc wins on any conflict). Six sprints,
each a tracer-bullet vertical slice: at the end of every sprint the binary
builds, does something real end-to-end, and is covered by tests.

Sizing: **S** ≈ half a day, **M** ≈ 1–2 days, **L** ≈ 3+ days (single
engineer + agent loop). Dependencies reference task ids; tasks without a
listed dependency only require the prior sprint's DoD.

Standing definition of done (applies to every sprint, in addition to the
per-sprint DoD):

- `cargo test` green, `cargo clippy -- -D warnings` clean, `cargo fmt --check` clean.
- New/changed behavior covered by tests (unit for schema/adapters, integration
  under `tests/` using `tempfile` sandboxes — never the developer's real
  `~/.claude` or `~/.codex`).
- README "Status" section updated to reflect what actually works.
- No scope creep into the non-goals (`design.md` §6): no `use`, no registry,
  no policy, no Cursor, no network calls.

---

## Sprint 1 — Scaffold, schema, validate / list / show

Slice: a user can author `.agent-profile/profiles/*.yaml` +
`capabilities/*.yaml` and get them parsed, merged, validated, and printed.
No adapter code yet.

| Id | Task | Acceptance criteria | Size | Deps |
|---|---|---|---|---|
| S1-T1 | Crate scaffold: single crate, public `lib.rs` API + thin `main.rs` clap shell (derive), module skeleton per design §7 (`cli/`, `schema/`, `adapters/`, `state/`, `doctor/`), exit-code enum (0/1/2/3/4/5) mapped centrally in `main.rs`. Global flags `--json`, `--dir`, `-q`, `-v` parsed (may be no-ops where unused). | `cargo run -- --help` shows all subcommands from design §2 (stubs return "not implemented", exit 1). Exit-code mapping unit-tested. All business logic callable from `lib.rs` without the CLI (fold-into-agent-loop requirement). | M | — |
| S1-T2 | Schema structs (serde): `Profile`, `CapabilityBlock`, `ModelSpec`, `ToolsSpec`, `McpServer` (stdio/http), enums for `permissionMode` and targets. Strict parsing: unknown top-level keys are errors; `apiVersion` must be `agent-profile/v1`; capability `kind: capability` enforced; capability blocks restricted to `model\|permissionMode\|tools\|mcpServers\|skills\|context`. | Round-trip tests for all three example profiles from design §1.2/§1.4. Unknown key, bad apiVersion, and extra capability key each produce a distinct error message naming the file and key. | M | S1-T1 |
| S1-T3 | Discovery + include resolution + merge: walk up from cwd to find `.agent-profile/` (`--dir` overrides); resolve `include:` from `capabilities/`; ONE include level (include inside a block = validation error); merge order blocks-in-order then profile wins; maps deep-merge one level; lists replace, never concat. Track per-key provenance (which file supplied the final value). | Table-driven merge tests covering: scalar override, map deep-merge one level, list replacement, block-vs-block order, nested-include rejection, missing block error. Provenance recorded for every resolved key. | M | S1-T2 |
| S1-T4 | Secret-literal scanner (design §5.4): token prefixes (`ghp_`, `github_pat_`, `sk-`, `xox`, `AKIA`, `glpat-`, `Bearer `), high-entropy ≥20-char strings, secret-named keys not using `${env:…}`; `# agent-profile: allow-literal` same-line escape hatch. Runs over all `env` maps and `mcpServers` strings. | Fixture suite: each heuristic has a triggering and a non-triggering case; `allow-literal` silences exactly the annotated value; `${env:VAR}` never flags. Findings carry file/key location. | M | S1-T2 |
| S1-T5 | `validate [--role <r>]`: schema + name-equals-filename-stem + targets-list sanity + include resolution + secret scan; all profiles when `--role` omitted. Exit 2 on any failure, 0 on clean. `--json` output with findings list. | Running against the three example profiles exits 0; seeded-broken fixtures (bad stem, missing block, nested include, secret literal) each exit 2 with the right finding code. | S | S1-T3, S1-T4 |
| S1-T6 | `list` and `show --role <r> [--resolved]`: list prints profiles + capability blocks; `show --resolved` prints the fully merged profile with `# from: <file>` provenance comment per key (design §1.3). Both honor `--json`. | `show --resolved` for `reviewer` shows keys sourced from both included blocks and the profile, each with correct provenance. `list --json` is valid JSON with `schemaVersion: 1`. | S | S1-T3 |
| S1-T7 | Repo example fixtures: check in `.agent-profile/` examples (reviewer/implementer/qa profiles verbatim from design §1, the three capability blocks, fragment files) under `examples/` and wire them into integration tests as fixtures. | `agent-profile validate --dir examples/.agent-profile` exits 0 in CI. | S | S1-T5 |

Sprint DoD: standing DoD + `validate`/`list`/`show` work end-to-end on the
examples; `--json` envelope (`schemaVersion: 1`) established and tested once,
reused by all later commands.

---

## Sprint 2 — `codex-agent` render + diff (simplest target first)

Slice: dry-run rendering and diffing for the Codex target — per the roadmap
recommendation, Codex first because the surface is compact. Establishes the
`Action` model every later sprint builds on. Nothing writes to provider files
yet (`--out` sandbox only).

| Id | Task | Acceptance criteria | Size | Deps |
|---|---|---|---|---|
| S2-T1 | Adapter contract: `trait RenderTarget { fn plan(&Profile, scope) -> Vec<Action> }` with `Action` ops `create \| merge-keys \| symlink \| append-block` (exact shapes from design §3.4 state records, minus hashes), plus a `Skipped` finding type for inexpressible fields (never silently dropped). | Trait + Action serialize to the `render --json` contract in design §2. Unit test: a synthetic adapter's plan round-trips through JSON. | M | — |
| S2-T2 | Canonical tool-grammar parser + Codex translation: parse Claude rule grammar (`Tool`, `Tool(spec)`, `Bash(cmd:*)`, `mcp__server__tool`); translate `Bash(...)` rules to `prefix_rule(pattern=[…], decision=…)`; classify untranslatable rules (returned as findings, consumed by doctor in S3). | Table tests: every grammar form parses; `Bash(git diff:*)` → the exact prefix_rule string in design §3.3; `mcp__x__y` and bare-tool rules on Codex are classified untranslatable with a reason. | M | — |
| S2-T3 | `codex-agent` adapter: own `.codex/agents/<role>.toml` (provenance header comment, `name`/`description`/`model`/`model_reasoning_effort`, `[permissions]` from translated rules, flat keys — **never** `[profiles.*]`); plan `merge-keys` for `[mcp_servers.<name>]` into the scoped `config.toml` via `toml_edit` (key-level only); plan `append-block` for the `AGENTS.md` marked `@include` stub; `permissionMode` → approval-key mapping per design §3.3.4. `--scope repo\|user` selects paths. | Golden-file test: `implementer` plan matches design §4.6 (file list, ops, key names). `toml_edit` merge test: a `config.toml` fixture with comments, `[marketplaces.*]` and `[projects.*]` tables comes out byte-identical except the added `mcp_servers` keys. Plan never includes `auth.json`/cache paths (denylist asserted). | L | S2-T1, S2-T2 |
| S2-T4 | `render --role <r> --target <t> [--out <dir>] [--scope …]`: dry-run by default (prints the plan: paths, ops, line counts, SKIPPED lines with workaround hints); there is **no** `--dry-run` flag; `--out` materializes rendered artifacts into a sandbox directory only; rejects targets not in the profile's `targets:` list (exit 2). | Human output matches the §4.6 walkthrough shape; `--out` writes the agent TOML + a rendered config.toml fragment into the directory and touches nothing else (asserted with a before/after dir snapshot); `--json` matches the §2 contract. | M | S2-T3 |
| S2-T5 | `diff --role <r> --target <t>`: rendered plan vs current on-disk state using `similar`; unified diff per path; exit 3 when differences exist, 0 when none (script-gate semantics); `--json` = diffs per path. | Against an empty repo fixture: exit 3 with creates shown. After manually placing identical rendered output: exit 0. Merge-key diff shows only the added TOML keys (matches §4.6 output). | M | S2-T4 |

Sprint DoD: standing DoD + `render`/`diff` fully working for `codex-agent` on
the example profiles; the Action model and ownership denylist are in place as
shared infrastructure; README gains a "try it" snippet using `--out`.

---

## Sprint 3 — Claude targets render + doctor

Slice: both Claude render targets plan correctly, and `doctor` — the
differentiated DX feature — validates profiles against the *installed* CLIs.

| Id | Task | Acceptance criteria | Size | Deps |
|---|---|---|---|---|
| S3-T1 | `claude-subagent` adapter: single owned `.claude/agents/<role>.md` (frontmatter `name`/`description`/`model`/`tools` line, provenance comment, fragment contents + per-skill "Use the <name> skill" pointer lines in the body per design §3.1); `mcpServers`/`permissionMode` and other inexpressible fields emitted as SKIPPED findings with the named workaround (e.g. "use claude-teammate"). `--scope user` targets `~/.claude/agents/`. | Golden test reproduces design §3.1 output for `reviewer`; §4.1 walkthrough output (including the skipped-mcpServers hint) reproduced. | M | S2-T4 |
| S3-T2 | `claude-teammate` adapter (the headline target, design §3.2): agent .md (own) + `.mcp.json` merge-keys (added `mcpServers.<key>` only; collision with a key we didn't create = drift finding) + `.claude/settings.local.json` merge-keys (`permissions.allow`/`deny` entries tracked individually, `permissions.defaultMode` from `permissionMode`) + per-skill symlink plans into `~/.agents/skills/` + `CLAUDE.md` marked-block `@include` stub with `session=` marker. Render output includes the honest project-wide-settings limitation note verbatim policy from §3.2.3. | Plan for `reviewer` contains exactly the 5 action kinds in the §3.4 state example. JSON merge tests: existing `.mcp.json`/`settings.local.json` keys we don't own are preserved key-for-key; collision produces the drift finding. Never-touch denylist (`~/.claude.json` wholesale, `~/.claude/settings.json`, plugin registries) asserted untouchable with no flag bypass. | L | S3-T1 |
| S3-T3 | Doctor core + field-support matrix: probe `claude --version` / `codex --version` (via `which` + exec); versioned in-crate table of which frontmatter/TOML/settings fields each CLI version honors, with a documented update procedure in a `doctor/README` comment block — this is the adapter-churn early-warning system. Unknown-to-target fields → error finding naming the nearest workaround. | Matrix lookup unit-tested across at least two synthetic CLI versions with differing field support; missing CLI binary → clear error finding, not a panic. Version parse handles real `claude`/`codex` output formats. | L | S3-T2 |
| S3-T4 | Doctor checks battery (design §5.3): MCP stdio `command` on PATH / http `url` well-formed; `${env:VAR}` resolvable (warn only); skills resolve in `~/.agents/skills/<name>/SKILL.md` then repo fallback; Codex grammar translatability (errors list each untranslatable rule, from S2-T2); stale state.json sessions >24h (info) — stub until S4 lands state, then activate. | Each check has pass + fail fixtures. Severity mapping enforced: errors → exit 4, warnings alone → exit 0. Output reproduces the §4.3 walkthrough shape. | M | S3-T3 |
| S3-T5 | `doctor --role <r> --target <t> --json`: `{ target, cliVersion, findings: [{level, code, message, field?}] }` with stable finding codes (F-prefixed, registered in one enum so codes never collide or get reused). | JSON schema asserted in tests; finding codes documented in a table in the README; exit-code behavior covered. | S | S3-T4 |

Sprint DoD: standing DoD + all three targets render and diff on the example
profiles; `doctor` runs against the real installed CLIs on the dev machine and
produces honest findings; finding-code table published in README.

---

## Sprint 4 — Spawn-time apply / teardown, state, backups

Slice: the actual product moment — reversible spawn-scoped apply. This sprint
turns plans into mutations, safely.

| Id | Task | Acceptance criteria | Size | Deps |
|---|---|---|---|---|
| S4-T1 | State ledger + hashing: `state.json` (`schemaVersion: 1`, sessions map, action records with `hashBefore`/`hashAfter`/`backup` per design §3.4); sha256 hashing; backups under `.agent-profile/backups/<session-id>/`; atomic write-then-rename (`tempfile`) for state.json itself and **every** file mutation. | State round-trips through serde matching the §3.4 example verbatim. Corrupt state.json → exit 5 with recovery hint, never a panic. Kill-during-write test (write to temp, rename) leaves no torn files. | M | — |
| S4-T2 | Action executor: interpret `create`/`merge-keys`/`symlink`/`append-block` against the filesystem with per-action backup + hash recording; JSON merges key-level on `serde_json::Value`; TOML merges via `toml_edit`; hardcoded never-touch denylist enforced at executor level (no flag bypasses — defense in depth below the adapters). | Each op has an integration test in a tempdir sandbox; executor refuses any path on the denylist even if an adapter (or test) plans it; partial-failure mid-apply rolls back already-executed actions of that session. | L | S4-T1 |
| S4-T3 | `apply --role <r> --target <t> --session-id <id> [--scope] [--force]`: `--session-id` mandatory (clap-enforced); pre-apply drift check (owned path exists without our provenance header → exit 3; merge-key collision → exit 3); `--force` overrides, still backs up, records the override in state; duplicate active session for same role+target → exit 5; resolved `${env:VAR}` secrets may only be written to gitignored files (`git check-ignore`, design §5.5) else refuse; one-time gitignore hint for `state.json`/`backups/`. | §4.5 drift-refusal walkthrough reproduced exactly (message, exit 3, suggested commands). §4.2 multi-role apply works in a sandbox repo: 3 sessions, correct action counts, `apply --json` matches the §2 contract incl. `sessionId`/`backupDir`/per-action status. Secret-to-tracked-file refusal tested both ways. | L | S4-T2 |
| S4-T4 | `teardown (--session-id <id> \| --all) [--force]`: replay actions in reverse per design §3.4 — `create` deleted only if hash still equals `hashAfter`; `merge-keys` removes exactly the recorded keys preserving everyone else's later changes; `symlink` removed only if it still points at `linkTarget`; `append-block` deleted by marker wherever it moved; post-apply drift → refuse with drifted-path list, exit 3; `--force` falls back to backup restore (owned) / best-effort key-level (merges); backups deleted on clean teardown; session removed from state. | Property-style test: for each target, apply→teardown on a fixture repo restores a byte-identical tree (modulo state/backups). Interleaved-edit test: a foreign key added to `.mcp.json` between apply and teardown survives teardown. §4.4 walkthrough reproduced. Unknown session id → exit 5. | L | S4-T3 |
| S4-T5 | `current`: list active sessions from state (role, target, applied-at, action count), human + `--json`; activate the doctor stale-session check from S3-T4. | §4.2 `current` output shape reproduced; empty state prints a friendly empty line, exit 0. | S | S4-T3 |
| S4-T6 | Full-cycle integration suite: scripted sandbox runs of validate→doctor→render→diff→apply→current→teardown for all three targets, asserting exit codes at every step; this becomes the CI smoke test. | Suite passes in CI without any real `~/.claude`/`~/.codex` (HOME overridden into the sandbox); runtime < 60s. | M | S4-T4, S4-T5 |

Sprint DoD: standing DoD + the §3.5 two-shell-call contract is real: an
orchestrator script can apply, run, and tear down in a trap, and the repo is
clean afterwards. This is the internal dogfood milestone — start using it from
agent-loop locally.

---

## Sprint 5 — Orchestrator interface, docs, examples

Slice: everything an external Looping Engineering practitioner needs to adopt
the tool without talking to us. The probe's success metrics depend on this
sprint more than on any code sprint.

| Id | Task | Acceptance criteria | Size | Deps |
|---|---|---|---|---|
| S5-T1 | Freeze and golden-test the `--json` contracts (render/apply/doctor/diff/list/show/current, `schemaVersion: 1`); document each contract with an example in `docs/json-api.md`; any breaking change henceforth bumps schemaVersion. | Golden JSON fixtures in tests; doc examples generated from the real binary output (checked by a test, so they can't rot). | M | — |
| S5-T2 | `completions <shell>` (clap_complete: bash/zsh/fish) + `--version` build metadata (version + git sha). | Generated zsh completions load cleanly in CI; `--version` output stable format. | S | — |
| S5-T3 | Hook recipes doc (`docs/integrations.md`): the §3.5 apply/trap/teardown pattern written concretely for CAO, crew-code, and agent-loop, including the per-worker-worktree pattern that collapses the project-scope settings caveat, exit-code gating (`\|\| exit $?`), and `--json` parsing examples (jq). | Each recipe copy-pastes into a working script against the sandbox fixture repo (verified manually once, recorded in the doc). Honest-limitation note from §3.2.3 carried verbatim. | M | S5-T1 |
| S5-T4 | README rewrite: positioning paragraph (spawn-time role-profile resolver, explicitly NOT a control plane/registry/business), the Agent Teams gap (#23669/#24505) and Codex story, 5-minute quickstart (validate→render→apply→teardown), doctor pitch, non-goals section lifted from design §6, the three 90-day falsifiable probe metrics stated openly, link tree to docs/. Tone: engineer-to-engineer, zero hype. | A reader who has never seen the repo can complete the quickstart in a fresh repo in <5 min (test on a clean machine/worktree). Non-goals and metrics present verbatim in spirit. | M | S5-T3 |
| S5-T5 | Polished `examples/`: the three roles + capabilities + fragments with a walkthrough README per design §4 flows; GitHub issue templates that ask reporters which orchestrator and CLI versions they use (feeds metric 1 and 3 measurement). | `validate`/`render`/`apply`/`teardown` all succeed against `examples/` in CI; issue templates render on GitHub. | S | S5-T4 |
| S5-T6 | Real dogfood integration: wire agent-profile into one actual agent-loop (or CAO) run spawning ≥2 differently-profiled workers; capture rough edges as issues; fix the small ones in-sprint. | One recorded successful end-to-end loop run using apply/teardown around real worker spawns; remaining rough edges filed as issues, not silently known. | M | S5-T3 |

Sprint DoD: standing DoD + a stranger can adopt the tool from README alone;
JSON contracts frozen; one real orchestrator integration exists (this is
already metric-1 evidence if it's an external orchestrator — document it).

---

## Sprint 6 — CI/CD, release, distribution

Slice: v0.1.0 shipped through channels a Rust-CLI audience expects. Start the
90-day probe clock at the release announcement, not before.

| Id | Task | Acceptance criteria | Size | Deps |
|---|---|---|---|---|
| S6-T1 | CI workflow (GitHub Actions): test + clippy `-D warnings` + fmt + the S4-T6 smoke suite, on macOS and Linux; PR-blocking. | Green on main; a seeded clippy warning fails CI. | S | — |
| S6-T2 | Release automation with `cargo-dist` (or equivalent): tag-triggered builds for macOS arm64/x86_64 and Linux x86_64/arm64 (musl), checksummed archives, generated install script, GitHub Release with changelog. | `git tag v0.1.0-rc1` produces installable artifacts for all four targets; binary runs the smoke quickstart on a clean macOS and Linux box. | M | S6-T1 |
| S6-T3 | Homebrew tap (`xiaoleiy/homebrew-tap`) with formula auto-bumped by the release workflow; `cargo install agent-profile` path via crates.io publish (verify name availability early — if taken, decide the fallback name **before** tagging). | `brew install xiaoleiy/tap/agent-profile` and `cargo install agent-profile` both yield a working binary; release workflow bumps the formula without manual edits. | M | S6-T2 |
| S6-T4 | Probe instrumentation (process, not telemetry — the binary makes zero network calls): a `docs/probe.md` scorecard with the three 90-day metrics, how each is measured (stars, non-author issues/PRs tagged by use case, Codex-target requests, vendor issue watch on #23669/#24505 with the ~7-week close-cadence note), and the pre-committed failure action (fold into agent-loop as internal module). Date-stamped at release. | Scorecard exists with empty measurement table + review dates (T+30/T+60/T+90); linked from README. | S | — |
| S6-T5 | v0.1.0 release + announce: tag, release notes (what it does, what it deliberately doesn't, the metrics), short announce posts where Looping Engineering practitioners are (the orchestrator repos' discussions, relevant threads on the Agent Teams gap issues — as a "this exists" comment, not spam). | v0.1.0 live on GitHub Releases, brew, crates.io; announce posted; probe clock start date recorded in `docs/probe.md`. | S | S6-T2, S6-T3, S6-T4 |

Sprint DoD: standing DoD + release is reproducible from a tag with no manual
steps beyond the tag push; probe scorecard live and dated.

---

## Sequencing notes

- S2 before S3 deliberately (roadmap recommendation): Codex's compact TOML
  surface debugs the Action model cheaply before the five-surface
  claude-teammate adapter stresses it.
- Doctor (S3) lands before apply (S4) so that by the time the tool can mutate
  files, it can also tell you why a mutation won't behave — the preflight
  story is part of the safety story.
- The field-support matrix (S3-T3) is the highest-uncertainty task in the
  plan; if CLI version probing proves flaky, degrade gracefully to
  "matrix keyed on documented versions + a `--assume-version` override"
  rather than blocking the sprint.
- If the schedule compresses, cut from the end of S5 (S5-T6 can slip into S6)
  — never from S4's drift/teardown tests; reversibility is the trust feature.

---

## Deviations

- **S1-T6 acceptance ("show --resolved for reviewer shows keys sourced from
  both included blocks and the profile")**: the design §1.2 `reviewer.yaml`
  example — reproduced verbatim in `examples/` per S1-T7 — explicitly sets
  every mergeable key itself, so under the normative merge rule ("profile
  wins over all blocks") every resolved key of `reviewer` is correctly
  attributed to `profiles/reviewer.yaml`; no block-sourced value can survive.
  The criterion is covered instead by a dedicated integration fixture
  (`show_resolved_provenance_covers_blocks_and_profile` in `tests/cli.rs`)
  where included blocks supply surviving keys and the resolved view
  attributes keys to two blocks and the profile. `reviewer` is still
  asserted to render provenance comments (`# from: profiles/reviewer.yaml`).

- **S2-T3 wording "into the scoped config.toml"**: the design doc wins on
  conflict, and design §4.6 shows a *default (repo) scope* render merging
  `[mcp_servers.*]` into `~/.codex/config.toml`; the recon doc (§2) likewise
  only observed MCP servers in the global config. The codex-agent adapter
  therefore always targets `~/.codex/config.toml` for the MCP merge
  (key-level, toml_edit). `--scope user` still relocates the *owned* files
  (`~/.codex/agents/<role>.toml`, `~/.codex/AGENTS.md` for the marked block).

- **S2-T3 prefix_rule string representation**: `toml_edit` serializes string
  values containing double quotes as TOML *literal strings*
  (`'prefix_rule(pattern=["git","diff"], decision="allow")'`) rather than the
  escaped basic strings shown in design §3.3.1. The parsed value is
  character-identical to the design's; tests assert the exact rule text. The
  golden test for §4.6 asserts file list, ops, and key names as specified
  (line counts naturally differ from the illustrative `(19 lines)`).

- **S3-T3 "probe against the INSTALLED CLI"**: offline CLIs expose no
  capability introspection, so probing is limited to `which` + `<cli>
  --version` (parses the real `2.0.34 (Claude Code)` / `codex-cli 0.21.0`
  output shapes). Field support itself comes from the in-crate
  documented-version matrix (`src/doctor/matrix.rs`, update procedure in its
  module comment), with the sprint-plan-sanctioned `--assume-version`
  override when probing is unavailable. Missing binary → `F001` error
  finding; unparseable version → `F002` warn and version-gated checks are
  skipped rather than guessed.

- **S3-T2 `permissions.defaultMode` values**: the design does not pin the
  `permissionMode` → Claude `defaultMode` mapping. Chosen (documented in
  `claude::default_mode`): `readonly`/`plan` → `"plan"` (Claude has no
  readonly mode; the merged deny rules carry the read-only posture),
  `acceptEdits` → `"acceptEdits"`, `auto` → `"auto"` (observed in the wild,
  recon §1.2), `unrestricted` → `"bypassPermissions"`.

- **S3-T2 teammate agent .md body**: per the §3.4 state example, context
  fragments route exclusively to the CLAUDE.md marked block and skills to
  symlinks, so the teammate-owned agent file body carries only the
  provenance header (frontmatter as §3.1). The subagent target embeds
  fragment contents + skill pointer lines per §3.1.

- **S3-T2 "only added keys" in JSON diffs**: appending an entry to an
  existing JSON array necessarily rewrites the previous last element's
  trailing comma, so the unified diff shows that one line as `-`/`+` with
  identical content. The merge itself is key-level and content-preserving
  (`serde_json` with `preserve_order`); the test asserts no *content* is
  removed.

- **S3-T4 §4.3 reproduction**: the walkthrough's `F012`/`F031`/`F044`/`ok
  skills: 2/2` lines are reproduced byte-for-byte in shape; doctor on the
  examples `reviewer` additionally reports `model.effort` as not honored by
  Claude targets (an honest finding the illustrative walkthrough omits) and
  `F061` untranslatable-rule errors when the target is codex-agent.

- **S4-T3 `apply --json` actions shape**: design §4.2 illustrates
  `"actions":5` (a count) while §2 specifies "same [as render] plus …
  per-action status". §2 wins: `apply --json` emits the render-shaped
  `actions` array with a per-action `"status"` (`applied`/`unchanged`); the
  count is the array length.

- **S4-T3 §4.2 three-teammate-sessions in one repo**: roles whose
  `permissionMode` maps to *different* `permissions.defaultMode` values
  collide on that key under the normative §5.1 merge rule when applied to
  `claude-teammate` in the same repo (the second apply is an exit-3 drift
  refusal — correctly). The integration fixture therefore applies
  reviewer+qa (both map to `"plan"`) to claude-teammate and implementer to
  codex-agent. §4.2's own parenthetical (per-worker worktrees) dissolves the
  collision in real orchestrator use.

- **S4-T4 hash verification vs foreign-edit preservation**: §5.2 says "each
  action's hashAfter re-verified; mismatch → refuse", but §3.4 requires
  merge-keys teardown to *preserve* foreign changes and the S4-T4 acceptance
  requires a foreign `.mcp.json` key added after apply to survive teardown.
  Reconciliation implemented: `hashAfter` equality is the clean fast path;
  on mismatch, `merge-keys` falls back to verifying that the exact keys we
  wrote are still intact (changed → drift, removed-by-user → no-op, foreign
  additions → fine); `create`/`symlink` stay strictly verified
  (hash / link target); `append-block` is removed by marker "wherever it now
  sits" (tolerant by §3.4's own wording).

- **S4-T3 secret materialization scope**: per §5.5 only Claude `.mcp.json`
  env maps materialize `${env:VAR}`; the codex `config.toml` merge writes the
  reference verbatim (matches the §4.6 diff). The resolved fragment is also
  recorded in the state.json action record (needed for key-level reversal and
  drift verification) — state.json is required-gitignored alongside
  `backups/` via the one-time hint, and the record is deleted on teardown,
  so the hygiene story of §5.5 is preserved.

- **S4-T2 pre-existing correct symlink**: a `.claude/skills/<s>` symlink that
  already points at our target is treated as foreign (status `unchanged`,
  not recorded in state, left in place at teardown) rather than drift —
  removing a link we did not create would violate "restore the exact
  pre-apply state".

- **S4-T3 one-time gitignore hint**: printed to stderr (keeps `--json`
  stdout parseable) and tracked via a `gitignoreHintShown` extension flag in
  state.json (absent from the §3.4 example; skipped while false, so the
  example round-trips verbatim).

- **S5-T6 "wire into one actual agent-loop (or CAO) run"**: no real
  orchestrator is installed in the build environment, so the closest
  faithful behavior was implemented and recorded instead: (a) a real,
  captured render→apply→teardown cycle against a sandbox repo
  (`docs/dogfood.md`, output verbatim, including a foreign edit surviving
  teardown); (b) the trap-based worker wrapper from `docs/integrations.md`
  executed end-to-end in the sandbox on both the worker-success and
  worker-failure paths, plus the duplicate-session exit-5 collision; (c) the
  per-worker worktree pattern verified with reviewer+qa applied to
  `claude-teammate` simultaneously in two worktrees of one repo, both torn
  down byte-clean. Rough edges found while dogfooding are recorded in
  `docs/dogfood.md`; the two small ones (JSON merges mislabeled "via
  toml_edit" in render human output; `append-block` column misalignment in
  apply human output) were fixed in-sprint. A run against a real external
  orchestrator remains open for S6/the probe period.

- **S5-T2 "completions load cleanly in CI"**: CI does not exist until
  S6-T1. Verified locally instead: `zsh -n` syntax check + `compinit` load
  of the generated `_agent-profile`, plus structural assertions
  (`#compdef`, function name, subcommands present) in `tests/cli.rs`.
  Wiring the same check into CI belongs to S6-T1.

- **S5-T1 golden normalization**: `appliedAt` (the only volatile field in
  any v1 envelope) is pinned to a fixed timestamp before golden comparison;
  everything else is compared byte-for-byte against real binary output.

- **Regression round 1 — `completions --json` exception**: design §2 and
  `docs/json-api.md` state every command supports `--json` and prints exactly
  one JSON envelope. `completions <shell>` is the one exception: it emits the
  raw shell completion script (the bytes meant to be sourced) and ignores
  `--json`, because wrapping a completion script in a JSON envelope would make
  it unusable. The safer, more useful behavior is to keep emitting the raw
  script; `docs/json-api.md` now documents the exception explicitly.

- **Regression round 1 — path-traversal hardening**: `--role`, `include`
  names, and `--session-id` are now validated as single safe path components
  (no `/`, `\`, or `..`); `skills:` entries must be bare names and `context:`
  fragments must stay inside the `.agent-profile/` tree (no `..`/absolute).
  These names are used to build filesystem paths (capability/profile files,
  backup dirs, skill symlinks), so an unvalidated `../` could read or write
  outside the workspace/repo. The identity check (`name:` must equal the
  filename stem, V003) is now enforced in `merge::resolve` as well as
  `validate`, so `render`/`apply` — which build provider file paths from
  `name` — can never act on an unvalidated `name`. `.yml` files are no longer
  discovered by `list` (resolution is `.yaml`-only per §1.1), so discovery and
  show/validate agree.

- **Regression round 1 — resolved-secret hygiene in state.json**: the apply
  ledger now records the *unresolved* merge fragment (with `${env:VAR}`
  intact) in each merge-keys action's `content`, never the materialized
  secret. The resolved value lives only in the on-disk target file (which
  apply already gitignore-enforces, §5.5) and is no longer copyable out of
  `state.json`. Post-apply drift detection still relies on `hashAfter` first;
  the unresolved `content` is the fallback key-level check.

- **Regression round 2 — render/diff/apply run the full validate gate
  (AREA2-03, A3-R2-2)**: design §5.4 places the secret-literal scan under
  `validate`, and §2's exit table reserves 2 for "validation failure". The
  enforcement was inconsistent: render/apply rejected V001/V003 but skipped
  the secret scan, so a profile validate refuses (inline `ghp_` token) was
  written verbatim to `--out` sandboxes and `~/.codex/config.toml`.
  `build_plan` (the shared front half of render/diff/apply) now runs the
  full `validate` unit report and refuses with exit 2 on any finding — a
  profile that fails `validate` is never materialized anywhere. New code
  **V013** (`context:` fragment file does not exist) extends "include
  resolution" to fragments: previously validate passed a profile whose
  fragment the claude-subagent renderer could not read, surfacing as a raw
  I/O error (exit 1) instead of a validation failure (exit 2).

- **Regression round 2 — diff/teardown compare materialized `.mcp.json`
  content (A3-R2-1)**: §5.5 has apply resolve `${env:VAR}` into a gitignored
  `.mcp.json` while state.json keeps the reference. diff and the pre-teardown
  intact-check compared the on-disk *resolved* value against the *unresolved*
  fragment and misclassified the tool's own key as a foreign collision
  (exit 3 after a pristine apply). Both now resolve set env references the
  same way apply does before comparing, so `diff` is the §2 post-apply
  clean-state check (exit 0) and clean teardowns no longer need `--force`.

- **Regression round 2 — pre-existing identical keys are not adopted
  (A3-R2-3)**: §3.2.2 says "existing keys we did not create are never
  modified — a name collision is a drift error". When the existing value
  *equals* the profile's (user already had `defaultMode: plan`), apply is not
  modifying anything, so instead of refusing (exit 3) we chose the safer
  reading: the key is recorded as pre-existing (not ours), the merge is a
  no-op for it, and teardown leaves it untouched. The user's key survives a
  full apply/teardown round-trip byte-for-byte; list entries already behaved
  this way via the `appended` tracking.

- **Regression round 2 — cross-session refcounting at teardown
  (AP4-R2-01)**: §3.4 "remove exactly the recorded keys" silently weakened a
  second still-active session sharing the same repo (teardown of s1 stripped
  the `deny` rules and `defaultMode` s2 still required). Apply now records a
  *shared claim* when a merged key/entry already exists and another active
  session recorded it (a pre-existing value no session recorded stays
  user-owned and is never recorded); teardown skips keys/entries any other
  active session still claims — the last session out removes them. Worktree
  isolation remains the recommended setup (§3.2.3); this makes the shared-repo
  fallback safe instead of silently lossy.

- **Regression round 3 — diff on a session-owned `${env:VAR}` mcp key
  (A3-R3-1)**: §2 frames diff as "rendered output vs what is on disk right
  now", but rendering `.mcp.json` content faithfully requires the apply-time
  value of every `${env:VAR}` reference, which state.json deliberately never
  stores (§5.5 — no materialized secrets in the ledger). With the variable
  unset or rotated in the diffing shell, a literal re-render misattributed
  the tool's own key as a foreign collision (§3.2.2 reserves collisions for
  "existing keys we did not create"). Chosen behavior: a merge key recorded
  by an active session whose *unresolved* fragment still equals the plan's,
  and whose on-disk value matches the recorded fragment modulo `${env:VAR}`
  materialization, is owned-and-clean — diff adopts the on-disk value (exit
  0, empty `diffs` in `--json`). Consequence, stated honestly: once the
  variable is unset/rotated, an in-place edit of only the materialized
  secret value is indistinguishable from legitimate materialization and is
  reported clean (whole-file edits still surface as ordinary unified diffs;
  teardown's `hashAfter` check is unaffected). A genuinely foreign key —
  one no active session recorded — still collides (exit 3).

- **Regression round 3 — teardown of a forced merge restores the backed-up
  key (AREA2-R3-01)**: §3.4 "merge-keys → remove exactly the recorded keys"
  is destructive when the recorded key was a *foreign* value that `apply
  --force` overwrote (§5.1 backs it up precisely so it can come back).
  Teardown now restores a recorded key from the session backup whenever the
  backup holds a different value than the one we wrote, and removes it
  otherwise (creations, and shared claims where the last session out
  removes) — making the merge-keys `--force` path consistent with the
  owned-file path and with §3.2 "teardown restores the exact pre-apply
  state".

- **Regression round 3 — append-block residue (A3-R3-3)**: teardown removed
  an emptied CLAUDE.md/AGENTS.md only when the tearing session itself
  created the file. When the file pre-existed solely as *another session's
  marked blocks* (multi-role apply in one repo), the last teardown left a
  0-byte file. Teardown now also removes the emptied file when its pre-apply
  backup consists entirely of agent-profile marked blocks — the file owes
  its existence to the tool. A user-owned file (any foreign content,
  including an intentionally empty pre-existing file) is preserved.

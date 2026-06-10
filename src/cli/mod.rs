//! Command definitions and output formatting (human + `--json`). The thin
//! `main.rs` shell parses args and calls [`run`]; all logic underneath is
//! plain library code.

pub mod resolved;

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand};
use serde::Serialize;

use crate::JSON_SCHEMA_VERSION;
use crate::adapters::{self, Action, Plan, PlanContext, Skipped};
use crate::doctor;
use crate::error::{Error, ExitCode};
use crate::schema::types::Target;
use crate::schema::validate::{self, UnitReport};
use crate::schema::{ResolvedProfile, Workspace, merge};
use crate::state::{self, Session};

pub use crate::adapters::Scope;

/// Build metadata shown by `--version` (S5-T2): crate version + git sha.
/// Format is stable: `X.Y.Z (<sha>)`, with `unknown` outside a git build.
pub const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("AGENT_PROFILE_GIT_SHA"),
    ")"
);

#[derive(Debug, Parser)]
#[command(
    name = "agent-profile",
    version = VERSION,
    about = "Spawn-time role-profile resolver for coding-agent runtimes"
)]
pub struct Cli {
    /// Machine-readable JSON output.
    #[arg(long, global = true)]
    pub json: bool,
    /// Override `.agent-profile/` discovery (default: walk up from cwd).
    #[arg(long, global = true, value_name = "PATH")]
    pub dir: Option<PathBuf>,
    /// Suppress non-essential output.
    #[arg(short, long, global = true)]
    pub quiet: bool,
    /// Increase verbosity.
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub verbose: u8,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Args)]
pub struct RoleTargetArgs {
    /// Role profile name.
    #[arg(long)]
    pub role: String,
    /// Runtime target (claude-subagent | claude-teammate | codex-agent).
    #[arg(long)]
    pub target: String,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List role profiles and capability blocks.
    List,
    /// Print a profile; --resolved = after includes, with provenance.
    Show {
        /// Role profile name.
        #[arg(long)]
        role: String,
        /// Print the fully merged profile with per-key provenance.
        #[arg(long)]
        resolved: bool,
    },
    /// Schema + include resolution + secret-literal scan.
    Validate {
        /// Validate a single role (all profiles when omitted).
        #[arg(long)]
        role: Option<String>,
    },
    /// Preflight a profile against the installed CLI.
    Doctor {
        #[command(flatten)]
        role_target: RoleTargetArgs,
        /// Assume this CLI version instead of probing `<cli> --version`
        /// (checks run against the documented-version matrix).
        #[arg(long = "assume-version", value_name = "X.Y[.Z]")]
        assume_version: Option<doctor::Version>,
    },
    /// Dry-run by default: print planned files/merges.
    Render {
        #[command(flatten)]
        role_target: RoleTargetArgs,
        /// Write rendered artifacts to a directory instead.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
        /// Where target files would live.
        #[arg(long, value_enum, default_value = "repo")]
        scope: Scope,
    },
    /// Rendered output vs what is on disk right now.
    Diff {
        #[command(flatten)]
        role_target: RoleTargetArgs,
        #[arg(long, value_enum, default_value = "repo")]
        scope: Scope,
    },
    /// Spawn-time apply; requires --session-id.
    Apply {
        #[command(flatten)]
        role_target: RoleTargetArgs,
        /// Orchestrator-supplied worker/session identifier.
        #[arg(long = "session-id", value_name = "ID")]
        session_id: String,
        #[arg(long, value_enum, default_value = "repo")]
        scope: Scope,
        /// Override drift refusal (records the override).
        #[arg(long)]
        force: bool,
    },
    /// Reverse a session's actions from state.json.
    #[command(group = clap::ArgGroup::new("which").required(true).args(["session_id", "all"]))]
    Teardown {
        #[arg(long = "session-id", value_name = "ID")]
        session_id: Option<String>,
        /// Tear down every active session.
        #[arg(long)]
        all: bool,
        /// Tear down even if post-apply drift detected.
        #[arg(long)]
        force: bool,
    },
    /// List active sessions from state.json.
    Current,
    /// Generate shell completions.
    Completions {
        /// Shell to generate completions for.
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

/// Run a parsed command; prints output and returns the process exit code.
/// This is the only place exit codes are decided (besides clap usage errors).
pub fn run(cli: Cli) -> i32 {
    match dispatch(&cli) {
        Ok(code) => code.code(),
        Err(err) => {
            if cli.json {
                let envelope = serde_json::json!({
                    "schemaVersion": JSON_SCHEMA_VERSION,
                    "error": { "exitCode": err.exit_code().code(), "message": err.to_string() },
                });
                println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
            }
            eprintln!("error: {err}");
            err.exit_code().code()
        }
    }
}

fn dispatch(cli: &Cli) -> Result<ExitCode, Error> {
    match &cli.command {
        Command::List => cmd_list(cli),
        Command::Show { role, resolved } => cmd_show(cli, role, *resolved),
        Command::Validate { role } => cmd_validate(cli, role.as_deref()),
        Command::Render {
            role_target,
            out,
            scope,
        } => cmd_render(cli, role_target, out.as_deref(), *scope),
        Command::Diff { role_target, scope } => cmd_diff(cli, role_target, *scope),
        Command::Doctor {
            role_target,
            assume_version,
        } => cmd_doctor(cli, role_target, *assume_version),
        Command::Apply {
            role_target,
            session_id,
            scope,
            force,
        } => cmd_apply(cli, role_target, session_id, *scope, *force),
        Command::Teardown {
            session_id,
            all,
            force,
        } => cmd_teardown(cli, session_id.as_deref(), *all, *force),
        Command::Current => cmd_current(cli),
        Command::Completions { shell } => cmd_completions(*shell),
    }
}

fn cmd_completions(shell: clap_complete::Shell) -> Result<ExitCode, Error> {
    use clap::CommandFactory;
    let mut command = Cli::command();
    clap_complete::generate(shell, &mut command, "agent-profile", &mut std::io::stdout());
    Ok(ExitCode::Success)
}

fn workspace(cli: &Cli) -> Result<Workspace, Error> {
    let cwd = std::env::current_dir().map_err(|source| Error::Io {
        path: PathBuf::from("."),
        source,
    })?;
    Workspace::locate(cli.dir.as_deref(), &cwd)
}

// ---------------------------------------------------------------- list

#[derive(Debug, Serialize)]
struct ProfileSummary {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    targets: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    include: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct CapabilitySummary {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn cmd_list(cli: &Cli) -> Result<ExitCode, Error> {
    let ws = workspace(cli)?;

    let mut profiles = Vec::new();
    for name in ws.profile_names()? {
        match ws.load_profile(&name) {
            Ok(src) => profiles.push(ProfileSummary {
                name,
                role: Some(src.value.role.clone()),
                description: Some(src.value.description.clone()),
                targets: src
                    .value
                    .targets
                    .iter()
                    .map(|t| t.as_str().into())
                    .collect(),
                include: src.value.include.clone(),
                error: None,
            }),
            Err(err) => profiles.push(ProfileSummary {
                name,
                role: None,
                description: None,
                targets: Vec::new(),
                include: Vec::new(),
                error: Some(err.to_string()),
            }),
        }
    }

    let mut capabilities = Vec::new();
    for name in ws.capability_names()? {
        let error = match ws.load_capability(&name) {
            Ok(_) => None,
            Err(err) => Some(err.to_string()),
        };
        capabilities.push(CapabilitySummary { name, error });
    }

    if cli.json {
        let envelope = serde_json::json!({
            "schemaVersion": JSON_SCHEMA_VERSION,
            "profiles": profiles,
            "capabilities": capabilities,
        });
        println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
        return Ok(ExitCode::Success);
    }

    let name_width = profiles.iter().map(|p| p.name.len()).max().unwrap_or(0);
    println!("PROFILES");
    if profiles.is_empty() {
        println!("  (none)");
    }
    for p in &profiles {
        match &p.error {
            None => println!(
                "  {:name_width$}  {}",
                p.name,
                p.description.as_deref().unwrap_or("")
            ),
            Some(e) => println!("  {:name_width$}  (invalid: {e})", p.name),
        }
    }
    println!("CAPABILITIES");
    if capabilities.is_empty() {
        println!("  (none)");
    }
    for c in &capabilities {
        match &c.error {
            None => println!("  {}", c.name),
            Some(e) => println!("  {}  (invalid: {e})", c.name),
        }
    }
    Ok(ExitCode::Success)
}

// ---------------------------------------------------------------- show

fn cmd_show(cli: &Cli, role: &str, show_resolved: bool) -> Result<ExitCode, Error> {
    let ws = workspace(cli)?;

    if !show_resolved {
        let src = ws.load_profile(role)?;
        if cli.json {
            let envelope = serde_json::json!({
                "schemaVersion": JSON_SCHEMA_VERSION,
                "role": role,
                "profile": src.value,
            });
            println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
        } else {
            print!("{}", src.raw);
            if !src.raw.ends_with('\n') {
                println!();
            }
        }
        return Ok(ExitCode::Success);
    }

    let resolved_profile = merge::resolve(&ws, role)?;
    if cli.json {
        let envelope = serde_json::json!({
            "schemaVersion": JSON_SCHEMA_VERSION,
            "role": role,
            "resolved": resolved::resolved_json(&resolved_profile),
            "provenance": resolved_profile.provenance,
        });
        println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
    } else {
        print!("{}", resolved::resolved_yaml(&resolved_profile));
    }
    Ok(ExitCode::Success)
}

// ---------------------------------------------------------------- validate

fn cmd_validate(cli: &Cli, role: Option<&str>) -> Result<ExitCode, Error> {
    let ws = workspace(cli)?;
    let reports = match role {
        Some(role) => vec![validate::validate_role(&ws, role)?],
        None => validate::validate_all(&ws)?,
    };
    let ok = reports.iter().all(UnitReport::ok);

    if cli.json {
        let envelope = serde_json::json!({
            "schemaVersion": JSON_SCHEMA_VERSION,
            "ok": ok,
            "results": reports,
        });
        println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
    } else {
        for report in &reports {
            print_unit_report(report, cli.quiet);
        }
    }
    Ok(if ok {
        ExitCode::Success
    } else {
        ExitCode::Validation
    })
}

fn print_unit_report(report: &UnitReport, quiet: bool) {
    let label = match report.kind {
        validate::UnitKind::Profile => report.name.clone(),
        validate::UnitKind::Capability => format!("{} (capability)", report.name),
    };
    if report.ok() {
        if !quiet {
            let blocks = match (report.kind, report.blocks_resolved) {
                (validate::UnitKind::Profile, n) => {
                    format!(", {n} capability block{} resolved", plural(n))
                }
                (validate::UnitKind::Capability, _) => String::new(),
            };
            println!("✓ {label}: schema OK{blocks}, no secret literals");
        }
    } else {
        println!("✗ {label}:");
        for finding in &report.findings {
            println!("    {finding}");
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

// ---------------------------------------------------------------- render / diff

/// Shared front half of render/diff/apply: resolve, check target, plan,
/// denylist.
fn build_plan(
    cli: &Cli,
    rt: &RoleTargetArgs,
    scope: Scope,
    session_id: Option<&str>,
) -> Result<(Workspace, ResolvedProfile, Target, PlanContext, Plan), Error> {
    let ws = workspace(cli)?;
    let target = Target::parse(&rt.target).ok_or_else(|| Error::UnknownTarget {
        target: rt.target.clone(),
    })?;
    let resolved = merge::resolve(&ws, &rt.role)?;
    if !resolved.profile.targets.contains(&target) {
        return Err(Error::TargetNotAllowed {
            role: rt.role.clone(),
            target: rt.target.clone(),
        });
    }
    let repo_root = ws.root().parent().unwrap_or(ws.root()).to_path_buf();
    let home = std::env::home_dir().ok_or_else(|| Error::Io {
        path: PathBuf::from("~"),
        source: std::io::Error::other("cannot determine home directory"),
    })?;
    let ctx = PlanContext {
        scope,
        repo_root,
        workspace_root: ws.root().to_path_buf(),
        home,
        session_id: session_id.map(String::from),
    };
    let adapter = adapters::adapter_for(target).ok_or(Error::NotImplemented)?;
    let plan = adapter.plan(&resolved, &ctx)?;
    // Hardcoded never-touch denylist — enforced regardless of flags.
    adapters::denylist::assert_plan_allowed(&plan, &ctx)?;
    Ok((ws, resolved, target, ctx, plan))
}

fn cmd_render(
    cli: &Cli,
    rt: &RoleTargetArgs,
    out: Option<&std::path::Path>,
    scope: Scope,
) -> Result<ExitCode, Error> {
    let (_ws, _resolved, target, _ctx, plan) = build_plan(cli, rt, scope, None)?;

    let written = match out {
        Some(dir) => Some((dir, adapters::materialize(&plan, dir)?.len())),
        None => None,
    };

    if cli.json {
        let mut envelope = serde_json::json!({
            "schemaVersion": JSON_SCHEMA_VERSION,
            "role": rt.role,
            "target": target.as_str(),
            "actions": plan.actions,
            "skipped": plan.skipped,
        });
        if !plan.notes.is_empty() {
            envelope["notes"] = serde_json::json!(plan.notes);
        }
        if let Some(dir) = out {
            envelope["outDir"] = serde_json::json!(dir.display().to_string());
        }
        println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
    } else {
        print_plan_human(&plan, written);
    }
    Ok(ExitCode::Success)
}

fn print_plan_human(plan: &Plan, written: Option<(&std::path::Path, usize)>) {
    match written {
        None => println!("PLAN (dry-run — nothing written):"),
        Some((dir, _)) => println!(
            "PLAN (rendered to {} — provider files untouched):",
            dir.display()
        ),
    }
    let width = plan
        .actions
        .iter()
        .map(|a| a.path().len())
        .max()
        .unwrap_or(0)
        .max(20);
    for action in &plan.actions {
        match action {
            Action::Create { path, content } => println!(
                "  {:<10}  {path:<width$}  ({} lines)",
                "create",
                content.lines().count()
            ),
            Action::MergeKeys { path, keys, .. } => {
                let added = keys
                    .iter()
                    .map(|k| format!("+ [{k}]"))
                    .collect::<Vec<_>>()
                    .join("  ");
                println!("  {:<10}  {path:<width$}  {added}", "merge-keys");
                if path.ends_with(".toml") {
                    println!(
                        "{:14}(key-level merge via toml_edit; comments/format preserved;",
                        ""
                    );
                    println!("{:14} existing tables untouched)", "");
                } else {
                    println!(
                        "{:14}(key-level JSON merge; keys we don't own preserved key-for-key)",
                        ""
                    );
                }
            }
            Action::AppendBlock { path, content, .. } => println!(
                "  {:<10}  {path:<width$}  marked block ({} lines, @include stub)",
                "append",
                content.lines().count()
            ),
            Action::Symlink { path, link_target } => {
                println!("  {:<10}  {path:<width$}  -> {link_target}", "symlink")
            }
        }
    }
    for s in &plan.skipped {
        print_skipped(s);
    }
    for note in &plan.notes {
        println!("  note: {note}");
    }
    match written {
        Some((_, n)) => println!("Wrote {n} rendered file{} (sandbox only).", plural(n)),
        None => println!(
            "Run `agent-profile render … --out <dir>` to inspect files, or `apply` to write."
        ),
    }
}

fn print_skipped(s: &Skipped) {
    println!("  skipped {}: {}", s.field, s.reason);
    if let Some(hint) = &s.hint {
        println!("          → {hint}");
    }
}

fn cmd_diff(cli: &Cli, rt: &RoleTargetArgs, scope: Scope) -> Result<ExitCode, Error> {
    let (_ws, _resolved, target, ctx, plan) = build_plan(cli, rt, scope, None)?;

    let mut diffs: Vec<(String, String)> = Vec::new();
    for action in &plan.actions {
        let abs = ctx.resolve(action.path());
        let (cur, desired) = match action {
            // Symlinks diff on their target, not file contents.
            Action::Symlink { link_target, .. } => {
                let cur = match std::fs::read_link(&abs) {
                    Ok(t) => symlink_repr(&abbreviate_home(&t, &ctx.home)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                    // Exists but is not a symlink — show it as drift.
                    Err(_) => "(existing path is not a symlink)\n".to_string(),
                };
                (cur, symlink_repr(link_target))
            }
            _ => {
                let current = match std::fs::read_to_string(&abs) {
                    Ok(s) => Some(s),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(source) => return Err(Error::Io { path: abs, source }),
                };
                let desired = adapters::desired_file_state(action, current.as_deref())?;
                (current.unwrap_or_default(), desired)
            }
        };
        if cur != desired {
            let text_diff = similar::TextDiff::from_lines(cur.as_str(), desired.as_str());
            let mut unified = text_diff.unified_diff();
            unified.context_radius(3).header(action.path(), "rendered");
            diffs.push((action.path().to_string(), unified.to_string()));
        }
    }

    if cli.json {
        let envelope = serde_json::json!({
            "schemaVersion": JSON_SCHEMA_VERSION,
            "role": rt.role,
            "target": target.as_str(),
            "diffs": diffs
                .iter()
                .map(|(path, diff)| serde_json::json!({ "path": path, "diff": diff }))
                .collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
    } else {
        for (_, diff) in &diffs {
            print!("{diff}");
        }
    }
    // `git diff --exit-code` semantics, shifted to our code space (design §2).
    Ok(if diffs.is_empty() {
        ExitCode::Success
    } else {
        ExitCode::Drift
    })
}

fn symlink_repr(target: &str) -> String {
    format!("symlink -> {target}\n")
}

fn abbreviate_home(path: &std::path::Path, home: &std::path::Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

// ---------------------------------------------------------------- doctor

fn cmd_doctor(
    cli: &Cli,
    rt: &RoleTargetArgs,
    assume_version: Option<doctor::Version>,
) -> Result<ExitCode, Error> {
    let ws = workspace(cli)?;
    let target = Target::parse(&rt.target).ok_or_else(|| Error::UnknownTarget {
        target: rt.target.clone(),
    })?;
    let resolved = merge::resolve(&ws, &rt.role)?;
    if !resolved.profile.targets.contains(&target) {
        return Err(Error::TargetNotAllowed {
            role: rt.role.clone(),
            target: rt.target.clone(),
        });
    }
    let home = std::env::home_dir().ok_or_else(|| Error::Io {
        path: PathBuf::from("~"),
        source: std::io::Error::other("cannot determine home directory"),
    })?;
    let env = doctor::Env {
        home,
        workspace_root: ws.root().to_path_buf(),
        assume_version,
    };
    let report = doctor::run(&resolved, target, &env);

    if cli.json {
        let envelope = serde_json::json!({
            "schemaVersion": JSON_SCHEMA_VERSION,
            "role": rt.role,
            "target": target.as_str(),
            "cliVersion": report.cli_version.map(|v| v.to_string()),
            "findings": report.findings,
        });
        println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
    } else {
        let shown = match (&report.cli_version, report.assumed) {
            (Some(v), true) => format!("{v} (assumed)"),
            (Some(v), false) => v.to_string(),
            (None, _) => "(unknown)".to_string(),
        };
        println!("{} --version → {shown}", report.cli);
        println!("FINDINGS");
        if report.findings.is_empty() && report.ok.is_empty() {
            println!("  (none)");
        }
        for f in &report.findings {
            println!(
                "  {:<5}  {}  {}",
                f.level.as_str(),
                f.code.as_str(),
                f.message
            );
        }
        for line in &report.ok {
            println!("  {:<5}  {line}", "ok");
        }
    }
    // Errors → exit 4; warnings/info alone → exit 0 (design §5.3).
    Ok(if report.has_errors() {
        ExitCode::Doctor
    } else {
        ExitCode::Success
    })
}

// ---------------------------------------------------------------- apply

fn cmd_apply(
    cli: &Cli,
    rt: &RoleTargetArgs,
    session_id: &str,
    scope: Scope,
    force: bool,
) -> Result<ExitCode, Error> {
    // A session id becomes a directory name under .agent-profile/backups/
    // (design §3.1/§3.4); reject any id that could escape it (e.g. `../../`).
    if !crate::schema::workspace::is_safe_component(session_id) {
        return Err(Error::InvalidSessionId {
            id: session_id.to_string(),
        });
    }
    let (ws, _resolved, target, ctx, plan) = build_plan(cli, rt, scope, Some(session_id))?;
    let mut ledger = state::load(ws.root())?;

    // Session/state gates (exit 5, design §2).
    if ledger.sessions.contains_key(session_id) {
        return Err(Error::Session(format!(
            "session `{session_id}` is already active — tear it down first \
             (`agent-profile teardown --session-id {session_id}`) or pick a new id"
        )));
    }
    if let Some((id, _)) = ledger
        .sessions
        .iter()
        .find(|(_, s)| s.role == rt.role && s.target == target.as_str())
    {
        return Err(Error::Session(format!(
            "active session `{id}` already applies role `{}` to target `{}` — tear it down first",
            rt.role,
            target.as_str()
        )));
    }

    let outcome = match state::execute_apply(&plan, &ctx, session_id, force) {
        Ok(outcome) => outcome,
        // §4.5 drift refusal: message + suggested commands, exit 3.
        Err(Error::ApplyDrift(msgs)) => {
            if cli.json {
                let envelope = serde_json::json!({
                    "schemaVersion": JSON_SCHEMA_VERSION,
                    "sessionId": session_id,
                    "status": "drift",
                    "drift": msgs,
                });
                println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
            } else {
                for msg in &msgs {
                    println!("✗ drift: {msg}");
                }
                println!(
                    "  → inspect: agent-profile diff --role {} --target {}",
                    rt.role, rt.target
                );
                println!("  → override (backs up the file first): apply --force");
            }
            return Ok(ExitCode::Drift);
        }
        Err(e) => return Err(e),
    };

    let scope_str = match scope {
        Scope::Repo => "repo",
        Scope::User => "user",
    };
    let session = Session {
        role: rt.role.clone(),
        target: target.as_str().to_string(),
        scope: scope_str.to_string(),
        applied_at: state::now_rfc3339(),
        agent_profile_version: env!("CARGO_PKG_VERSION").to_string(),
        actions: outcome.records.clone(),
    };
    ledger.sessions.insert(session_id.to_string(), session);

    // One-time hint: state.json/backups belong in .gitignore (design §3.4).
    let show_hint = !ledger.gitignore_hint_shown
        && !state::is_git_ignored(&ctx.repo_root, &state::state_path(ws.root()));
    if show_hint {
        ledger.gitignore_hint_shown = true;
    }
    state::save(ws.root(), &ledger)?;
    if show_hint && !cli.quiet {
        eprintln!(
            "hint: add `.agent-profile/state.json` and `.agent-profile/backups/` to your \
             .gitignore — they are local session state"
        );
    }

    let backup_dir = state::backup_dir(ws.root(), session_id);
    let backup_display = backup_dir
        .strip_prefix(&ctx.repo_root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| backup_dir.display().to_string());

    if cli.json {
        // §2 contract: render envelope plus sessionId/backupDir/per-action status.
        let actions: Vec<serde_json::Value> = plan
            .actions
            .iter()
            .map(|a| {
                let mut v = serde_json::to_value(a).unwrap();
                let status = if outcome.unchanged_paths.contains(&a.path().to_string()) {
                    "unchanged"
                } else {
                    "applied"
                };
                v["status"] = serde_json::json!(status);
                v
            })
            .collect();
        let mut envelope = serde_json::json!({
            "schemaVersion": JSON_SCHEMA_VERSION,
            "sessionId": session_id,
            "role": rt.role,
            "target": target.as_str(),
            "status": "applied",
            "backupDir": backup_display,
            "actions": actions,
            "skipped": plan.skipped,
        });
        if !plan.notes.is_empty() {
            envelope["notes"] = serde_json::json!(plan.notes);
        }
        println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
    } else {
        println!(
            "APPLY session {session_id} ({} → {})",
            rt.role,
            target.as_str()
        );
        let width = plan
            .actions
            .iter()
            .map(|a| a.path().len())
            .max()
            .unwrap_or(0)
            .max(20);
        for action in &plan.actions {
            let extra = match action {
                Action::MergeKeys { keys, .. } => format!(
                    "  + {}",
                    keys.iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Action::Symlink { link_target, .. } => format!("  -> {link_target}"),
                Action::AppendBlock { marker, .. } => format!("  marked block {marker}"),
                Action::Create { .. } => String::new(),
            };
            let status = if outcome.unchanged_paths.contains(&action.path().to_string()) {
                " (unchanged)"
            } else {
                ""
            };
            // Width 12 fits the longest op name, `append-block`.
            println!(
                "  {:<12} {:<width$}{extra}{status}",
                action.op(),
                action.path()
            );
        }
        for s in &plan.skipped {
            print_skipped(s);
        }
        for note in &plan.notes {
            println!("  note: {note}");
        }
        println!(
            "✓ session {session_id} applied ({} action{}; backups in {backup_display})",
            outcome.records.len(),
            plural(outcome.records.len()),
        );
    }
    Ok(ExitCode::Success)
}

// ---------------------------------------------------------------- teardown

fn cmd_teardown(
    cli: &Cli,
    session_id: Option<&str>,
    all: bool,
    force: bool,
) -> Result<ExitCode, Error> {
    let ws = workspace(cli)?;
    let mut ledger = state::load(ws.root())?;

    let ids: Vec<String> = if all {
        ledger.sessions.keys().cloned().collect()
    } else {
        let id = session_id.expect("clap enforces session-id xor all");
        if !ledger.sessions.contains_key(id) {
            return Err(Error::Session(format!(
                "unknown session `{id}` — see `agent-profile current`"
            )));
        }
        vec![id.to_string()]
    };

    if ids.is_empty() {
        if cli.json {
            let envelope = serde_json::json!({
                "schemaVersion": JSON_SCHEMA_VERSION,
                "sessions": [],
            });
            println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
        } else {
            println!("No active sessions — nothing to tear down.");
        }
        return Ok(ExitCode::Success);
    }

    let repo_root = ws.root().parent().unwrap_or(ws.root()).to_path_buf();
    let home = std::env::home_dir().ok_or_else(|| Error::Io {
        path: PathBuf::from("~"),
        source: std::io::Error::other("cannot determine home directory"),
    })?;
    let ctx = PlanContext {
        scope: Scope::Repo,
        repo_root,
        workspace_root: ws.root().to_path_buf(),
        home,
        session_id: None,
    };

    let mut json_sessions = Vec::new();
    let mut drifted: Vec<(String, Vec<String>)> = Vec::new();
    for id in &ids {
        let session = ledger.sessions.get(id).expect("id from ledger").clone();
        match state::execute_teardown(&session, id, &ctx, force) {
            Ok(steps) => {
                ledger.sessions.remove(id);
                state::save(ws.root(), &ledger)?;
                if cli.json {
                    json_sessions.push(serde_json::json!({
                        "sessionId": id,
                        "status": "torn-down",
                        "steps": steps,
                    }));
                } else {
                    let width = steps
                        .iter()
                        .map(|s| s.path.len())
                        .max()
                        .unwrap_or(0)
                        .max(20);
                    for s in &steps {
                        if s.detail.is_empty() {
                            println!("  {:<8} {}", s.op, s.path);
                        } else {
                            println!("  {:<8} {:<width$}  ({})", s.op, s.path, s.detail);
                        }
                    }
                    println!("✓ session {id} torn down, backups deleted");
                }
            }
            Err(Error::TeardownDrift(paths)) => drifted.push((id.clone(), paths)),
            Err(e) => return Err(e),
        }
    }

    if !drifted.is_empty() {
        if cli.json {
            let envelope = serde_json::json!({
                "schemaVersion": JSON_SCHEMA_VERSION,
                "sessions": json_sessions,
                "drift": drifted
                    .iter()
                    .map(|(id, paths)| serde_json::json!({ "sessionId": id, "paths": paths }))
                    .collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
        } else {
            for (id, paths) in &drifted {
                println!("✗ drift: session {id} files changed after apply:");
                for p in paths {
                    println!("    {p}");
                }
            }
            println!(
                "  → tear down anyway (backup restore for owned files, best-effort key-level \
                 for merges): teardown --force"
            );
        }
        return Ok(ExitCode::Drift);
    }

    if cli.json {
        let envelope = serde_json::json!({
            "schemaVersion": JSON_SCHEMA_VERSION,
            "sessions": json_sessions,
        });
        println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
    }
    Ok(ExitCode::Success)
}

// ---------------------------------------------------------------- current

fn cmd_current(cli: &Cli) -> Result<ExitCode, Error> {
    let ws = workspace(cli)?;
    let ledger = state::load(ws.root())?;

    // §4.2 ordering: by applied time, then id.
    let mut sessions: Vec<(&String, &Session)> = ledger.sessions.iter().collect();
    sessions.sort_by(|a, b| (&a.1.applied_at, a.0).cmp(&(&b.1.applied_at, b.0)));

    if cli.json {
        let envelope = serde_json::json!({
            "schemaVersion": JSON_SCHEMA_VERSION,
            "sessions": sessions
                .iter()
                .map(|(id, s)| serde_json::json!({
                    "sessionId": id,
                    "role": s.role,
                    "target": s.target,
                    "scope": s.scope,
                    "appliedAt": s.applied_at,
                    "actions": s.actions.len(),
                }))
                .collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
        return Ok(ExitCode::Success);
    }

    println!("ACTIVE SESSIONS");
    if sessions.is_empty() {
        println!("  (none)");
        return Ok(ExitCode::Success);
    }
    let id_w = sessions.iter().map(|(id, _)| id.len()).max().unwrap_or(0);
    let role_w = sessions
        .iter()
        .map(|(_, s)| s.role.len())
        .max()
        .unwrap_or(0);
    let target_w = sessions
        .iter()
        .map(|(_, s)| s.target.len())
        .max()
        .unwrap_or(0);
    for (id, s) in &sessions {
        // appliedAt is YYYY-MM-DDTHH:MM:SSZ — show the HH:MM:SS as in §4.2.
        let time = s.applied_at.get(11..19).unwrap_or(&s.applied_at);
        println!(
            "  {:<id_w$}   {:<role_w$} → {:<target_w$}   applied {time}  ({} action{})",
            id,
            s.role,
            s.target,
            s.actions.len(),
            plural(s.actions.len()),
        );
    }
    Ok(ExitCode::Success)
}

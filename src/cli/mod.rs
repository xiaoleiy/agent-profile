//! Command definitions and output formatting (human + `--json`). The thin
//! `main.rs` shell parses args and calls [`run`]; all logic underneath is
//! plain library code.

pub mod resolved;

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand};
use serde::Serialize;

use crate::JSON_SCHEMA_VERSION;
use crate::adapters::{self, Action, Plan, PlanContext, Skipped};
use crate::error::{Error, ExitCode};
use crate::schema::types::Target;
use crate::schema::validate::{self, UnitReport};
use crate::schema::{ResolvedProfile, Workspace, merge};

pub use crate::adapters::Scope;

#[derive(Debug, Parser)]
#[command(
    name = "agent-profile",
    version,
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
        shell: String,
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
        Command::Doctor { .. }
        | Command::Apply { .. }
        | Command::Teardown { .. }
        | Command::Current
        | Command::Completions { .. } => Err(Error::NotImplemented),
    }
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

/// Shared front half of render/diff: resolve, check target, plan, denylist.
fn build_plan(
    cli: &Cli,
    rt: &RoleTargetArgs,
    scope: Scope,
) -> Result<(ResolvedProfile, Target, PlanContext, Plan), Error> {
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
        home,
        session_id: None,
    };
    let adapter = adapters::adapter_for(target).ok_or(Error::NotImplemented)?;
    let plan = adapter.plan(&resolved, &ctx)?;
    // Hardcoded never-touch denylist — enforced regardless of flags.
    adapters::denylist::assert_plan_allowed(&plan, &ctx)?;
    Ok((resolved, target, ctx, plan))
}

fn cmd_render(
    cli: &Cli,
    rt: &RoleTargetArgs,
    out: Option<&std::path::Path>,
    scope: Scope,
) -> Result<ExitCode, Error> {
    let (_resolved, target, _ctx, plan) = build_plan(cli, rt, scope)?;

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
                println!(
                    "{:14}(key-level merge via toml_edit; comments/format preserved;",
                    ""
                );
                println!("{:14} existing tables untouched)", "");
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
    let (_resolved, target, ctx, plan) = build_plan(cli, rt, scope)?;

    let mut diffs: Vec<(String, String)> = Vec::new();
    for action in &plan.actions {
        let abs = ctx.resolve(action.path());
        let current = match std::fs::read_to_string(&abs) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => return Err(Error::Io { path: abs, source }),
        };
        let desired = adapters::desired_file_state(action, current.as_deref())?;
        let cur = current.unwrap_or_default();
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

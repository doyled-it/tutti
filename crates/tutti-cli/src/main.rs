// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `tutti` CLI: load config, acquire the run lock, wire adapters, drain issues.

mod lock;
mod wire;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tutti_core::config::Config;
use tutti_core::engine::Engine;
use tutti_core::message::{PlanAction, PlanDecision};

#[derive(Parser)]
#[command(
    name = "tutti",
    about = "Drive coding agents through a strict, forge-integrated workflow"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Drain ready issues once. Schedule repeated runs externally (cron/launchd).
    Run {
        #[arg(long, default_value = "tutti.toml")]
        config: PathBuf,
        /// The GitHub repo "owner/name".
        #[arg(long)]
        repo: String,
        /// Repo root on disk (where worktrees are created).
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
    },
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run {
            config,
            repo,
            repo_root,
        } => match run(config, repo, repo_root).await {
            Ok((n, plan)) => {
                println!("tutti: shipped {n} issue(s)");
                report_plan(plan.as_ref());
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("tutti: {e}");
                std::process::ExitCode::FAILURE
            }
        },
    }
}

/// Surface the planner's decision to the operator: a stdout line describing the action,
/// plus a prominent stderr notice when the planner wants a human or requests a stop.
fn report_plan(plan: Option<&PlanDecision>) {
    let Some(decision) = plan else {
        return;
    };
    let action = match &decision.action {
        PlanAction::NextIssue => "next-issue".to_string(),
        PlanAction::CreateIssues(list) => format!("create-issues ({})", list.len()),
        PlanAction::CloseMilestone(title) => format!("close-milestone ({title})"),
        PlanAction::Stop => "stop".to_string(),
    };
    println!("tutti: planner -> {action} ({})", decision.rationale);
    let halts = matches!(
        decision.action,
        PlanAction::CloseMilestone(_) | PlanAction::Stop
    );
    if decision.needs_human {
        eprintln!("tutti: planner requests a human: {action}");
    } else if halts {
        eprintln!("tutti: planner requests stop: {action}");
    }
}

async fn run(
    config: PathBuf,
    repo: String,
    repo_root: PathBuf,
) -> Result<(u32, Option<PlanDecision>), String> {
    let cfg = Config::load(&config).map_err(|e| e.to_string())?;
    let _lock = lock::PidLock::acquire(repo_root.join(".tutti").join("run.lock.d"))
        .map_err(|e| format!("could not acquire run lock: {e}"))?;

    let adapters = wire::build(&cfg, &repo, repo_root.clone());
    // Recover any issues a prior crash left in-progress, then prune stale worktrees.
    let _ = adapters.forge.recover_stale().await;
    use tutti_core::workspace::Workspace;
    let _ = adapters.workspace.prune().await;

    let engine = Engine::new(
        &cfg,
        &adapters.forge,
        &adapters.backend,
        Box::new(adapters.workspace),
    )
    .map_err(|e| e.to_string())?;
    let (shipped, plan) = engine.drain().await.map_err(|e| e.to_string())?;
    Ok((shipped, plan))
}

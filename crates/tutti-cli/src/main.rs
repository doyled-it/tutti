// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `tutti` CLI: load config, acquire the run lock, wire adapters, drain issues.

mod green;
mod lock;
mod retrofit;
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
        /// The target: "owner/name" for GitHub and Gitea, a project id or URL-encoded
        /// path ("group%2Fproject") for GitLab.
        #[arg(long)]
        repo: String,
        /// Repo root on disk (where worktrees are created).
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
        /// Forge kind: github (default) | gitea | gitlab. Overrides [forge].kind in config.
        #[arg(long)]
        forge: Option<String>,
        /// Forge login (the `tea` login for gitea). Overrides [forge].login in config.
        #[arg(long)]
        login: Option<String>,
    },
    /// Install Tutti's opinionated tooling into an existing repo (detect language, add the
    /// gate + CI + AGENTS.md, merge config), then run the gate for a baseline.
    Retrofit {
        /// Repo root on disk.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Print the plan and stop, writing nothing.
        #[arg(long)]
        dry_run: bool,
        /// Apply without the interactive confirmation (CI/scripts).
        #[arg(long)]
        yes: bool,
        /// After a successful retrofit, immediately run `tutti green` on the result.
        /// Requires `--repo`.
        #[arg(long)]
        then_green: bool,
        /// The forge target for `--then-green` ("owner/name" etc, see `Run`).
        #[arg(long)]
        repo: Option<String>,
    },
    /// Drive the agent to a green gate per target, opening a PR per target.
    Green {
        /// Repo root on disk (where `tutti.toml` lives and the greening worktree is made).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// The target: "owner/name" for GitHub and Gitea, a project id or URL-encoded
        /// path ("group%2Fproject") for GitLab.
        #[arg(long)]
        repo: String,
        /// Forge kind: github (default) | gitea | gitlab. Overrides [forge].kind in config.
        #[arg(long)]
        forge: Option<String>,
        /// Forge login (the `tea` login for gitea). Overrides [forge].login in config.
        #[arg(long)]
        login: Option<String>,
        /// Max greener iterations per target before giving up.
        #[arg(long, default_value_t = 5)]
        max_iters: u32,
        /// Discard and recreate any existing greening branch for a target.
        #[arg(long)]
        fresh: bool,
        /// Branch each greening branch is cut from. Defaults to the config's integration branch.
        #[arg(long)]
        base: Option<String>,
        /// Run this single command as the only target instead of discovering per-language gates.
        #[arg(long)]
        gate: Option<String>,
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
            forge,
            login,
        } => match run(config, repo, repo_root, forge, login).await {
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
        Cmd::Retrofit {
            path,
            dry_run,
            yes,
            then_green,
            repo,
        } => {
            if then_green && repo.is_none() {
                eprintln!("tutti: --then-green requires --repo");
                return std::process::ExitCode::FAILURE;
            }
            match retrofit::run(path.clone(), dry_run, yes).await {
                Ok(()) => {
                    if dry_run || !then_green {
                        return std::process::ExitCode::SUCCESS;
                    }
                    let repo = repo.expect("checked above");
                    // Retrofit only writes to the working tree; commit the output so the
                    // greening worktree (forked from HEAD) sees the new gate + tooling.
                    // Best-effort: if there is nothing to commit, ignore the failure and
                    // proceed to greening anyway.
                    let _ = std::process::Command::new("git")
                        .args(["-C", &path.to_string_lossy(), "add", "-A"])
                        .status();
                    let _ = std::process::Command::new("git")
                        .args([
                            "-C",
                            &path.to_string_lossy(),
                            "commit",
                            "-m",
                            "chore: install Tutti tooling (retrofit)",
                        ])
                        .status();
                    match green::run(path, repo, None, None, 5, false, None, None).await {
                        Ok(()) => std::process::ExitCode::SUCCESS,
                        Err(e) => {
                            eprintln!("tutti: {e}");
                            std::process::ExitCode::FAILURE
                        }
                    }
                }
                Err(e) => {
                    eprintln!("tutti: {e}");
                    std::process::ExitCode::FAILURE
                }
            }
        }
        Cmd::Green {
            path,
            repo,
            forge,
            login,
            max_iters,
            fresh,
            base,
            gate,
        } => match green::run(path, repo, forge, login, max_iters, fresh, base, gate).await {
            Ok(()) => std::process::ExitCode::SUCCESS,
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
    forge: Option<String>,
    login: Option<String>,
) -> Result<(u32, Option<PlanDecision>), String> {
    use std::str::FromStr;
    let cfg = Config::load(&config).map_err(|e| e.to_string())?;

    // CLI overrides config; config supplies the default; ForgeKind defaults to GitHub.
    let kind = match forge {
        Some(s) => tutti_core::config::ForgeKind::from_str(&s).map_err(|e| e.to_string())?,
        None => cfg.forge.kind,
    };
    let login = login.or_else(|| cfg.forge.login.clone());

    let _lock = lock::PidLock::acquire(repo_root.join(".tutti").join("run.lock.d"))
        .map_err(|e| format!("could not acquire run lock: {e}"))?;

    let adapters = wire::build(&cfg, kind, login.as_deref(), &repo, repo_root.clone())
        .map_err(|e| e.to_string())?;
    // Recover any issues a prior crash left in-progress, then prune stale worktrees.
    let _ = adapters.forge.recover_stale().await;
    use tutti_core::workspace::Workspace;
    let _ = adapters.workspace.prune().await;

    // The conventions provider injects the per-language convention skill into the coding
    // roles. Declared before the engine so the binding outlives it, as the context does.
    let conventions = tutti_app_core::ConventionsSkill;
    let engine = Engine::new(
        &cfg,
        adapters.forge.as_ref(),
        &adapters.backend,
        Box::new(adapters.workspace),
    )
    .map_err(|e| e.to_string())?
    .with_conventions(&conventions);
    // codegraph context, gated by config and binary presence. `detect` returns None when
    // the binary is absent, so this is a full no-op on machines without codegraph.
    let codegraph = if cfg.codegraph_enabled() {
        tutti_core::context::CodeGraph::detect(repo_root.clone())
    } else {
        None
    };
    let engine = match &codegraph {
        Some(cg) => engine.with_context(cg),
        None => engine,
    };
    let (shipped, plan) = engine.drain().await.map_err(|e| e.to_string())?;
    Ok((shipped, plan))
}

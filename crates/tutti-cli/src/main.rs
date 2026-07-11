// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `tutti` CLI: load config, acquire the run lock, wire adapters, drain issues.

mod lock;
mod wire;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tutti_core::config::Config;
use tutti_core::engine::Engine;

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
    /// Drain ready issues once (or repeatedly with --loop).
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
            Ok(n) => {
                println!("tutti: shipped {n} issue(s)");
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("tutti: {e}");
                std::process::ExitCode::FAILURE
            }
        },
    }
}

async fn run(config: PathBuf, repo: String, repo_root: PathBuf) -> Result<u32, String> {
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
    let (shipped, _plan) = engine.drain().await.map_err(|e| e.to_string())?;
    Ok(shipped)
}

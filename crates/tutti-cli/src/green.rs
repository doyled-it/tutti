// SPDX-License-Identifier: AGPL-3.0-or-later
//! `tutti green`: drive the agent to a green gate per target, opening a PR per target.

use std::path::PathBuf;
use std::str::FromStr;
use tutti_core::config::Config;
use tutti_core::greening::{discover_targets, green_all, GreenOptions, GreenOutcome};

/// Resolve the current git branch name at `repo`. Greening forks its worktree from HEAD
/// (the branch that actually has the code and the gate), not the configured integration
/// branch, so a target PRs into the branch it was built on unless `--base` overrides it.
fn current_branch(repo: &std::path::Path) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repo)
        .output()
        .map_err(|e| format!("git rev-parse: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "could not resolve the current branch in {}",
            repo.display()
        ));
    }
    let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if b.is_empty() || b == "HEAD" {
        return Err("greening needs a named current branch (detached HEAD)".into());
    }
    Ok(b)
}

/// Run `tutti green`. `path` is the repo root (where `tutti.toml` lives and where the
/// greening worktree is created); `repo` is the forge-specific target, matching `Run`.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    path: PathBuf,
    repo: String,
    forge: Option<String>,
    login: Option<String>,
    max_iters: u32,
    fresh: bool,
    base: Option<String>,
    gate: Option<String>,
) -> Result<(), String> {
    let config_path = path.join("tutti.toml");
    let cfg = Config::load(&config_path).map_err(|_| {
        format!(
            "tutti green needs a tutti.toml at {} (run the wizard / retrofit first)",
            config_path.display()
        )
    })?;

    // CLI overrides config; config supplies the default; ForgeKind defaults to GitHub.
    let kind = match forge {
        Some(s) => tutti_core::config::ForgeKind::from_str(&s).map_err(|e| e.to_string())?,
        None => cfg.forge.kind,
    };
    let login = login.or_else(|| cfg.forge.login.clone());

    let adapters = crate::wire::build(&cfg, kind, login.as_deref(), &repo, path.clone())
        .map_err(|e| e.to_string())?;

    let config_gate = cfg.gate.commands.clone();
    let targets = discover_targets(&path, &config_gate, gate.as_deref());
    let opts = GreenOptions {
        max_iters,
        fresh,
        base: match base {
            Some(b) => b,
            None => current_branch(&path)?,
        },
        model: cfg.model.clone(),
    };
    let workspace = tutti_git::GitGreenWorkspace::new(path.clone());

    println!("Greening {} target(s)...", targets.len());
    let results = green_all(
        &targets,
        &opts,
        &adapters.backend,
        &workspace,
        adapters.forge.as_ref(),
    )
    .await;

    let mut any_fail = false;
    for r in &results {
        match &r.outcome {
            GreenOutcome::AlreadyGreen => {
                println!("  = {} ({}): already green", r.label, r.branch)
            }
            GreenOutcome::Greened { iters, pr, flags } => {
                println!(
                    "  + {} ({}): green in {iters} iter(s) -> PR #{pr}",
                    r.label, r.branch
                );
                for f in flags {
                    println!("      flag: {f}");
                }
            }
            GreenOutcome::Exhausted { iters, gate_log } => {
                any_fail = true;
                println!(
                    "  ! {} ({}): still red after {iters} iter(s); branch left to resume",
                    r.label, r.branch
                );
                for line in gate_log.lines() {
                    println!("        {line}");
                }
            }
            GreenOutcome::Rejected { reason } => {
                any_fail = true;
                println!("  x {} ({}): rejected ({reason})", r.label, r.branch);
            }
            GreenOutcome::Error(e) => {
                any_fail = true;
                println!("  x {} ({}): error ({e})", r.label, r.branch);
            }
        }
    }
    if any_fail {
        return Err("one or more targets did not green".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn errors_without_a_tutti_toml() {
        let d = tempfile::tempdir().unwrap();
        let e = run(
            d.path().to_path_buf(),
            "o/r".into(),
            None,
            None,
            5,
            false,
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(e.contains("tutti.toml"), "got: {e}");
    }
}

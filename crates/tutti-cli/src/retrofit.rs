// SPDX-License-Identifier: AGPL-3.0-or-later
//! `tutti retrofit`: detect an existing repo's language, preview the opinionated tooling
//! it is missing, apply it on confirmation, and report the baseline gate gap.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tutti_app_core::{
    apply_retrofit, detect_languages, package_name, plan_retrofit, run_baseline_gate,
    stack_profile, RetrofitPlan, ScaffoldContext,
};

/// Print the plan for the operator. Returns true if there is anything to do.
fn print_plan(plan: &RetrofitPlan) -> bool {
    let anything =
        !plan.adds.is_empty() || !plan.merges.is_empty() || !plan.gitignore_append.is_empty();
    if !anything {
        println!("Already Tutti-ready: nothing to add or merge.");
        return false;
    }
    if !plan.adds.is_empty() {
        println!("Add:");
        for f in &plan.adds {
            println!("  + {}", f.path.display());
        }
    }
    if !plan.merges.is_empty() {
        println!("\nMerge (your file is edited; diff below):");
        for m in &plan.merges {
            println!("  ~ {}", m.path.display());
            if let Some(note) = &m.note {
                println!("      note: {note}");
            }
            for line in m.diff.lines() {
                println!("      {line}");
            }
        }
    }
    if !plan.gitignore_append.is_empty() {
        println!("\nAppend to .gitignore:");
        for l in &plan.gitignore_append {
            println!("  + {l}");
        }
    }
    if !plan.skipped.is_empty() {
        println!("\nSkip (already present, left untouched):");
        for s in &plan.skipped {
            println!("  = {} ({})", s.path.display(), s.why);
        }
    }
    if !plan.already.is_empty() {
        println!("\nAlready configured (left untouched):");
        for p in &plan.already {
            println!("  = {}", p.display());
        }
    }
    anything
}

fn confirm(prompt: &str) -> bool {
    print!("{prompt} [y/N] ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Run `tutti retrofit`. `dry_run` prints the plan and stops; `yes` skips the prompt.
pub async fn run(path: PathBuf, dry_run: bool, yes: bool) -> Result<(), String> {
    let langs = detect_languages(&path);
    let stack_id = match langs.as_slice() {
        [] => return Err("no recognized language at that path (looked for pyproject.toml, Cargo.toml, package.json/tsconfig.json, go.mod)".into()),
        [one] => one.clone(),
        many => {
            return Err(format!(
                "found multiple languages ({}). This slice retrofits one language at a time; point at a single-language repo.",
                many.join(", ")
            ))
        }
    };
    let profile = stack_profile(&stack_id).ok_or_else(|| format!("no profile for {stack_id}"))?;
    let repo_name = repo_name_of(&path);
    let ctx = ScaffoldContext {
        package_name: package_name(&repo_name),
        repo_name,
    };

    println!("Detected: {} ({})\n", profile.display_name, stack_id);
    let plan = plan_retrofit(&path, &profile, &ctx);
    let anything = print_plan(&plan);

    if dry_run || !anything {
        return Ok(());
    }
    if !yes && !confirm("\nApply?") {
        println!("Aborted; nothing written.");
        return Ok(());
    }
    let report = apply_retrofit(&path, &plan).map_err(|e| format!("apply failed: {e}"))?;
    println!(
        "\nWrote {} file(s), merged {} config file(s), appended {} .gitignore line(s).",
        report.written.len(),
        report.merged.len(),
        report.gitignore_appended
    );

    println!("\nRunning the gate for a baseline...");
    match run_baseline_gate(&path, &profile).await {
        Ok(outcome) if outcome.passed => {
            println!("Gate passes. This repo is already green under the strict rails.");
        }
        Ok(outcome) => {
            println!(
                "Gate is RED (expected on legacy code). Getting to green is your migration:\n\n{}",
                outcome.log
            );
        }
        Err(e) => println!(
            "Could not run the gate ({e}); install the toolchain and run `bash scripts/check.sh`."
        ),
    }
    Ok(())
}

/// The repo name for the scaffold context: the directory's file name.
fn repo_name_of(path: &Path) -> String {
    path.canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "app".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dry_run_writes_nothing() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        run(d.path().to_path_buf(), true, false).await.unwrap();
        assert!(!d.path().join("scripts/check.sh").exists());
    }

    #[tokio::test]
    async fn yes_applies_without_prompting() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        run(d.path().to_path_buf(), false, true).await.unwrap();
        assert!(d.path().join("scripts/check.sh").exists());
        assert!(d.path().join("AGENTS.md").exists());
    }

    #[tokio::test]
    async fn no_language_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("README.md"), "hi\n").unwrap();
        assert!(run(d.path().to_path_buf(), true, false).await.is_err());
    }
}

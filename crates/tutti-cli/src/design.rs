// SPDX-License-Identifier: AGPL-3.0-or-later
//! `tutti design`: an interactive CLI that drives the design chain (the E2/E5 facilitation
//! loop) to a ratified design, grounds an existing repo first, then turns the ratified design
//! into a seedable backlog via a post-chain decompose pass and hands off (new repo: scaffold +
//! seed; existing repo: seed). Resumable and branchable from `.tutti/design/`.
//!
//! The wiring lives here. The real `Facilitator` is a thin adapter over `ClaudeSession::turn`
//! (`ClaudeFacilitator`); the interactive driver is written over an injected `Prompter` and the
//! `Facilitator` seam so it is hermetically testable with a fake of each. The full chain against
//! a real `claude` is a live `#[ignore]` smoke.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tutti_app_core::{
    detect_languages, package_name, run_post_write, scaffold, stack_profile, RepoGroundingReader,
    ScaffoldContext, StackProfile,
};
use tutti_backend_claude::session::ClaudeSession;
use tutti_core::config::{Config, ForgeKind};
use tutti_core::message::AgentEvent;
use tutti_design::{
    advance, backlog_prompt, definition, parse_backlog, render_page, render_plan, seed, store,
    DesignError, DesignPage, FacilitationInput, FacilitationState, Facilitator, MovementId,
    ProjectShape, RawTurn, RepoGrounder, Section, SessionState, Skill,
};

/// The real `Facilitator`: one facilitation turn is one `ClaudeSession::turn` in `cwd`,
/// resuming the session id from the prior turn. The streamed agent events are drained to a
/// sink for now (a later surface can show them live); the turn's outcome is its full reply
/// text plus the resumable session id, which is exactly what a `RawTurn` carries.
pub struct ClaudeFacilitator {
    pub session: ClaudeSession,
    pub model: String,
    pub cwd: PathBuf,
}

impl Facilitator for ClaudeFacilitator {
    async fn turn(&self, prompt: &str, resume: Option<&str>) -> Result<RawTurn, DesignError> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);
        // Drain the streamed events so the bounded channel never backs up the turn. A live
        // surface can replace this sink with a real renderer later.
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let outcome = self
            .session
            .turn(prompt, &self.model, resume, None, &self.cwd, None, tx)
            .await
            .map_err(|e| DesignError::Facilitation(format!("claude turn: {e}")))?;
        let _ = drain.await;
        Ok(RawTurn {
            session_id: outcome.session_id.unwrap_or_default(),
            output: outcome.assistant_text,
        })
    }
}

/// Resolve the `skills/` directory that holds the movement facilitation skills. One resolver so
/// the shipped-skills packaging (bundled beside the binary, like codegraph) has a single place to
/// change: the `TUTTI_SKILLS_DIR` env override wins, else a `skills/` dir beside the running
/// binary, else the dev/test fallback of `CARGO_MANIFEST_DIR/../../skills` (the repo's own).
fn skills_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("TUTTI_SKILLS_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let beside = parent.join("skills");
            if beside.is_dir() {
                return beside;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills")
}

/// Load a movement's facilitation skill from `skills/design/<name>/`, via the single
/// `skills_dir` resolver.
pub fn movement_skill(id: MovementId) -> Result<Skill, DesignError> {
    let dir = skills_dir()
        .join("design")
        .join(tutti_design::skill_dir_name(id));
    tutti_design::load_skill(&dir)
}

/// The human side of the loop, behind a trait so the driver is testable with a scripted fake.
pub trait Prompter {
    /// Show `question` and read one line of input.
    fn ask(&mut self, question: &str) -> std::io::Result<String>;
    /// Display text to the human (a proposed artifact, a status line).
    fn show(&mut self, text: &str);
    /// Ask a yes/no question, defaulting to no.
    fn confirm(&mut self, prompt: &str) -> std::io::Result<bool>;
}

/// Drive one movement to ratification over the `Facilitator` seam and the injected `Prompter`,
/// persisting after each step (`advance` writes the session). The loop turns each
/// `FacilitationState` into the next human input: a question is asked and answered, a proposed
/// artifact is shown and either ratified or sent back for revision, and ratification ends it.
pub async fn drive_movement(
    fac: &impl Facilitator,
    prompter: &mut impl Prompter,
    skill: &Skill,
    movement: MovementId,
    session: &mut SessionState,
    repo_root: &Path,
) -> Result<(), DesignError> {
    let mut input = FacilitationInput::Begin;
    loop {
        let state = advance(fac, skill, movement, session, repo_root, input).await?;
        input = match state {
            FacilitationState::AwaitingHuman { question } => {
                let answer = prompter
                    .ask(&question)
                    .map_err(|e| DesignError::Facilitation(format!("reading input: {e}")))?;
                FacilitationInput::Reply(answer)
            }
            FacilitationState::AwaitingRatification { artifact_section } => {
                prompter.show(&artifact_section);
                let ratify = prompter
                    .confirm("Ratify this section?")
                    .map_err(|e| DesignError::Facilitation(format!("reading input: {e}")))?;
                if ratify {
                    FacilitationInput::Accept
                } else {
                    let feedback = prompter
                        .ask("What should change?")
                        .map_err(|e| DesignError::Facilitation(format!("reading input: {e}")))?;
                    FacilitationInput::Revise(feedback)
                }
            }
            FacilitationState::Ratified { .. } => return Ok(()),
        };
    }
}

/// Drive the whole chain to completion: for each remaining movement, load its facilitation
/// skill and `drive_movement` it to ratification, until `session.is_complete()`. State is
/// persisted throughout by `advance`, so a stop between movements resumes here.
pub async fn drive_chain(
    fac: &impl Facilitator,
    prompter: &mut impl Prompter,
    session: &mut SessionState,
    repo_root: &Path,
) -> Result<(), DesignError> {
    while let Some(movement) = session.current() {
        let skill = movement_skill(movement)?;
        drive_movement(fac, prompter, &skill, movement, session, repo_root).await?;
    }
    Ok(())
}

/// The real `Prompter`: print to stdout, read a line from stdin, confirm defaulting to no.
struct StdioPrompter;

impl Prompter for StdioPrompter {
    fn ask(&mut self, question: &str) -> io::Result<String> {
        println!("\n{question}");
        print!("> ");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        Ok(line.trim().to_string())
    }

    fn show(&mut self, text: &str) {
        println!("\n{text}");
    }

    fn confirm(&mut self, prompt: &str) -> io::Result<bool> {
        print!("{prompt} [y/N] ");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
    }
}

/// Ask the human which project shape a greenfield session should run.
fn ask_shape(prompter: &mut impl Prompter) -> Result<ProjectShape, String> {
    loop {
        let answer = prompter
            .ask("Project shape? [1] small CLI or library  [2] mobile app  [3] multi-service")
            .map_err(|e| e.to_string())?;
        match answer.trim().to_lowercase().as_str() {
            "1" | "small" | "small_cli" | "cli" | "library" => return Ok(ProjectShape::SmallCli),
            "2" | "mobile" => return Ok(ProjectShape::Mobile),
            "3" | "multi" | "multi_service" | "multiservice" => {
                return Ok(ProjectShape::MultiService)
            }
            _ => println!("Please answer 1, 2, or 3."),
        }
    }
}

/// Ask the human which language scaffold a new repo should be created with, returning the
/// chosen profile. The choices are the built-in stack ids.
fn ask_stack(prompter: &mut impl Prompter) -> Result<StackProfile, String> {
    loop {
        let answer = prompter
            .ask("New-repo language? [python] [rust] [typescript] [go]")
            .map_err(|e| e.to_string())?;
        if let Some(profile) = stack_profile(answer.trim().to_lowercase().as_str()) {
            return Ok(profile);
        }
        println!("Unknown stack. Choose one of: python, rust, typescript, go.");
    }
}

/// Build a best-effort design page from the session's ratified artifacts. This is E7's thin
/// handoff render, not E3's rich page: each ratified section's markdown is escaped into a
/// `<pre>` block so nothing is lost, and the real diagram-aware render remains E3's to own.
fn design_page_from_session(session: &SessionState, title: &str) -> DesignPage {
    let sections = session
        .artifacts
        .iter()
        .enumerate()
        .map(|(i, artifact)| {
            let def = definition(artifact.movement);
            Section {
                eyebrow: format!("MOVEMENT {} - {}", i + 1, def.title.to_uppercase()),
                heading: def.title.to_string(),
                body_html: format!("<pre>{}</pre>", escape_html(&artifact.section)),
                diagrams: Vec::new(),
            }
        })
        .collect();
    DesignPage {
        title: title.to_string(),
        subtitle: "A Tutti design".to_string(),
        sections,
    }
}

/// Minimal HTML escaping for inlining plain text as a `<pre>` body.
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The repo name for scaffold/context and the design-page title: the directory's file name,
/// falling back to "app".
fn repo_name_of(path: &Path) -> String {
    path.canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .or_else(|| path.file_name().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "app".to_string())
}

/// Run `tutti design`. Drives the design chain to a ratified design, then decomposes it into a
/// backlog and hands off (new repo: scaffold + seed; existing repo: seed). `repo` is the repo
/// path on disk (also where `.tutti/design/` lives); `target`/`forge`/`login` name the forge to
/// seed onto (seeding is skipped, with a note, when no `target` is given).
#[allow(clippy::too_many_arguments)]
pub async fn run(
    repo: Option<PathBuf>,
    resume: bool,
    branch: Option<String>,
    config: PathBuf,
    target: Option<String>,
    forge: Option<String>,
    login: Option<String>,
) -> Result<(), String> {
    let cfg = Config::load(&config).map_err(|e| e.to_string())?;
    let repo_root = repo.clone().unwrap_or_else(|| PathBuf::from("."));

    // Minimal branch: snapshot the current session under a branch name before this run mutates
    // it, so a design variant can be preserved. Running a fully independent session per branch
    // (a separate live session file the driver reads and writes) is follow-up work; see the
    // plan's out-of-scope note.
    if let Some(name) = &branch {
        let src = store::session_path(&repo_root);
        if src.exists() {
            let dir = repo_root.join(".tutti").join("design").join("branches");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let dst = dir.join(format!("{name}.json"));
            std::fs::copy(&src, &dst).map_err(|e| e.to_string())?;
            println!(
                "tutti: snapshotted the current session to {}",
                dst.display()
            );
        }
    }

    let mut prompter = StdioPrompter;

    // Determine the session: resume an existing one, or start a fresh one (grounded in an
    // existing repo, or greenfield with a chosen shape).
    let langs = detect_languages(&repo_root);
    let is_existing_repo = !langs.is_empty();

    let mut session = if resume {
        store::load(&repo_root)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                format!(
                    "no design session to resume at {} (run without --resume to start one)",
                    store::session_path(&repo_root).display()
                )
            })?
    } else if is_existing_repo {
        // Ground the existing repo, confirm (or override) the inferred shape, and carry the
        // grounding into the chain so grounded movements confirm rather than ask from scratch.
        let grounding = RepoGroundingReader
            .ground(&repo_root)
            .map_err(|e| e.to_string())?;
        println!(
            "Grounded an existing {} repo; inferred shape: {:?}.",
            langs.join(", "),
            grounding.inferred_shape
        );
        let shape = if prompter
            .confirm("Use the inferred shape?")
            .map_err(|e| e.to_string())?
        {
            grounding.inferred_shape
        } else {
            ask_shape(&mut prompter)?
        };
        let mut s = SessionState::new(shape);
        s.grounding = Some(grounding);
        store::save(&repo_root, &s).map_err(|e| e.to_string())?;
        s
    } else {
        let shape = ask_shape(&mut prompter)?;
        let s = SessionState::new(shape);
        store::save(&repo_root, &s).map_err(|e| e.to_string())?;
        s
    };

    // Drive the chain to completion over the real Claude facilitator.
    let fac = ClaudeFacilitator {
        session: ClaudeSession::default(),
        model: cfg.model.clone(),
        cwd: repo_root.clone(),
    };
    if !session.is_complete() {
        drive_chain(&fac, &mut prompter, &mut session, &repo_root)
            .await
            .map_err(|e| e.to_string())?;
    }

    println!("tutti: design ratified.");

    // The handoff: one post-chain decompose turn produces the backlog, then scaffold (new repo)
    // and seed.
    handoff(
        &fac,
        &mut prompter,
        &session,
        &repo_root,
        is_existing_repo,
        &cfg,
        target,
        forge,
        login,
    )
    .await
}

/// The post-chain handoff: run one decompose turn to get a `BacklogPlan`, render it for review,
/// then (new repo) scaffold a chosen stack and (either repo) seed the backlog onto the forge.
/// Also writes a best-effort `design.html` from the ratified artifacts.
#[allow(clippy::too_many_arguments)]
async fn handoff(
    fac: &ClaudeFacilitator,
    prompter: &mut impl Prompter,
    session: &SessionState,
    repo_root: &Path,
    is_existing_repo: bool,
    cfg: &Config,
    target: Option<String>,
    forge: Option<String>,
    login: Option<String>,
) -> Result<(), String> {
    // Best-effort design page from the ratified artifacts (E3 owns the rich render).
    let title = repo_name_of(repo_root);
    let page = design_page_from_session(session, &title);
    let page_path = repo_root.join("design.html");
    if let Err(e) = std::fs::write(&page_path, render_page(&page)) {
        println!("tutti: could not write {} ({e})", page_path.display());
    } else {
        println!("tutti: wrote {}", page_path.display());
    }

    // One decompose turn: ask the agent to emit a BacklogPlan JSON from the ratified design.
    let raw = fac
        .turn(&backlog_prompt(session), None)
        .await
        .map_err(|e| e.to_string())?;
    let plan = parse_backlog(&raw.output).map_err(|e| e.to_string())?;
    prompter.show(&render_plan(&plan));

    if !prompter
        .confirm("Seed this backlog?")
        .map_err(|e| e.to_string())?
    {
        println!("tutti: backlog not seeded.");
        return Ok(());
    }

    // New repo: scaffold a chosen stack before seeding. Existing repo: seed onto it as-is.
    if !is_existing_repo {
        let profile = ask_stack(prompter)?;
        let repo_name = repo_name_of(repo_root);
        let ctx = ScaffoldContext {
            package_name: package_name(&repo_name),
            repo_name,
        };
        let report = scaffold(repo_root, &profile, &ctx, &|s| run_post_write(repo_root, s))
            .map_err(|e| format!("scaffold failed: {e}"))?;
        println!(
            "tutti: scaffolded {} ({} file(s) written, {} skipped).",
            profile.display_name,
            report.written.len(),
            report.skipped.len()
        );
        for warning in &report.warnings {
            println!("tutti: {warning}");
        }
    }

    // Seed the backlog onto the forge. Needs a forge target; without one, report and stop.
    let Some(target) = target else {
        println!(
            "tutti: no --target given, so the backlog was not seeded onto a forge. \
             Re-run with --target <owner/name> to seed it."
        );
        return Ok(());
    };
    let kind = match forge {
        Some(s) => ForgeKind::from_str(&s).map_err(|e| e.to_string())?,
        None => cfg.forge.kind,
    };
    let login = login.or_else(|| cfg.forge.login.clone());
    let adapters = crate::wire::build(
        cfg,
        kind,
        login.as_deref(),
        &target,
        repo_root.to_path_buf(),
    )
    .map_err(|e| e.to_string())?;
    let ready_label = cfg.status_labels().ready;
    let seed_report = seed(&plan, adapters.forge.as_ref(), &ready_label)
        .await
        .map_err(|e| e.to_string())?;
    println!(
        "tutti: seeded {} issue(s) ({} already present).",
        seed_report.created.len(),
        seed_report.skipped.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use tutti_design::{store, ProjectShape};

    /// A scripted facilitator: hands back the queued replies in order. The same shape E2 uses
    /// for its hermetic loop tests.
    struct FakeFacilitator {
        replies: std::cell::RefCell<VecDeque<String>>,
    }
    impl FakeFacilitator {
        fn new(replies: Vec<&str>) -> Self {
            Self {
                replies: std::cell::RefCell::new(replies.into_iter().map(String::from).collect()),
            }
        }
    }
    impl Facilitator for FakeFacilitator {
        async fn turn(&self, _prompt: &str, _resume: Option<&str>) -> Result<RawTurn, DesignError> {
            let out = self
                .replies
                .borrow_mut()
                .pop_front()
                .expect("FakeFacilitator ran out of scripted replies");
            Ok(RawTurn {
                session_id: "sid".into(),
                output: out,
            })
        }
    }

    /// A scripted prompter: answers and confirmations dequeued in order; `shown` records what
    /// the driver displayed.
    struct FakePrompter {
        answers: VecDeque<String>,
        confirms: VecDeque<bool>,
        shown: Vec<String>,
    }
    impl FakePrompter {
        fn new(answers: Vec<&str>, confirms: Vec<bool>) -> Self {
            Self {
                answers: answers.into_iter().map(String::from).collect(),
                confirms: confirms.into_iter().collect(),
                shown: Vec::new(),
            }
        }
    }
    impl Prompter for FakePrompter {
        fn ask(&mut self, _question: &str) -> std::io::Result<String> {
            Ok(self
                .answers
                .pop_front()
                .expect("FakePrompter ran out of scripted answers"))
        }
        fn show(&mut self, text: &str) {
            self.shown.push(text.to_string());
        }
        fn confirm(&mut self, _prompt: &str) -> std::io::Result<bool> {
            Ok(self
                .confirms
                .pop_front()
                .expect("FakePrompter ran out of scripted confirmations"))
        }
    }

    #[tokio::test]
    async fn driver_runs_one_movement_to_ratification() {
        let repo = tempfile::tempdir().unwrap();
        // The agent asks once, then completes with the Constitution section.
        let fac = FakeFacilitator::new(vec![
            r#"{"ask": "What must stay true?"}"#,
            r###"{"complete": {"artifact_section": "## Constitution\nprivacy first"}}"###,
        ]);
        // The human answers the question, then ratifies.
        let mut prompter = FakePrompter::new(vec!["privacy first"], vec![true]);

        // The first movement of a SmallCli session is Constitution; load its real skill.
        let mut session = SessionState::new(ProjectShape::SmallCli);
        let movement = session.current().expect("a fresh session has a movement");
        assert_eq!(movement, MovementId::Constitution);
        let skill = movement_skill(movement).expect("the Constitution skill loads");

        drive_movement(
            &fac,
            &mut prompter,
            &skill,
            movement,
            &mut session,
            repo.path(),
        )
        .await
        .expect("the movement drives to ratification");

        // The movement is ratified in the session and the proposed artifact was shown.
        assert!(session.ratified.contains(&MovementId::Constitution));
        assert_eq!(session.artifacts.len(), 1);
        assert!(session.active.is_none());
        assert!(prompter.shown.iter().any(|s| s.contains("privacy first")));

        // It persisted: a fresh load from disk sees the ratified movement.
        let reloaded = store::load(repo.path()).unwrap().expect("a saved session");
        assert!(reloaded.ratified.contains(&MovementId::Constitution));
    }

    #[tokio::test]
    async fn a_rejected_section_is_revised_then_ratified() {
        let repo = tempfile::tempdir().unwrap();
        // The agent completes, is asked to revise, then completes again.
        let fac = FakeFacilitator::new(vec![
            r###"{"complete": {"artifact_section": "## Constitution\nv1"}}"###,
            r###"{"complete": {"artifact_section": "## Constitution\nv2 tightened"}}"###,
        ]);
        // Reject the first section (with revision feedback), ratify the second.
        let mut prompter = FakePrompter::new(vec!["tighten it"], vec![false, true]);

        let mut session = SessionState::new(ProjectShape::SmallCli);
        let movement = session.current().unwrap();
        let skill = movement_skill(movement).unwrap();
        drive_movement(
            &fac,
            &mut prompter,
            &skill,
            movement,
            &mut session,
            repo.path(),
        )
        .await
        .unwrap();

        assert!(session.ratified.contains(&MovementId::Constitution));
        // The ratified artifact is the revised one.
        assert!(session.artifacts[0].section.contains("v2 tightened"));
    }

    /// A live prompter for the smoke: answer every agent question with the same guidance and
    /// ratify every proposed section, so a real chain drives to completion unattended.
    struct AutoPrompter {
        answer: String,
    }
    impl Prompter for AutoPrompter {
        fn ask(&mut self, _question: &str) -> std::io::Result<String> {
            Ok(self.answer.clone())
        }
        fn show(&mut self, _text: &str) {}
        fn confirm(&mut self, _prompt: &str) -> std::io::Result<bool> {
            Ok(true)
        }
    }

    /// Live end-to-end smoke: drive a real `claude` session through the whole SmallCli chain,
    /// run the post-chain decompose pass, and prove a backlog and a `design.html` are produced.
    /// Ignored by default so the hermetic gate needs no `claude`; run with
    /// `env -C <repo> cargo test -p tutti-cli -- --ignored live_smoke`.
    #[tokio::test]
    #[ignore = "live: needs claude -p on PATH"]
    async fn live_smoke_drives_a_session_and_decomposes() {
        let repo = tempfile::tempdir().unwrap();
        let fac = ClaudeFacilitator {
            session: ClaudeSession::default(),
            model: "sonnet".into(),
            cwd: repo.path().to_path_buf(),
        };
        let mut prompter = AutoPrompter {
            answer: "Keep it minimal: a small CLI for solo developers, offline-first, one \
                     command. Nothing else is in scope."
                .into(),
        };
        let mut session = SessionState::new(ProjectShape::SmallCli);

        drive_chain(&fac, &mut prompter, &mut session, repo.path())
            .await
            .expect("the chain drives to completion against real claude");
        assert!(session.is_complete());

        let raw = fac
            .turn(&backlog_prompt(&session), None)
            .await
            .expect("the decompose turn runs");
        let plan = parse_backlog(&raw.output).expect("a BacklogPlan comes back");
        assert!(plan.issue_count() > 0, "the decompose pass produced issues");

        let page = design_page_from_session(&session, "smoke");
        let page_path = repo.path().join("design.html");
        std::fs::write(&page_path, render_page(&page)).unwrap();
        assert!(page_path.exists(), "a design.html was written");
    }
}

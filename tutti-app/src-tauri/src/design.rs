// SPDX-License-Identifier: AGPL-3.0-or-later
//! The design surface: Tauri commands that drive the design chain one movement turn at a
//! time over `tutti_design::facilitate::advance`, forward the streamed agent turn onto a
//! `design://delta` event, render a live preview of the accreting design page, and
//! propose/seed the resulting backlog. The frontend is the prompter: each discrete command
//! is one load -> advance -> return, so no CLI stdin loop is needed.
//!
//! `FacilitationState`, `FacilitationInput`, and `SeedReport` are not `Serialize`, so the
//! wire types here are small app-side DTOs (`DesignStep`, `SeedReportDto`) mapped from them;
//! `BacklogPlan` already round-trips through serde and is used directly.

use crate::state::AppState;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, State};
use tokio::sync::mpsc;
use tutti_app_core::{detect_languages, RepoGroundingReader};
use tutti_backend_claude::session::ClaudeSession;
use tutti_core::message::AgentEvent;
use tutti_design::{
    advance, backlog_prompt, definition, parse_backlog, render_page, render_plan, seed, store,
    BacklogPlan, DesignError, DesignPage, FacilitationInput, FacilitationState, Facilitator,
    MovementId, ProjectShape, RawTurn, RepoGrounder, Section, SessionState, Skill,
};

/// What to present after one `advance` (or after starting the next movement). A serializable
/// projection of `tutti_design::FacilitationState`: `Ratified { next: Some }` becomes
/// `Advanced` (a following movement is queued) and `Ratified { next: None }` becomes
/// `Complete` (the chain is done), so the frontend can branch on the two ratification
/// outcomes without reconstructing the enum.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DesignStep {
    /// The agent asked a question; collect a reply.
    Question { question: String },
    /// The agent proposed an artifact section; collect ratify or revise.
    Ratify { artifact_section: String },
    /// A movement was ratified and another one follows; begin it next.
    Advanced {
        movement: MovementId,
        next: MovementId,
    },
    /// The last movement was ratified; the chain is complete.
    Complete { movement: MovementId },
}

impl DesignStep {
    /// Map a facilitation state onto the wire DTO. Pure; unit-tested below.
    fn from_state(state: FacilitationState) -> Self {
        match state {
            FacilitationState::AwaitingHuman { question } => DesignStep::Question { question },
            FacilitationState::AwaitingRatification { artifact_section } => {
                DesignStep::Ratify { artifact_section }
            }
            FacilitationState::Ratified { movement, next } => match next {
                Some(next) => DesignStep::Advanced { movement, next },
                None => DesignStep::Complete { movement },
            },
        }
    }
}

/// One ratified movement's captured section, for the status/transcript view.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DesignArtifact {
    pub movement: MovementId,
    pub section: String,
}

/// A snapshot of the design session for the pane: the shape, the selected movements, how far
/// it has progressed, and the ratified artifacts so far.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DesignSessionStatus {
    pub shape: ProjectShape,
    pub movements: Vec<MovementId>,
    pub ratified: Vec<MovementId>,
    pub current: Option<MovementId>,
    pub complete: bool,
    pub artifacts: Vec<DesignArtifact>,
    /// The in-flight movement's pending state, so a pane reloaded mid-movement can rehydrate
    /// the step (show the pending question or the artifact awaiting ratification) instead of
    /// re-running the movement's opening turn and overwriting a proposed artifact.
    pub active: Option<DesignActive>,
}

/// The pending state of the movement currently being facilitated (from `session.active`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DesignActive {
    pub movement: MovementId,
    /// The artifact section awaiting ratification, if the last turn proposed one.
    pub pending_artifact: Option<String>,
    /// The last question the agent asked, if the movement is awaiting the human's answer
    /// (only set when there is no `pending_artifact`).
    pub pending_question: Option<String>,
}

/// A proposed backlog plus its human-readable review text, for the confirm-before-seed UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BacklogProposal {
    pub plan: BacklogPlan,
    pub rendered: String,
}

/// What a seed run did (a serializable projection of `tutti_design::SeedReport`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SeedReportDto {
    pub created: Vec<String>,
    pub skipped: Vec<String>,
}

/// The app-side facilitator: one facilitation turn is one `ClaudeSession::turn` in the
/// project's repo checkout. Unlike the CLI adapter, which drains the streamed events to a
/// sink, this forwards each agent line onto `design://delta` so the pane can show the turn
/// live.
struct AppFacilitator {
    app: tauri::AppHandle,
    session: ClaudeSession,
    model: String,
    cwd: PathBuf,
}

impl Facilitator for AppFacilitator {
    async fn turn(&self, prompt: &str, resume: Option<&str>) -> Result<RawTurn, DesignError> {
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(64);
        let app = self.app.clone();
        let fwd = tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                if let AgentEvent::Line(text) = ev {
                    let _ = app.emit("design://delta", text);
                }
            }
        });
        let outcome = self
            .session
            .turn(prompt, &self.model, resume, None, &self.cwd, None, tx)
            .await
            .map_err(|e| DesignError::Facilitation(format!("claude turn: {e}")))?;
        let _ = fwd.await;
        Ok(RawTurn {
            session_id: outcome.session_id.unwrap_or_default(),
            output: outcome.assistant_text,
        })
    }
}

/// Resolve the `skills/` directory that holds the movement facilitation skills. Ported from
/// the CLI's resolver (it is private to that binary crate): the `TUTTI_SKILLS_DIR` override
/// wins, else a `skills/` dir beside the running binary, else the dev/test fallback of
/// `<manifest>/../../skills`, which from `tutti-app/src-tauri` is the repo's own `skills/`.
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

/// Load a movement's facilitation skill from `skills/design/<name>/`.
fn movement_skill(id: MovementId) -> Result<Skill, DesignError> {
    let dir = skills_dir()
        .join("design")
        .join(tutti_design::skill_dir_name(id));
    tutti_design::load_skill(&dir)
}

/// A single-flight guard over the design-busy flag: refuses a second concurrent design turn
/// and clears the flag on every exit path (including an early `?`), mirroring the orchestrator
/// chat guard.
struct BusyGuard<'a>(&'a AtomicBool);
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn acquire_busy(flag: &AtomicBool) -> Result<BusyGuard<'_>, String> {
    if flag.swap(true, Ordering::SeqCst) {
        return Err("a design turn is already in progress".into());
    }
    Ok(BusyGuard(flag))
}

/// Pull the active project's repo root and model out from under the lock. Nothing borrowed
/// crosses the `advance` await; the guard is dropped here.
async fn active_context(state: &State<'_, AppState>) -> Result<(PathBuf, String), String> {
    let guard = state.project.lock().await;
    let p = guard.as_ref().ok_or("no project loaded")?;
    Ok((p.repo_root.clone(), p.config.model.clone()))
}

/// Load the design session at `dir`, erroring if none exists yet (start one first).
fn load_session(dir: &Path) -> Result<SessionState, String> {
    store::load(dir)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no design session; start one first".to_string())
}

/// Build a status snapshot from a session.
fn status_of(session: &SessionState) -> DesignSessionStatus {
    DesignSessionStatus {
        shape: session.shape,
        movements: session.movements.clone(),
        ratified: session.ratified.clone(),
        current: session.current(),
        complete: session.is_complete(),
        artifacts: session
            .artifacts
            .iter()
            .map(|a| DesignArtifact {
                movement: a.movement,
                section: a.section.clone(),
            })
            .collect(),
        active: session.active.as_ref().map(|p| {
            use tutti_design::session::Speaker;
            let pending_artifact = p.pending_artifact.clone();
            // The pending question is the last agent turn, but only when no artifact is
            // proposed yet (once an artifact is pending, the movement awaits ratification).
            let pending_question = if pending_artifact.is_some() {
                None
            } else {
                p.transcript
                    .iter()
                    .rev()
                    .find(|t| t.speaker == Speaker::Agent)
                    .map(|t| t.text.clone())
            };
            DesignActive {
                movement: p.movement,
                pending_artifact,
                pending_question,
            }
        }),
    }
}

/// Run one facilitation turn (`advance`) for the current movement with the given input, under
/// the busy guard. Shared by begin/reply/revise.
async fn run_turn(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    input: FacilitationInput,
) -> Result<DesignStep, String> {
    let _busy = acquire_busy(&state.design_busy)?;
    let (dir, model) = active_context(&state).await?;
    let mut session = load_session(&dir)?;
    let movement = session
        .current()
        .ok_or("the design chain is already complete")?;
    let skill = movement_skill(movement).map_err(|e| e.to_string())?;
    let fac = AppFacilitator {
        app,
        session: ClaudeSession::default(),
        model,
        cwd: dir.clone(),
    };
    let st = advance(&fac, &skill, movement, &mut session, &dir, input)
        .await
        .map_err(|e| e.to_string())?;
    Ok(DesignStep::from_state(st))
}

/// Start (or resume) the current movement's facilitation: the first `advance(Begin)`.
#[tauri::command]
pub async fn design_begin_movement(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<DesignStep, String> {
    run_turn(app, state, FacilitationInput::Begin).await
}

/// Answer the agent's last question and advance the current movement.
#[tauri::command]
pub async fn design_reply(
    text: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<DesignStep, String> {
    run_turn(app, state, FacilitationInput::Reply(text)).await
}

/// Reject the proposed artifact and ask the agent to revise it, with feedback.
#[tauri::command]
pub async fn design_revise(
    text: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<DesignStep, String> {
    run_turn(app, state, FacilitationInput::Revise(text)).await
}

/// Ratify the proposed artifact section, advancing the chain. This has no agent turn, so it
/// streams nothing; it just moves the session forward.
#[tauri::command]
pub async fn design_ratify(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<DesignStep, String> {
    run_turn(app, state, FacilitationInput::Accept).await
}

/// Report the current design session's status, or `None` when none has been started.
#[tauri::command]
pub async fn design_session_status(
    state: State<'_, AppState>,
) -> Result<Option<DesignSessionStatus>, String> {
    let (dir, _) = active_context(&state).await?;
    match store::load(&dir).map_err(|e| e.to_string())? {
        Some(session) => Ok(Some(status_of(&session))),
        None => Ok(None),
    }
}

/// Start a fresh design session for `shape`, grounding it in the active project's repo when
/// that repo already has detectable languages (so the code-derivable movements confirm rather
/// than ask from scratch), and persist it. The session lives at the active project's repo
/// root, so every other design command finds it there.
#[tauri::command]
pub async fn design_start(
    shape: ProjectShape,
    state: State<'_, AppState>,
) -> Result<DesignSessionStatus, String> {
    let (dir, _) = active_context(&state).await?;
    let mut session = SessionState::new(shape);
    // Best-effort grounding: a failure to read the repo is not fatal to starting a session.
    if !detect_languages(&dir).is_empty() {
        if let Ok(grounding) = RepoGroundingReader.ground(&dir) {
            session.grounding = Some(grounding);
        }
    }
    store::save(&dir, &session).map_err(|e| e.to_string())?;
    Ok(status_of(&session))
}

/// Build a best-effort design page from the session's ratified artifacts. Ported from the
/// CLI's `design_page_from_session`: each ratified section's markdown is escaped into a
/// `<pre>` block (the rich diagram-aware render is a separate concern), so nothing is lost.
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

/// The repo/design-page title: the directory's file name, falling back to "app".
fn repo_name_of(path: &Path) -> String {
    path.canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .or_else(|| path.file_name().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "app".to_string())
}

/// Render the accreting design page from the ratified artifacts, for the live preview pane.
#[tauri::command]
pub async fn design_preview(state: State<'_, AppState>) -> Result<String, String> {
    let (dir, _) = active_context(&state).await?;
    let session = load_session(&dir)?;
    let title = repo_name_of(&dir);
    let page = design_page_from_session(&session, &title);
    Ok(render_page(&page))
}

/// After the chain is complete, run one decompose turn to propose a backlog and render it for
/// review (nothing is written to the forge yet).
#[tauri::command]
pub async fn design_propose_backlog(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<BacklogProposal, String> {
    let _busy = acquire_busy(&state.design_busy)?;
    let (dir, model) = active_context(&state).await?;
    let session = load_session(&dir)?;
    if !session.is_complete() {
        return Err("finish the design chain before proposing a backlog".into());
    }
    let fac = AppFacilitator {
        app,
        session: ClaudeSession::default(),
        model,
        cwd: dir.clone(),
    };
    let raw = fac
        .turn(&backlog_prompt(&session), None)
        .await
        .map_err(|e| e.to_string())?;
    let plan = parse_backlog(&raw.output).map_err(|e| e.to_string())?;
    let rendered = render_plan(&plan);
    Ok(BacklogProposal { plan, rendered })
}

/// Seed a reviewed backlog onto the active project's own forge, idempotently. The target and
/// forge come from the loaded project context (the CLI takes them as `--target`/`--forge`
/// because it has no loaded project); the ready label is the project's own status:ready label.
#[tauri::command]
pub async fn design_seed_backlog(
    plan: BacklogPlan,
    state: State<'_, AppState>,
) -> Result<SeedReportDto, String> {
    let _busy = acquire_busy(&state.design_busy)?;
    // Re-validate at the command boundary: `parse_backlog` enforces this at propose time, but
    // this command takes a `BacklogPlan` round-tripped through the frontend, and `seed` creates
    // the ready label as a side effect even for an issueless plan. Refuse an empty backlog.
    if plan.issue_count() == 0 {
        return Err("empty backlog: nothing to seed".into());
    }
    // Pull owned data out under the lock; the seed is many sequential forge writes, so do not
    // hold the project lock across them (it would block the board, the orchestrator, and the
    // run driver for the duration).
    let (config, repo, repo_root) = {
        let guard = state.project.lock().await;
        let p = guard.as_ref().ok_or("no project loaded")?;
        (p.config.clone(), p.repo.clone(), p.repo_root.clone())
    };
    let forge =
        crate::commands::build_forge(&config, &repo, repo_root).map_err(|e| e.to_string())?;
    let ready_label = config.status_labels().ready;
    let report = seed(&plan, forge.as_ref(), &ready_label)
        .await
        .map_err(|e| e.to_string())?;
    Ok(SeedReportDto {
        created: report.created,
        skipped: report.skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tutti_design::session::MovementArtifact;

    #[test]
    fn awaiting_human_maps_to_a_question_step() {
        let st = DesignStep::from_state(FacilitationState::AwaitingHuman {
            question: "Who is this for?".into(),
        });
        assert_eq!(
            st,
            DesignStep::Question {
                question: "Who is this for?".into()
            }
        );
    }

    #[test]
    fn awaiting_ratification_maps_to_a_ratify_step() {
        let st = DesignStep::from_state(FacilitationState::AwaitingRatification {
            artifact_section: "## Constitution\nprivacy first".into(),
        });
        assert_eq!(
            st,
            DesignStep::Ratify {
                artifact_section: "## Constitution\nprivacy first".into()
            }
        );
    }

    #[test]
    fn ratified_with_a_next_movement_maps_to_advanced() {
        let st = DesignStep::from_state(FacilitationState::Ratified {
            movement: MovementId::Constitution,
            next: Some(MovementId::Frame),
        });
        assert_eq!(
            st,
            DesignStep::Advanced {
                movement: MovementId::Constitution,
                next: MovementId::Frame,
            }
        );
    }

    #[test]
    fn ratified_with_no_next_maps_to_complete() {
        let st = DesignStep::from_state(FacilitationState::Ratified {
            movement: MovementId::Decompose,
            next: None,
        });
        assert_eq!(
            st,
            DesignStep::Complete {
                movement: MovementId::Decompose,
            }
        );
    }

    #[test]
    fn design_step_serializes_with_a_snake_case_kind_tag() {
        let json = serde_json::to_string(&DesignStep::Question {
            question: "q?".into(),
        })
        .unwrap();
        assert!(json.contains("\"kind\":\"question\""));
        assert!(json.contains("\"question\":\"q?\""));
    }

    #[test]
    fn design_page_from_session_renders_one_section_per_artifact() {
        let mut session = SessionState::new(ProjectShape::SmallCli);
        session.artifacts.push(MovementArtifact {
            movement: MovementId::Constitution,
            section: "principles: privacy & <safety>".into(),
        });
        session.artifacts.push(MovementArtifact {
            movement: MovementId::Frame,
            section: "customer: solo devs".into(),
        });
        let page = design_page_from_session(&session, "demo");
        assert_eq!(page.title, "demo");
        assert_eq!(page.sections.len(), 2);
        // The first section carries the Constitution artifact, HTML-escaped inside a <pre>.
        assert!(page.sections[0].eyebrow.contains("CONSTITUTION"));
        assert!(page.sections[0].body_html.contains("&lt;safety&gt;"));
    }

    #[test]
    fn status_of_reports_progress_and_artifacts() {
        let mut session = SessionState::new(ProjectShape::SmallCli);
        let first = session.current().expect("a fresh session has a movement");
        session.artifacts.push(MovementArtifact {
            movement: first,
            section: "section body".into(),
        });
        session.ratify().expect("ratify the first movement");
        let status = status_of(&session);
        assert_eq!(status.ratified, vec![first]);
        assert_eq!(status.current, session.current());
        assert!(!status.complete);
        assert_eq!(status.artifacts.len(), 1);
        assert_eq!(status.artifacts[0].movement, first);
        assert!(
            status.active.is_none(),
            "no in-flight movement after a clean ratify"
        );
    }

    #[test]
    fn status_of_exposes_a_pending_artifact_for_rehydration() {
        use tutti_design::session::{MovementProgress, Speaker, Turn};
        let mut session = SessionState::new(ProjectShape::SmallCli);
        let current = session.current().unwrap();
        session.active = Some(MovementProgress {
            movement: current,
            agent_session_id: Some("sid".into()),
            transcript: vec![Turn {
                speaker: Speaker::Agent,
                text: "## Constitution\nprivacy first".into(),
            }],
            pending_artifact: Some("## Constitution\nprivacy first".into()),
        });
        let active = status_of(&session)
            .active
            .expect("active movement is exposed");
        assert_eq!(active.movement, current);
        assert_eq!(
            active.pending_artifact.as_deref(),
            Some("## Constitution\nprivacy first")
        );
        // With an artifact pending, no pending question is reported (awaits ratification).
        assert!(active.pending_question.is_none());
    }

    #[test]
    fn status_of_exposes_a_pending_question_when_no_artifact() {
        use tutti_design::session::{MovementProgress, Speaker, Turn};
        let mut session = SessionState::new(ProjectShape::SmallCli);
        let current = session.current().unwrap();
        session.active = Some(MovementProgress {
            movement: current,
            agent_session_id: Some("sid".into()),
            transcript: vec![
                Turn {
                    speaker: Speaker::Human,
                    text: "an answer".into(),
                },
                Turn {
                    speaker: Speaker::Agent,
                    text: "What must stay true?".into(),
                },
            ],
            pending_artifact: None,
        });
        let active = status_of(&session)
            .active
            .expect("active movement is exposed");
        assert_eq!(
            active.pending_question.as_deref(),
            Some("What must stay true?")
        );
        assert!(active.pending_artifact.is_none());
    }
}

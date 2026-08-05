// SPDX-License-Identifier: AGPL-3.0-or-later
//! The orchestrator chat session: drives `claude -p` as a resumable, streaming multi-turn
//! conversation in the repo checkout. Reuses `stream.rs` for parsing and mirrors the
//! spawn/stderr-drain plumbing of `ClaudeBackend::run`, but its outcome is streamed
//! assistant text plus a captured session id, not a ship artifact.

use crate::stream;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::mpsc::Sender;
use tutti_core::message::AgentEvent;
use tutti_core::traits::{EngineError, Result};

/// What one chat turn produced: the (possibly newly captured) session id to resume next
/// time, and the assistant's full reply text for persistence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnOutcome {
    pub session_id: Option<String>,
    pub assistant_text: String,
    /// A proposal the agent wrote this turn, if any. Live-only: the app emits it on
    /// `orchestrator://proposal` and does not persist it.
    pub proposal: Option<Proposal>,
}

/// Something the agent proposes and the user applies or dismisses. Tagged on `kind` so one
/// artifact path serves every proposal type; turns are single-flight and the path is cleared
/// at the start of each turn, so at most one proposal exists per turn.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Proposal {
    Gate(GateProposal),
    Triage(TriageProposal),
}

impl Proposal {
    /// True when applying this proposal would actually do something. An agent can write a
    /// well-formed but empty proposal (no commands, or two empty triage lists); surfacing
    /// that as a card asks the user to approve a no-op.
    pub fn is_actionable(&self) -> bool {
        match self {
            Proposal::Gate(g) => !g.commands.is_empty(),
            Proposal::Triage(t) => !t.ready.is_empty() || !t.needs_human.is_empty(),
        }
    }
}

/// A structured gate proposal the agent writes to an artifact file when it and the user
/// have agreed on what verifies the project. Read back after a turn; never hand-parsed from
/// prose.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GateProposal {
    pub commands: Vec<String>,
    /// Reserved. PR B applies `commands` only (they run from the repo root, which the
    /// proposal instruction states), so a proposed `working_dir` is not applied. Kept as a
    /// tolerant serde field so a proposal that still carries it deserializes. Wiring it into
    /// `apply_gate` is a follow-up.
    #[serde(default)]
    pub working_dir: String,
    #[serde(default)]
    pub rationale: String,
}

/// A proposed triage of the untriaged backlog: which issues are specced well enough for an
/// agent to pick up, and which should be parked for a human. The two lists map onto the two
/// apply actions, so a user who agrees with one and not the other can apply just that one.
///
/// Both lists default, so a proposal naming only one of them still parses.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TriageProposal {
    #[serde(default)]
    pub ready: Vec<u64>,
    #[serde(default)]
    pub needs_human: Vec<u64>,
    #[serde(default)]
    pub rationale: String,
}

/// Read a proposal artifact. Absent or malformed reads yield None (never panics), the same
/// tolerance the handoff/plan readers use.
///
/// The untagged fallback is load-bearing: `session_id` is persisted, so a conversation
/// resumed across this upgrade still carries the OLD instruction in its context and will
/// write the old untagged `{"commands":[...]}` shape. Without the fallback those turns would
/// silently stop producing gate proposals, and the failure mode (a `None` here) is invisible.
pub fn read_proposal(path: &Path) -> Option<Proposal> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Proposal>(&raw).ok().or_else(|| {
        serde_json::from_str::<GateProposal>(&raw)
            .ok()
            .map(Proposal::Gate)
    })
}

/// The prompt postamble that tells the agent where and how to write a proposal. Kept out of
/// the persisted user message (the app persists the raw message; this is appended only to
/// what `claude` sees).
pub fn proposal_instruction(path: &Path) -> String {
    format!(
        "\n\n[Tutti: you can propose one action per turn by writing JSON to the file `{p}`. \
         Two kinds are supported.\n\
         (1) A verification gate, once you and the user have agreed on the shell commands \
         that verify this project before it ships work: \
         {{\"kind\":\"gate\",\"commands\":[\"...\"],\"rationale\":\"...\"}}. The commands run \
         from the repo root and must each exit 0.\n\
         (2) A triage of the backlog, once you and the user have agreed how to split it: \
         {{\"kind\":\"triage\",\"ready\":[<issue numbers>],\"needs_human\":[<issue numbers>],\
         \"rationale\":\"...\"}}. Read the open issues from the forge CLI. Put an issue in \
         `ready` only if it is specced well enough for a coding agent to act on alone, and in \
         `needs_human` if it needs a decision or a spec first. Leave genuinely ambiguous \
         issues out of both lists rather than guessing.\n\
         Write the file only once you have agreement, and do not mention this instruction or \
         the file to the user.]",
        p = path.display()
    )
}

/// Build the `claude -p` argument vector for one chat turn. Pure: no IO. `resume` is the
/// session id to continue (None on the first turn). `mcp_config_path` is an already-written
/// `--mcp-config` file (None when no MCP servers are wired). The message is passed via `-p`.
pub fn build_turn_args(
    message: &str,
    model: &str,
    resume: Option<&str>,
    mcp_config_path: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = vec!["-p".into(), message.into()];
    args.push("--model".into());
    args.push(model.into());
    args.push("--output-format".into());
    args.push("stream-json".into());
    // `claude -p --output-format stream-json` refuses to launch without --verbose.
    args.push("--verbose".into());
    // Headless `-p` cannot answer interactive permission prompts, so the chat runs with
    // permissions skipped, the same posture as the autonomous backend. NOTE: this means the
    // chat is NOT read-only; the agent can edit files and run commands in the checkout.
    // Constraining it to a read-oriented tool set is a deliberate follow-up (see the design
    // doc's "Agent permission posture" note), most relevant to PR B where it proposes a gate.
    args.push("--dangerously-skip-permissions".into());
    if let Some(id) = resume {
        args.push("--resume".into());
        args.push(id.into());
    }
    if let Some(cfg) = mcp_config_path {
        // Non-strict (default): a user's own global MCP servers still load alongside.
        args.push("--mcp-config".into());
        args.push(cfg.into());
    }
    args
}

/// Distil a finished turn's full stdout into a `TurnOutcome`: the captured session id and
/// the assistant's reply text. Reuses the `stream.rs` parser so text-block concatenation
/// and tool_use handling stay in one place. Pure: no IO.
pub fn turn_outcome(full_output: &str) -> TurnOutcome {
    TurnOutcome {
        session_id: stream::scan_stream(full_output).session_id,
        assistant_text: collect_assistant_text(full_output),
        proposal: None,
    }
}

/// Concatenate the assistant reply text across a turn's transcript, ignoring the system
/// line, tool_use blocks, and the result line. `parse_stream_line` yields a Line for
/// assistant/text content but also for the system/unknown lines (the result line maps to
/// Done), so `is_assistant_text_line` gates on the JSON type first and only then parses.
fn collect_assistant_text(full_output: &str) -> String {
    let mut assistant_text = String::new();
    for line in full_output.lines() {
        if stream::is_assistant_text_line(line) {
            if let Some(AgentEvent::Line(text)) = stream::parse_stream_line(line) {
                assistant_text.push_str(&text);
            }
        }
    }
    assistant_text
}

/// Drives `claude -p` as an interactive chat session. `program` is the executable (usually
/// "claude"; a test points it at a fake script).
pub struct ClaudeSession {
    pub program: String,
}

impl Default for ClaudeSession {
    fn default() -> Self {
        Self {
            program: "claude".into(),
        }
    }
}

impl ClaudeSession {
    /// Run one chat turn in `cwd`, streaming parsed events on `events`, and return the
    /// captured session id plus the assistant's reply. `cwd` MUST be the same repo checkout
    /// every turn (claude keys its resumable session store by working directory).
    #[allow(clippy::too_many_arguments)]
    pub async fn turn(
        &self,
        message: &str,
        model: &str,
        resume: Option<&str>,
        mcp_config_path: Option<&str>,
        cwd: &Path,
        proposal_path: Option<&Path>,
        events: Sender<AgentEvent>,
    ) -> Result<TurnOutcome> {
        // Clear a stale proposal from a prior turn so this turn's outcome reflects only what
        // the agent writes now (the same discipline `ClaudeBackend::run` uses on its out_path).
        //
        // A failure here is fatal rather than ignored. If the file exists and we cannot
        // remove it, something other than this process owns it, and whatever we read back
        // afterwards would not be the agent's proposal. Since a gate proposal becomes shell
        // commands the engine later runs, reading a file we could not clear is exactly the
        // case worth refusing.
        if let Some(pp) = proposal_path {
            match std::fs::remove_file(pp) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(EngineError::Backend(format!(
                        "could not clear the proposal artifact at {}: {e}",
                        pp.display()
                    )))
                }
            }
        }
        // Append the proposal instruction to what claude sees (the app persists the raw
        // message; this augmentation is prompt-only).
        let prompt = match proposal_path {
            Some(pp) => format!("{message}{}", proposal_instruction(pp)),
            None => message.to_string(),
        };
        let args = build_turn_args(&prompt, model, resume, mcp_config_path);
        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.args(&args);
        cmd.current_dir(cwd);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        // Retries a transient ETXTBSY spawn (see spawn::spawn_with_etxtbsy_retry); shared with
        // ClaudeBackend::run so the two spawn sites cannot drift.
        let mut child = crate::spawn::spawn_with_etxtbsy_retry(&mut cmd)
            .await
            .map_err(|e| EngineError::Backend(format!("spawn claude: {e}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| EngineError::Backend("no stdout".into()))?;
        // Drain stderr concurrently so a full ~64KB pipe cannot deadlock the run.
        let stderr_pipe = child.stderr.take();
        let stderr_task = tokio::spawn(async move {
            let mut buf = String::new();
            if let Some(pipe) = stderr_pipe {
                let _ = BufReader::new(pipe).read_to_string(&mut buf).await;
            }
            buf
        });

        let mut reader = BufReader::new(stdout).lines();
        let mut full = String::new();
        while let Some(line) = reader
            .next_line()
            .await
            .map_err(|e| EngineError::Backend(format!("read: {e}")))?
        {
            full.push_str(&line);
            full.push('\n');
            // `parse_display_event` drops the raw-JSON system/rate_limit/unknown lines, so the
            // live deltas stay identical to the persisted reply text (which
            // `collect_assistant_text` gates on the same predicate); `ToolUse`/`Done` flow.
            if let Some(ev) = stream::parse_display_event(&line) {
                let _ = events.send(ev).await;
            }
        }
        let status = child
            .wait()
            .await
            .map_err(|e| EngineError::Backend(format!("wait: {e}")))?;
        let stderr = stderr_task.await.unwrap_or_default();
        let _ = events.send(AgentEvent::Done).await;

        let scan = stream::scan_stream(&full);
        if !status.success() {
            let snippet: String = stderr.trim().chars().take(500).collect();
            return Err(EngineError::Backend(format!(
                "claude exited non-zero: {snippet}"
            )));
        }
        if scan.rate_limited {
            return Err(EngineError::Backend("usage/rate limit".into()));
        }
        if let Some(r) = &scan.result {
            if r.is_error {
                let mut reason = String::from("claude reported an error");
                if let Some(status) = &r.api_error_status {
                    reason.push_str(&format!(" ({status})"));
                }
                if !r.result.is_empty() {
                    let snippet: String = r.result.trim().chars().take(500).collect();
                    reason.push_str(&format!(": {snippet}"));
                }
                return Err(EngineError::Backend(reason));
            }
        }
        // Build the outcome from the `scan` already computed above (no second pass over the
        // transcript); `turn_outcome` remains the pure entry point used by the unit tests.
        Ok(TurnOutcome {
            session_id: scan.session_id,
            assistant_text: collect_assistant_text(&full),
            // Drop a well-formed but empty proposal rather than asking the user to approve
            // a no-op (see `Proposal::is_actionable`).
            proposal: proposal_path
                .and_then(read_proposal)
                .filter(Proposal::is_actionable),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// The `program` a `ClaudeSession` runs. A test points it at a shell script that cats a
    /// fixture to stdout, exercising the spawn + stream + parse loop hermetically.
    #[cfg(unix)]
    #[tokio::test]
    async fn turn_streams_events_and_returns_outcome() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/session-turn.jsonl"
        );
        let script = dir.path().join("fake-claude.sh");
        std::fs::write(&script, format!("#!/bin/sh\ncat {fixture}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let session = ClaudeSession {
            program: script.to_string_lossy().into_owned(),
        };
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(64);
        let outcome = session
            .turn("hi", "sonnet", None, None, dir.path(), None, tx)
            .await
            .unwrap();

        assert_eq!(outcome.session_id.as_deref(), Some("fix-1"));
        assert_eq!(
            outcome.assistant_text,
            "Looking at your repo. It is a Rust workspace."
        );

        // The stream surfaced a tool_use event and at least one text line, ending in Done.
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolUse(n) if n == "Bash")));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Line(_))));
        assert!(matches!(events.last(), Some(AgentEvent::Done)));
        // No streamed Line is a raw protocol line (the `system` init line degrades to
        // `Line(<raw JSON>)`; it must be filtered so the live bubble matches the reply).
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, AgentEvent::Line(t) if t.contains("\"type\":\"system\""))),
            "raw system line leaked into the streamed deltas: {events:?}"
        );
    }

    /// `claude -p` can exit 0 while the result line is flagged `is_error`; `turn` must still
    /// fail rather than return a garbage/empty outcome.
    #[cfg(unix)]
    #[tokio::test]
    async fn turn_errors_on_result_flagged_is_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-claude.sh");
        // Exits 0 but the result line is flagged is_error: turn must still fail.
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"error\",\"is_error\":true,\"result\":\"boom happened\",\"session_id\":\"s\"}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let session = ClaudeSession {
            program: script.to_string_lossy().into_owned(),
        };
        let (tx, _rx) = mpsc::channel::<AgentEvent>(64);
        let err = session
            .turn("hi", "sonnet", None, None, dir.path(), None, tx)
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("boom happened"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn turn_reads_a_proposal_written_by_the_agent() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let gate = dir.path().join("gate.json");
        let script = dir.path().join("fake-claude.sh");
        // The fake agent writes a proposal to the gate path, then emits a clean turn.
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s' '{{\"commands\":[\"cargo test\"],\"working_dir\":\"\",\"rationale\":\"r\"}}' > {gate}\nprintf '%s\\n' '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"ok\",\"session_id\":\"s\"}}'\n",
                gate = gate.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let session = ClaudeSession {
            program: script.to_string_lossy().into_owned(),
        };
        let (tx, _rx) = mpsc::channel::<AgentEvent>(64);
        let outcome = session
            .turn("hi", "sonnet", None, None, dir.path(), Some(&gate), tx)
            .await
            .unwrap();
        let Some(Proposal::Gate(g)) = outcome.proposal else {
            panic!("expected a gate proposal");
        };
        assert_eq!(g.commands, vec!["cargo test".to_string()]);
        // The stale artifact is deleted before the next turn: a second turn with no write yields None.
        std::fs::remove_file(&script).ok();
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"ok\",\"session_id\":\"s\"}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (tx2, _rx2) = mpsc::channel::<AgentEvent>(64);
        let outcome2 = session
            .turn("hi", "sonnet", None, None, dir.path(), Some(&gate), tx2)
            .await
            .unwrap();
        assert!(
            outcome2.proposal.is_none(),
            "stale proposal must be cleared before the turn"
        );
    }

    #[test]
    fn first_turn_has_no_resume() {
        let args = build_turn_args("hello", "sonnet", None, None);
        assert!(args.windows(2).any(|w| w == ["-p", "hello"]));
        assert!(args.windows(2).any(|w| w == ["--model", "sonnet"]));
        assert!(args.contains(&"--verbose".to_string()));
        assert!(!args.iter().any(|a| a == "--resume"));
        assert!(!args.iter().any(|a| a == "--mcp-config"));
    }

    #[test]
    fn later_turn_resumes_and_wires_mcp() {
        let args = build_turn_args("next", "sonnet", Some("abc-123"), Some("/tmp/mcp.json"));
        assert!(args.windows(2).any(|w| w == ["--resume", "abc-123"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--mcp-config", "/tmp/mcp.json"]));
    }

    #[test]
    fn read_proposal_present_absent_and_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("gate.json");
        // Absent -> None.
        assert!(read_proposal(&p).is_none());
        // Malformed -> None (never panics).
        std::fs::write(&p, "not json").unwrap();
        assert!(read_proposal(&p).is_none());
        // Present and tagged -> parsed.
        std::fs::write(
            &p,
            r#"{"kind":"gate","commands":["cargo test"],"working_dir":"","rationale":"it is a rust workspace"}"#,
        )
        .unwrap();
        let Some(Proposal::Gate(got)) = read_proposal(&p) else {
            panic!("expected a gate proposal");
        };
        assert_eq!(got.commands, vec!["cargo test".to_string()]);
        assert_eq!(got.working_dir, "");
        assert_eq!(got.rationale, "it is a rust workspace");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn turn_refuses_when_the_stale_proposal_cannot_be_cleared() {
        // If we cannot clear the artifact, something else owns it, and what we read back
        // would not be the agent's proposal. A gate proposal becomes shell commands the
        // engine runs, so reading a file we could not clear is the case worth refusing.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        let artifact = locked.join("proposal.json");
        std::fs::write(&artifact, "{}").unwrap();
        // Read+execute only: the file exists but cannot be unlinked from this directory.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();

        let session = ClaudeSession {
            program: "/bin/echo".into(),
        };
        let (tx, _rx) = mpsc::channel::<AgentEvent>(64);
        let err = session
            .turn("hi", "sonnet", None, None, dir.path(), Some(&artifact), tx)
            .await
            .expect_err("a stale artifact we cannot clear must fail the turn");
        assert!(err.to_string().contains("could not clear"));

        // Restore so the tempdir can be cleaned up.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn read_proposal_accepts_the_untagged_legacy_gate_shape() {
        // A conversation resumed across the tagged-union upgrade still carries the old
        // instruction, so it writes the old shape. Dropping it would silently break gate
        // proposals for exactly the sessions that already had one in flight.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("gate.json");
        std::fs::write(&p, r#"{"commands":["cargo test"],"rationale":"legacy"}"#).unwrap();
        let Some(Proposal::Gate(got)) = read_proposal(&p) else {
            panic!("expected the legacy shape to parse as a gate proposal");
        };
        assert_eq!(got.commands, vec!["cargo test".to_string()]);
        assert_eq!(got.rationale, "legacy");
    }

    #[test]
    fn read_proposal_parses_a_triage() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("gate.json");
        std::fs::write(
            &p,
            r#"{"kind":"triage","ready":[4,9],"needs_human":[12],"rationale":"4 and 9 are specced"}"#,
        )
        .unwrap();
        let Some(Proposal::Triage(got)) = read_proposal(&p) else {
            panic!("expected a triage proposal");
        };
        assert_eq!(got.ready, vec![4, 9]);
        assert_eq!(got.needs_human, vec![12]);
        assert_eq!(got.rationale, "4 and 9 are specced");
    }

    #[test]
    fn read_proposal_parses_a_triage_naming_only_one_list() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("gate.json");
        std::fs::write(&p, r#"{"kind":"triage","ready":[4]}"#).unwrap();
        let Some(Proposal::Triage(got)) = read_proposal(&p) else {
            panic!("expected a triage proposal");
        };
        assert_eq!(got.ready, vec![4]);
        assert!(got.needs_human.is_empty());
    }

    #[test]
    fn an_empty_proposal_is_not_actionable() {
        // Read tolerantly, then filtered at the emit site so no no-op card is shown.
        assert!(!Proposal::Gate(GateProposal::default()).is_actionable());
        assert!(!Proposal::Triage(TriageProposal::default()).is_actionable());
        assert!(Proposal::Triage(TriageProposal {
            needs_human: vec![1],
            ..TriageProposal::default()
        })
        .is_actionable());
        assert!(Proposal::Gate(GateProposal {
            commands: vec!["true".into()],
            ..GateProposal::default()
        })
        .is_actionable());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn turn_drops_a_well_formed_but_empty_proposal() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let gate = dir.path().join("gate.json");
        let script = dir.path().join("fake-claude.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s' '{{\"kind\":\"triage\",\"ready\":[],\"needs_human\":[]}}' > {gate}\nprintf '%s\\n' '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"ok\",\"session_id\":\"s\"}}'\n",
                gate = gate.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let session = ClaudeSession {
            program: script.to_string_lossy().into_owned(),
        };
        let (tx, _rx) = mpsc::channel::<AgentEvent>(64);
        let outcome = session
            .turn("hi", "sonnet", None, None, dir.path(), Some(&gate), tx)
            .await
            .unwrap();
        assert!(outcome.proposal.is_none());
    }

    #[test]
    fn proposal_instruction_names_the_path_and_both_shapes() {
        let s = proposal_instruction(std::path::Path::new("/tmp/tutti-gate-9.json"));
        assert!(s.contains("/tmp/tutti-gate-9.json"));
        assert!(s.contains("\"kind\":\"gate\""));
        assert!(s.contains("\"kind\":\"triage\""));
        assert!(s.to_lowercase().contains("commands"));
        assert!(s.contains("needs_human"));
    }

    #[test]
    fn turn_outcome_collects_assistant_text_and_session_id() {
        let stream = concat!(
            r#"{"type":"system","subtype":"init","session_id":"s-1"}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello "}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"world"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
            "\n",
            r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"s-1"}"#,
        );
        let outcome = turn_outcome(stream);
        assert_eq!(outcome.session_id.as_deref(), Some("s-1"));
        // Assistant text blocks are concatenated; a tool_use line contributes no text.
        assert_eq!(outcome.assistant_text, "Hello world");
    }
}

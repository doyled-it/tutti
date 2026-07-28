// SPDX-License-Identifier: AGPL-3.0-or-later
//! Context providers: a source of MCP servers Tutti wires into agent runs, plus the
//! codegraph implementation. codegraph pre-indexes a repo and serves a `codegraph_explore`
//! MCP tool so agents read structure from an index instead of crawling files.

use crate::mcp::McpServer;
use async_trait::async_trait;
use std::path::PathBuf;

/// A source of MCP servers for an agent run. One impl today (`CodeGraph`); the trait
/// exists so the engine can hold an optional provider and tests can inject a fake without
/// requiring the codegraph binary.
#[async_trait]
pub trait ContextProvider: Send + Sync {
    /// Best-effort preparation before an agent runs (e.g. ensure the index exists).
    /// Never fails the run: swallow and log internally.
    async fn ensure_ready(&self);
    /// The MCP servers to wire into the invocation.
    fn mcp_servers(&self) -> Vec<McpServer>;
}

/// codegraph, bound to the project's main working dir. `serve --mcp -p <project>` serves
/// that one index regardless of the agent's (possibly transient worktree) cwd.
pub struct CodeGraph {
    project: PathBuf,
}

impl CodeGraph {
    /// `None` when the `codegraph` binary is not runnable (probed via `codegraph --version`).
    /// `project` is the main working dir to index and serve.
    pub fn detect(project: PathBuf) -> Option<CodeGraph> {
        let ok = std::process::Command::new("codegraph")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        ok.then_some(CodeGraph { project })
    }

    /// Construct without probing the binary. Test-only.
    #[cfg(test)]
    pub fn for_tests(project: &str) -> CodeGraph {
        CodeGraph {
            project: PathBuf::from(project),
        }
    }

    fn project_str(&self) -> String {
        self.project.to_string_lossy().into_owned()
    }
}

#[async_trait]
impl ContextProvider for CodeGraph {
    async fn ensure_ready(&self) {
        // Already indexed: codegraph's own watcher keeps it fresh, nothing to do.
        if self.project.join(".codegraph").exists() {
            return;
        }
        // `codegraph init -i <project>` = initialize + initial index. Best-effort: a
        // failure (missing binary, permissions) must not fail the agent run.
        let status = tokio::process::Command::new("codegraph")
            .arg("init")
            .arg("-i")
            .arg(&self.project)
            .status()
            .await;
        match status {
            Err(e) => eprintln!("codegraph init skipped: {e}"),
            Ok(s) if !s.success() => {
                eprintln!(
                    "codegraph init exited non-zero for {}",
                    self.project.display()
                );
            }
            Ok(_) => {}
        }
    }

    fn mcp_servers(&self) -> Vec<McpServer> {
        vec![McpServer {
            name: "codegraph".into(),
            command: "codegraph".into(),
            args: vec![
                "serve".into(),
                "--mcp".into(),
                "-p".into(),
                self.project_str(),
            ],
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_server_points_serve_at_the_project_dir() {
        let cg = CodeGraph::for_tests("/main/checkout");
        let s = &cg.mcp_servers()[0];
        assert_eq!(s.name, "codegraph");
        assert_eq!(s.command, "codegraph");
        assert_eq!(s.args, vec!["serve", "--mcp", "-p", "/main/checkout"]);
    }

    #[tokio::test]
    async fn ensure_ready_skips_when_index_exists() {
        // A dir that already has `.codegraph` must not shell out to `codegraph init`.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codegraph")).unwrap();
        let cg = CodeGraph::for_tests(dir.path().to_str().unwrap());
        // Must return without error even if the codegraph binary is missing.
        cg.ensure_ready().await;
    }
}

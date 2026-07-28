// SPDX-License-Identifier: AGPL-3.0-or-later
//! MCP server specs Tutti hands to a backend, and the Claude `--mcp-config` file writer.
//! Backend- and forge-neutral: a backend decides how to consume these.

use crate::traits::{EngineError, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// A stdio MCP server a backend can wire into an agent invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Serialize)]
struct McpEntry<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    command: &'a str,
    args: &'a [String],
}

/// Serialize `servers` to the Claude Code `--mcp-config` shape and write it to
/// `dir/mcp-config.json`, returning the path. The one place the file shape is defined, so
/// the engine backend path and the future chat path write it identically.
pub fn write_mcp_config(servers: &[McpServer], dir: &Path) -> Result<PathBuf> {
    let map: serde_json::Map<String, serde_json::Value> = servers
        .iter()
        .map(|s| {
            (
                s.name.clone(),
                serde_json::to_value(McpEntry {
                    kind: "stdio",
                    command: &s.command,
                    args: &s.args,
                })
                .expect("McpEntry serializes"),
            )
        })
        .collect();
    let doc = serde_json::json!({ "mcpServers": map });
    let path = dir.join("mcp-config.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&doc)
            .map_err(|e| EngineError::Backend(format!("serialize mcp config: {e}")))?,
    )
    .map_err(|e| EngineError::Backend(format!("write mcp config: {e}")))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codegraph_server_serializes_to_claude_mcp_shape() {
        let servers = vec![McpServer {
            name: "codegraph".into(),
            command: "codegraph".into(),
            args: vec!["serve".into(), "--mcp".into(), "-p".into(), "/repo".into()],
        }];
        let dir = tempfile::tempdir().unwrap();
        let path = write_mcp_config(&servers, dir.path()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["mcpServers"]["codegraph"]["type"], "stdio");
        assert_eq!(v["mcpServers"]["codegraph"]["command"], "codegraph");
        assert_eq!(v["mcpServers"]["codegraph"]["args"][2], "-p");
        assert_eq!(v["mcpServers"]["codegraph"]["args"][3], "/repo");
        assert!(path.ends_with("mcp-config.json"));
    }

    #[test]
    fn empty_servers_still_writes_valid_empty_object() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_mcp_config(&[], dir.path()).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v["mcpServers"].as_object().unwrap().is_empty());
    }
}

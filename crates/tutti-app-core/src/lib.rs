// SPDX-License-Identifier: AGPL-3.0-or-later
//! Hermetic logic behind the desktop app: the board model and its assembly from a Forge.
//! No Tauri dependency, so it runs in the fast workspace gate.

use serde::{Deserialize, Serialize};
use tutti_core::config::Config;
use tutti_core::domain::Issue;
use tutti_core::status::StatusLabels;
use tutti_core::tracking::{Milestone, MilestoneId, TrackState};
use tutti_core::traits::{EngineError, Forge, Result};

/// One issue as shown on the board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueCard {
    pub id: u64,
    pub title: String,
    pub status: Status,
    pub milestone: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ready,
    InProgress,
    Done,
    Other,
}

/// A milestone row for the roadmap rail and the Lanes view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MilestoneRow {
    pub id: u64,
    pub title: String,
    pub open: bool,
    pub total: u32,
    pub done: u32,
}

/// The whole board for the selected milestone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    pub milestones: Vec<MilestoneRow>,
    pub selected_milestone: Option<u64>,
    pub ready: Vec<IssueCard>,
    pub in_progress: Vec<IssueCard>,
    pub done: Vec<IssueCard>,
}

/// A label as shown on the drawer: name plus its real forge color, for a GitLab-style
/// scoped-label pill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelChip {
    pub name: String,
    /// Normalized hex color WITHOUT a leading '#', lowercased. Empty if unknown.
    pub color: String,
}

/// The full detail for the drawer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueDetail {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<LabelChip>,
    pub milestone: Option<String>,
    pub status: Status,
    pub branch: String,
}

/// Default color for a label whose name is not found in the repo's label list (e.g. a
/// stale/renamed label still on the issue). A neutral gray.
const DEFAULT_LABEL_COLOR: &str = "8b949e";

/// Normalize a forge-returned label color: strip a leading '#' and lowercase.
fn normalize_color(color: &str) -> String {
    color.trim_start_matches('#').to_lowercase()
}

fn classify(issue: &Issue, labels: &StatusLabels) -> Status {
    if issue.has_label(&labels.done) {
        Status::Done
    } else if issue.has_label(&labels.in_progress) {
        Status::InProgress
    } else if issue.has_label(&labels.ready) {
        Status::Ready
    } else {
        Status::Other
    }
}

fn card(issue: &Issue, labels: &StatusLabels) -> IssueCard {
    IssueCard {
        id: issue.id.0,
        title: issue.title.clone(),
        status: classify(issue, labels),
        milestone: issue.milestone.clone(),
    }
}

fn milestone_row(m: &Milestone) -> MilestoneRow {
    MilestoneRow {
        id: m.id.0,
        title: m.title.clone(),
        open: m.state == TrackState::Open,
        total: m.progress.total,
        done: m.progress.done,
    }
}

/// Parse `owner/repo` (or a group/subgroup/project path) from a git remote URL. Handles
/// scp-style (`git@host:owner/repo.git`), https (`https://host/owner/repo.git`), and
/// `ssh://` forms. Returns None if no path can be extracted.
pub fn repo_from_remote(url: &str) -> Option<String> {
    let u = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let after_host: &str = if let Some(idx) = u.find("://") {
        // scheme://[user@]host/path
        let rest = &u[idx + 3..];
        let slash = rest.find('/')?;
        &rest[slash + 1..]
    } else if let Some(at) = u.find('@') {
        // git@host:owner/repo
        let after_at = &u[at + 1..];
        let colon = after_at.find(':')?;
        &after_at[colon + 1..]
    } else {
        // host:owner/repo (no scheme, no user@)
        let colon = u.find(':')?;
        &u[colon + 1..]
    };
    let path = after_host.trim_matches('/');
    // A real slug is at least owner/repo, so require a '/'. This rejects single-segment
    // junk (a bare owner, or a Windows path like C:\Users\... that slipped past the
    // host:path branch) and falls back to manual entry instead of a wrong slug.
    if path.is_empty() || !path.contains('/') {
        None
    } else {
        Some(path.to_string())
    }
}

/// Guess the forge kind from a git remote URL's host. Returns "github" | "gitlab" |
/// "gitea", or None for an unknown host (the user picks in that case).
pub fn forge_kind_from_remote(url: &str) -> Option<String> {
    let u = url.trim();
    let host = if let Some(idx) = u.find("://") {
        let rest = &u[idx + 3..];
        let rest = rest.rsplit('@').next().unwrap_or(rest); // drop user@
        rest.split(['/', ':']).next()?
    } else if let Some(at) = u.find('@') {
        u[at + 1..].split(':').next()?
    } else {
        u.split(':').next()?
    };
    let host = host.to_lowercase();
    if host.contains("github.com") {
        Some("github".into())
    } else if host.contains("gitlab.com") {
        Some("gitlab".into())
    } else if host.contains("codeberg.org") {
        Some("gitea".into())
    } else {
        None
    }
}

/// Parameters for a generated tutti.toml.
#[derive(Debug, Clone)]
pub struct InitParams {
    pub trunk: String,
    pub routing: String,
    pub integration_branch: String,
    pub model: String,
    pub max_issues_per_run: u32,
    pub require_label: String,
    pub skip_labels: Vec<String>,
    pub gate_commands: Vec<String>,
    pub forge_kind: String,
    pub login: Option<String>,
}

impl Default for InitParams {
    fn default() -> Self {
        Self {
            trunk: "main".into(),
            routing: "trunk".into(),
            integration_branch: "staging".into(),
            model: "claude-sonnet-5".into(),
            max_issues_per_run: 25,
            require_label: "status:ready".into(),
            skip_labels: vec!["status:needs-human".into()],
            gate_commands: vec!["true".into()],
            forge_kind: "github".into(),
            login: None,
        }
    }
}

/// Render a valid tutti.toml. The output must parse and validate via Config::load.
/// Quote a value as a valid TOML basic string. Unlike `{:?}` (Rust Debug), which emits
/// `\u{XX}`-with-braces for non-ASCII and is rejected by TOML, this keeps printable UTF-8
/// verbatim and escapes only what TOML requires (quote, backslash, control chars).
fn toml_basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04X}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn render_tutti_toml(p: &InitParams) -> String {
    let list = |xs: &[String]| {
        xs.iter()
            .map(|s| toml_basic_string(s))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut out = String::new();
    out.push_str(&format!("trunk = {}\n", toml_basic_string(&p.trunk)));
    out.push_str(&format!("routing = {}\n", toml_basic_string(&p.routing)));
    out.push_str(&format!(
        "integration_branch = {}\n",
        toml_basic_string(&p.integration_branch)
    ));
    out.push_str(&format!("model = {}\n", toml_basic_string(&p.model)));
    out.push_str(&format!("max_issues_per_run = {}\n", p.max_issues_per_run));
    out.push_str("\n[select]\n");
    out.push_str(&format!(
        "require_label = {}\n",
        toml_basic_string(&p.require_label)
    ));
    out.push_str(&format!("skip_labels = [{}]\n", list(&p.skip_labels)));
    out.push_str("\n[gate]\n");
    out.push_str(&format!("commands = [{}]\n", list(&p.gate_commands)));
    out.push_str("working_dir = \"\"\n");
    out.push_str("\n[forge]\n");
    out.push_str(&format!("kind = {}\n", toml_basic_string(&p.forge_kind)));
    if let Some(login) = &p.login {
        if !login.is_empty() {
            out.push_str(&format!("login = {}\n", toml_basic_string(login)));
        }
    }
    out
}

/// A saved project: a local folder (its identity), the resolved repo slug, a display
/// name, and the forge kind (for the sidebar's colored dot).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub dir: String,
    pub repo: String,
    pub name: String,
    pub forge: String,
}

/// The persisted project list plus which one is active. Serialized to projects.json.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectStore {
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
    #[serde(default)]
    pub active: Option<String>,
}

impl ProjectStore {
    /// Parse from JSON; an unreadable/absent store is an empty one (first run).
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// Insert or replace an entry by `dir`, and make it active.
    pub fn upsert(&mut self, entry: ProjectEntry) {
        if let Some(existing) = self.projects.iter_mut().find(|p| p.dir == entry.dir) {
            *existing = entry.clone();
        } else {
            self.projects.push(entry.clone());
        }
        self.active = Some(entry.dir);
    }

    /// Remove by `dir`; clears `active` if it pointed at the removed entry.
    pub fn remove(&mut self, dir: &str) {
        self.projects.retain(|p| p.dir != dir);
        if self.active.as_deref() == Some(dir) {
            self.active = None;
        }
    }

    /// Set the active dir if it is a known project.
    pub fn set_active(&mut self, dir: &str) {
        if self.projects.iter().any(|p| p.dir == dir) {
            self.active = Some(dir.to_string());
        }
    }
}

/// Assemble the board for `select` (default: all issues). Reads the milestone list for the
/// rail, then buckets either the selected milestone's children or every issue by status label.
pub async fn assemble_board(
    forge: &dyn Forge,
    cfg: &Config,
    select: Option<MilestoneId>,
) -> Result<Board> {
    let labels = cfg.status_labels();
    let milestones = forge.list_milestones().await?;
    let rows: Vec<MilestoneRow> = milestones.iter().map(milestone_row).collect();

    let issues = match select {
        Some(mid) => forge.milestone_children(mid).await?,
        None => forge.list_issues().await?,
    };

    let (mut ready, mut in_progress, mut done) = (Vec::new(), Vec::new(), Vec::new());
    for issue in issues {
        let c = card(&issue, &labels);
        match c.status {
            Status::Ready => ready.push(c),
            Status::InProgress => in_progress.push(c),
            Status::Done => done.push(c),
            Status::Other => ready.push(c), // untriaged shows under Ready
        }
    }

    Ok(Board {
        milestones: rows,
        selected_milestone: select.map(|m| m.0),
        ready,
        in_progress,
        done,
    })
}

/// Find `id` among all issues and build its drawer detail.
pub async fn issue_detail(forge: &dyn Forge, cfg: &Config, id: u64) -> Result<IssueDetail> {
    let labels = cfg.status_labels();
    let issue = forge
        .list_issues()
        .await?
        .into_iter()
        .find(|i| i.id.0 == id)
        .ok_or_else(|| EngineError::Forge(format!("issue {id} not found")))?;
    // Label colors are cosmetic, so a labels-endpoint failure must never block the issue
    // detail: degrade to an empty map (chips fall back to the default gray).
    let color_by_name: std::collections::HashMap<String, String> = forge
        .list_labels()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(name, color)| (name, normalize_color(&color)))
        .collect();
    let chips: Vec<LabelChip> = issue
        .labels
        .iter()
        .map(|name| LabelChip {
            name: name.clone(),
            color: color_by_name
                .get(name)
                .cloned()
                .unwrap_or_else(|| DEFAULT_LABEL_COLOR.into()),
        })
        .collect();
    Ok(IssueDetail {
        id,
        title: issue.title.clone(),
        body: issue.body.clone(),
        labels: chips,
        milestone: issue.milestone.clone(),
        status: classify(&issue, &labels),
        branch: format!("feat/issue-{id}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tutti_core::config::Config;
    use tutti_core::domain::{CiState, SelectFilter};
    use tutti_core::gate::Gate;
    use tutti_core::message::NewIssue;
    use tutti_core::testing::fake_forge::FakeForge;

    /// Mirrors the `cfg()` test helper in `tutti-core`'s `engine.rs`: default status
    /// labels (`status.status = None`) fall back to the `status:*` convention used below.
    fn cfg() -> Config {
        Config {
            trunk: "main".into(),
            routing: "trunk".into(),
            integration_branch: "version/v0.1".into(),
            model: "fake".into(),
            max_issues_per_run: 5,
            ci_max_polls: 40,
            poll_delay_secs: 0,
            select: SelectFilter {
                require_label: "status:ready".into(),
                skip_labels: vec!["status:needs-human".into()],
                milestone: None,
            },
            gate: Gate {
                commands: vec!["true".into()],
                working_dir: Default::default(),
            },
            status: None,
            forge: Default::default(),
            roles: tutti_core::config::default_roles(),
            merge_mode: tutti_core::domain::MergeMode::Merge,
        }
    }

    /// `FakeForge::milestone_children` resolves through its internal `milestone_of` map,
    /// which is only populated by `create_issue(.., Some(milestone_id), ..)`; issues
    /// preloaded via `FakeForge::new` are never linked. So tests that need
    /// `milestone_children` to see issues must create them through the Forge trait, not
    /// preload them.
    async fn seed_issue(
        forge: &FakeForge,
        milestone: tutti_core::tracking::MilestoneId,
        label: &str,
    ) {
        forge
            .create_issue(
                &NewIssue {
                    title: label.into(),
                    body: String::new(),
                    labels: vec![label.into()],
                },
                Some(milestone),
                None,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn buckets_children_by_status_label() {
        let forge = FakeForge::new(vec![], CiState::Pass);
        let m = forge.create_milestone("Phase 1", None, "").await.unwrap();
        seed_issue(&forge, m.id, "status:ready").await;
        seed_issue(&forge, m.id, "status:in-progress").await;
        seed_issue(&forge, m.id, "status:done").await;

        let board = assemble_board(&forge, &cfg(), Some(m.id)).await.unwrap();
        assert_eq!(board.ready.len(), 1);
        assert_eq!(board.in_progress.len(), 1);
        assert_eq!(board.done.len(), 1);
        assert_eq!(board.selected_milestone, Some(m.id.0));
    }

    #[tokio::test]
    async fn buckets_all_issues_when_no_milestone_selected() {
        let forge = FakeForge::new(vec![], CiState::Pass);
        let m1 = forge.create_milestone("Phase 1", None, "").await.unwrap();
        let m2 = forge.create_milestone("Phase 2", None, "").await.unwrap();
        seed_issue(&forge, m1.id, "status:ready").await;
        seed_issue(&forge, m1.id, "status:in-progress").await;
        seed_issue(&forge, m2.id, "status:done").await;

        let board = assemble_board(&forge, &cfg(), None).await.unwrap();
        assert_eq!(board.ready.len(), 1);
        assert_eq!(board.in_progress.len(), 1);
        assert_eq!(board.done.len(), 1);
        assert_eq!(board.selected_milestone, None);
        assert_eq!(board.milestones.len(), 2);
    }

    #[tokio::test]
    async fn issue_detail_finds_issue_across_milestones() {
        let forge = FakeForge::new(vec![], CiState::Pass);
        let m = forge.create_milestone("Phase 1", None, "").await.unwrap();
        let created = forge
            .create_issue(
                &NewIssue {
                    title: "do the thing".into(),
                    body: "some body".into(),
                    labels: vec!["status:ready".into()],
                },
                Some(m.id),
                None,
            )
            .await
            .unwrap();

        let detail = issue_detail(&forge, &cfg(), created.id.0).await.unwrap();
        assert_eq!(detail.id, created.id.0);
        assert_eq!(detail.title, "do the thing");
        assert_eq!(detail.body, "some body");
        assert_eq!(detail.status, Status::Ready);
        assert_eq!(detail.milestone.as_deref(), Some("Phase 1"));
        assert_eq!(detail.branch, format!("feat/issue-{}", created.id.0));
        // FakeForge::list_labels is empty, so every chip falls back to the default color.
        assert_eq!(detail.labels.len(), 1);
        assert_eq!(detail.labels[0].name, "status:ready");
        assert_eq!(detail.labels[0].color, "8b949e");
    }

    #[tokio::test]
    async fn issue_detail_errors_when_not_found() {
        let forge = FakeForge::new(vec![], CiState::Pass);
        let err = issue_detail(&forge, &cfg(), 999).await.unwrap_err();
        assert!(matches!(err, EngineError::Forge(msg) if msg.contains("999")));
    }

    #[test]
    fn repo_from_remote_parses_scp_style() {
        assert_eq!(
            repo_from_remote("git@github.com:doyled-it/tutti-live-sandbox.git"),
            Some("doyled-it/tutti-live-sandbox".to_string())
        );
    }

    #[test]
    fn repo_from_remote_parses_https() {
        assert_eq!(
            repo_from_remote("https://github.com/doyled-it/tutti-live-sandbox.git"),
            Some("doyled-it/tutti-live-sandbox".to_string())
        );
    }

    #[test]
    fn repo_from_remote_parses_https_without_dot_git() {
        assert_eq!(
            repo_from_remote("https://github.com/doyled-it/tutti-live-sandbox"),
            Some("doyled-it/tutti-live-sandbox".to_string())
        );
    }

    #[test]
    fn repo_from_remote_parses_gitlab_nested_path() {
        assert_eq!(
            repo_from_remote("git@gitlab.com:group/sub/project.git"),
            Some("group/sub/project".to_string())
        );
    }

    #[test]
    fn repo_from_remote_returns_none_for_garbage() {
        assert_eq!(repo_from_remote("not a url"), None);
    }

    #[test]
    fn repo_from_remote_rejects_single_segment_paths() {
        // A bare owner (no repo) and a Windows path have no owner/repo slash: reject them
        // so the caller falls back to manual entry rather than a wrong slug.
        assert_eq!(repo_from_remote("https://github.com/owner"), None);
        assert_eq!(repo_from_remote(r"C:\Users\me\project"), None);
    }

    fn entry(dir: &str) -> ProjectEntry {
        ProjectEntry {
            dir: dir.into(),
            repo: format!("owner/{dir}"),
            name: dir.into(),
            forge: "github".into(),
        }
    }

    #[test]
    fn upsert_adds_and_replaces() {
        let mut store = ProjectStore::default();
        store.upsert(entry("proj-a"));
        assert_eq!(store.projects.len(), 1);
        assert_eq!(store.active.as_deref(), Some("proj-a"));

        store.upsert(entry("proj-b"));
        assert_eq!(store.projects.len(), 2);
        assert_eq!(store.active.as_deref(), Some("proj-b"));

        // Re-upserting the same dir replaces the entry in place and keeps len at 2.
        let mut updated = entry("proj-a");
        updated.name = "renamed".into();
        store.upsert(updated);
        assert_eq!(store.projects.len(), 2);
        assert_eq!(store.active.as_deref(), Some("proj-a"));
        let found = store.projects.iter().find(|p| p.dir == "proj-a").unwrap();
        assert_eq!(found.name, "renamed");
    }

    #[test]
    fn remove_drops_and_clears_active() {
        let mut store = ProjectStore::default();
        store.upsert(entry("proj-a"));
        store.upsert(entry("proj-b"));
        assert_eq!(store.active.as_deref(), Some("proj-b"));

        // Removing a non-active entry leaves `active` alone.
        store.remove("proj-a");
        assert_eq!(store.projects.len(), 1);
        assert_eq!(store.active.as_deref(), Some("proj-b"));

        // Removing the active entry clears `active`.
        store.remove("proj-b");
        assert!(store.projects.is_empty());
        assert_eq!(store.active, None);
    }

    #[test]
    fn json_round_trip_and_garbage_defaults() {
        let mut store = ProjectStore::default();
        store.upsert(entry("proj-a"));

        let json = store.to_json();
        let parsed = ProjectStore::from_json(&json);
        assert_eq!(parsed, store);

        assert_eq!(ProjectStore::from_json("garbage"), ProjectStore::default());
    }

    #[test]
    fn forge_kind_from_remote_detects_github() {
        assert_eq!(
            forge_kind_from_remote("git@github.com:doyled-it/tutti.git"),
            Some("github".to_string())
        );
        assert_eq!(
            forge_kind_from_remote("https://github.com/doyled-it/tutti.git"),
            Some("github".to_string())
        );
    }

    #[test]
    fn forge_kind_from_remote_detects_gitlab() {
        assert_eq!(
            forge_kind_from_remote("git@gitlab.com:group/project.git"),
            Some("gitlab".to_string())
        );
        assert_eq!(
            forge_kind_from_remote("https://gitlab.com/group/project.git"),
            Some("gitlab".to_string())
        );
    }

    #[test]
    fn forge_kind_from_remote_detects_codeberg_as_gitea() {
        assert_eq!(
            forge_kind_from_remote("git@codeberg.org:owner/repo.git"),
            Some("gitea".to_string())
        );
        assert_eq!(
            forge_kind_from_remote("https://codeberg.org/owner/repo.git"),
            Some("gitea".to_string())
        );
    }

    #[test]
    fn forge_kind_from_remote_returns_none_for_unknown_host() {
        assert_eq!(
            forge_kind_from_remote("git@example.com:owner/repo.git"),
            None
        );
    }

    #[test]
    fn render_tutti_toml_round_trips_defaults_through_config_load() {
        let params = InitParams::default();
        let toml_text = render_tutti_toml(&params);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tutti.toml");
        std::fs::write(&path, &toml_text).unwrap();

        let cfg = tutti_core::config::Config::load(&path).unwrap();
        assert_eq!(cfg.trunk, params.trunk);
        assert_eq!(cfg.integration_branch, params.integration_branch);
        assert_eq!(cfg.forge.kind, tutti_core::config::ForgeKind::GitHub);
    }

    #[test]
    fn render_tutti_toml_round_trips_gitea_login_through_config_load() {
        let params = InitParams {
            forge_kind: "gitea".into(),
            login: Some("icesight-engine".into()),
            ..InitParams::default()
        };
        let toml_text = render_tutti_toml(&params);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tutti.toml");
        std::fs::write(&path, &toml_text).unwrap();

        let cfg = tutti_core::config::Config::load(&path).unwrap();
        assert_eq!(cfg.trunk, params.trunk);
        assert_eq!(cfg.integration_branch, params.integration_branch);
        assert_eq!(cfg.forge.kind, tutti_core::config::ForgeKind::Gitea);
        assert_eq!(cfg.forge.login.as_deref(), Some("icesight-engine"));
    }

    #[test]
    fn render_round_trips_non_default_values_for_every_field() {
        let params = InitParams {
            trunk: "trunk-branch".into(),
            routing: "phase_stacking".into(),
            integration_branch: "integ".into(),
            model: "claude-opus-4-8".into(),
            max_issues_per_run: 7,
            require_label: "ready-now".into(),
            skip_labels: vec!["blocked".into(), "needs-human".into()],
            gate_commands: vec!["cargo test".into(), "cargo clippy".into()],
            forge_kind: "gitlab".into(),
            login: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tutti.toml");
        std::fs::write(&path, render_tutti_toml(&params)).unwrap();
        let cfg = tutti_core::config::Config::load(&path).unwrap();
        assert_eq!(cfg.trunk, "trunk-branch");
        assert_eq!(cfg.routing, "phase_stacking");
        assert_eq!(cfg.integration_branch, "integ");
        assert_eq!(cfg.model, "claude-opus-4-8");
        assert_eq!(cfg.max_issues_per_run, 7);
        assert_eq!(cfg.select.require_label, "ready-now");
        assert_eq!(cfg.select.skip_labels, vec!["blocked", "needs-human"]);
        assert_eq!(cfg.gate.commands, vec!["cargo test", "cargo clippy"]);
    }

    #[test]
    fn render_tutti_toml_escapes_special_characters() {
        // A value with a quote, a backslash, and a non-ASCII char must still produce a
        // tutti.toml that Config::load accepts, with the value preserved verbatim.
        let params = InitParams {
            gate_commands: vec![r#"echo "café" \ done"#.into()],
            model: "modèle-5".into(),
            ..InitParams::default()
        };
        let toml_text = render_tutti_toml(&params);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tutti.toml");
        std::fs::write(&path, &toml_text).unwrap();

        let cfg = tutti_core::config::Config::load(&path).unwrap();
        assert_eq!(cfg.model, "modèle-5");
        assert_eq!(cfg.gate.commands, vec![r#"echo "café" \ done"#.to_string()]);
    }
}

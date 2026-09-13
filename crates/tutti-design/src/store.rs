// SPDX-License-Identifier: AGPL-3.0-or-later
//! Where a design session lives on disk: `<repo>/.tutti/design/session.json`, written
//! atomically (temp + rename) so a crash mid-write never truncates a resumable session.

use crate::error::Result;
use crate::session::SessionState;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The session file for a repo root.
pub fn session_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".tutti").join("design").join("session.json")
}

/// Persist a session, creating `.tutti/design/` as needed. Atomic: writes a sibling temp
/// file and renames it over the target, so a reader never sees a half-written file.
pub fn save(repo_root: &Path, state: &SessionState) -> Result<()> {
    let path = session_path(repo_root);
    let dir = path
        .parent()
        .expect("session_path always has a parent directory");
    fs::create_dir_all(dir)?;

    let json = serde_json::to_string_pretty(state)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(json.as_bytes())?;
    tmp.flush()?;
    // persist() maps a rename failure into a PersistError carrying the io::Error.
    tmp.persist(&path).map_err(|e| e.error)?;
    Ok(())
}

/// Load a session if one exists. `Ok(None)` when there is no session file yet (a fresh
/// repo), distinct from an error reading a corrupt or unreadable one.
pub fn load(repo_root: &Path) -> Result<Option<SessionState>> {
    let path = session_path(repo_root);
    match fs::read_to_string(&path) {
        Ok(s) => {
            let state: SessionState = serde_json::from_str(&s)?;
            state.validate()?;
            Ok(Some(state))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// The directory holding named session snapshots (branches) for a repo.
pub fn branches_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".tutti").join("design").join("branches")
}

/// Validate a branch name as a single safe path segment: non-empty, unchanged by trimming,
/// and only ASCII alphanumerics plus '-'/'_'. This rejects `..`, `/`, `\`, `.`, and
/// whitespace, so a name can never escape `branches_dir`.
fn valid_branch_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 100
        && name.trim() == name
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(crate::error::DesignError::Store(format!(
            "invalid branch name: {name:?}"
        )))
    }
}

/// The file for a named branch (name validated first).
pub fn branch_path(repo_root: &Path, name: &str) -> Result<PathBuf> {
    valid_branch_name(name)?;
    Ok(branches_dir(repo_root).join(format!("{name}.json")))
}

/// Persist `state` as the named branch, atomically (mirrors `save`).
pub fn save_branch(repo_root: &Path, name: &str, state: &SessionState) -> Result<()> {
    let path = branch_path(repo_root, name)?;
    let dir = path.parent().expect("branch_path always has a parent");
    fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(state)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(json.as_bytes())?;
    tmp.flush()?;
    tmp.persist(&path).map_err(|e| e.error)?;
    Ok(())
}

/// Load a named branch if it exists (validates the name and the session, like `load`).
pub fn load_branch(repo_root: &Path, name: &str) -> Result<Option<SessionState>> {
    let path = branch_path(repo_root, name)?;
    match fs::read_to_string(&path) {
        Ok(s) => {
            let state: SessionState = serde_json::from_str(&s)?;
            state.validate()?;
            Ok(Some(state))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// The names of all saved branches, sorted. Empty when the branches directory does not exist.
pub fn list_branches(repo_root: &Path) -> Result<Vec<String>> {
    let dir = branches_dir(repo_root);
    let mut names = Vec::new();
    match fs::read_dir(&dir) {
        Ok(rd) => {
            for entry in rd.flatten() {
                // Only real files, so a directory whose name ends in `.json` is not listed as a
                // phantom branch that would then error on load.
                let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
                let p = entry.path();
                if is_file && p.extension().and_then(|x| x.to_str()) == Some("json") {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        names.push(stem.to_string());
                    }
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    names.sort();
    Ok(names)
}

/// Snapshot the current active session to a named branch. Errors if there is no active session.
pub fn snapshot_active_to_branch(repo_root: &Path, name: &str) -> Result<()> {
    let active = load(repo_root)?
        .ok_or_else(|| crate::error::DesignError::Store("no active session to snapshot".into()))?;
    save_branch(repo_root, name, &active)
}

/// Make a named branch the active session (copy it to `session.json`). Errors if the branch
/// does not exist.
pub fn activate_branch(repo_root: &Path, name: &str) -> Result<()> {
    let state = load_branch(repo_root, name)?
        .ok_or_else(|| crate::error::DesignError::Store(format!("no such branch: {name}")))?;
    save(repo_root, &state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::DesignError;
    use crate::movement::MovementId;
    use crate::shape::ProjectShape;

    #[test]
    fn load_on_a_fresh_repo_is_none() {
        let repo = tempfile::tempdir().unwrap();
        assert!(load(repo.path()).unwrap().is_none());
    }

    #[test]
    fn save_then_load_round_trips_and_resumes() {
        let repo = tempfile::tempdir().unwrap();
        let mut s = SessionState::new(ProjectShape::MultiService);
        s.ratify().unwrap(); // Constitution
        s.ratify().unwrap(); // Frame
        save(repo.path(), &s).unwrap();

        let resumed = load(repo.path()).unwrap().expect("a saved session");
        assert_eq!(resumed, s);
        assert_eq!(resumed.current(), Some(MovementId::Impact));
    }

    #[test]
    fn save_creates_the_nested_tutti_design_dir() {
        let repo = tempfile::tempdir().unwrap();
        let s = SessionState::new(ProjectShape::SmallCli);
        save(repo.path(), &s).unwrap();
        assert!(session_path(repo.path()).is_file());
    }

    #[test]
    fn save_overwrites_a_previous_session() {
        let repo = tempfile::tempdir().unwrap();
        let mut s = SessionState::new(ProjectShape::SmallCli);
        save(repo.path(), &s).unwrap();
        s.ratify().unwrap();
        save(repo.path(), &s).unwrap();
        let resumed = load(repo.path()).unwrap().unwrap();
        assert_eq!(resumed.ratified.len(), 1);
    }

    #[test]
    fn round_trips_a_full_multi_service_walk_to_completion() {
        // The end-to-end integration test in lib.rs walks a 6-movement SmallCli session;
        // this covers the full 8-movement MultiService chain persisted and resumed to the end.
        let repo = tempfile::tempdir().unwrap();
        let mut s = SessionState::new(ProjectShape::MultiService);
        while s.current().is_some() {
            s.ratify().unwrap();
            save(repo.path(), &s).unwrap();
            assert_eq!(load(repo.path()).unwrap().unwrap(), s);
        }
        assert_eq!(s.ratified.len(), 8);
        assert!(load(repo.path()).unwrap().unwrap().is_complete());
    }

    #[test]
    fn load_on_garbage_bytes_errors_serde() {
        let repo = tempfile::tempdir().unwrap();
        let path = session_path(repo.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not json at all").unwrap();

        assert!(matches!(load(repo.path()), Err(DesignError::Serde(_))));
    }

    #[test]
    fn load_on_a_semantically_corrupt_session_errors_corrupt() {
        let repo = tempfile::tempdir().unwrap();
        let path = session_path(repo.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Valid JSON, but ratified is longer than movements: not a valid session.
        let corrupt = serde_json::json!({
            "shape": "small_cli",
            "movements": ["constitution"],
            "ratified": ["constitution", "frame"],
        });
        fs::write(&path, serde_json::to_vec(&corrupt).unwrap()).unwrap();

        assert!(matches!(load(repo.path()), Err(DesignError::Corrupt(_))));
    }

    #[test]
    fn branch_name_validation_rejects_traversal_and_separators() {
        let d = tempfile::tempdir().unwrap();
        let s = SessionState::new(ProjectShape::SmallCli);
        for bad in [
            "", "..", "../evil", "a/b", "a\\b", ".", "  ", " lead", "trail ",
        ] {
            assert!(save_branch(d.path(), bad, &s).is_err(), "rejects {bad:?}");
        }
        for good in ["alt", "explore-auth", "v2_idea", "a1"] {
            assert!(save_branch(d.path(), good, &s).is_ok(), "accepts {good:?}");
        }
        // A name over the length cap is rejected (fails validation, not the filesystem).
        assert!(
            save_branch(d.path(), &"a".repeat(101), &s).is_err(),
            "rejects an over-long name"
        );
    }

    #[test]
    fn list_branches_ignores_a_directory_named_like_a_branch() {
        let d = tempfile::tempdir().unwrap();
        save_branch(d.path(), "real", &SessionState::new(ProjectShape::SmallCli)).unwrap();
        // A subdirectory whose name ends in .json must not be listed as a phantom branch.
        fs::create_dir_all(branches_dir(d.path()).join("bogus.json")).unwrap();
        assert_eq!(list_branches(d.path()).unwrap(), vec!["real".to_string()]);
    }

    #[test]
    fn save_load_list_branches_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let mut s = SessionState::new(ProjectShape::Mobile);
        s.artifacts.push(crate::session::MovementArtifact {
            movement: MovementId::Constitution,
            section: "principles".into(),
        });
        save_branch(d.path(), "alt", &s).unwrap();
        save_branch(
            d.path(),
            "other",
            &SessionState::new(ProjectShape::SmallCli),
        )
        .unwrap();
        assert_eq!(
            list_branches(d.path()).unwrap(),
            vec!["alt".to_string(), "other".to_string()]
        );
        let back = load_branch(d.path(), "alt").unwrap().expect("branch loads");
        assert_eq!(back, s);
        assert!(load_branch(d.path(), "missing").unwrap().is_none());
    }

    #[test]
    fn list_branches_on_a_fresh_repo_is_empty() {
        let d = tempfile::tempdir().unwrap();
        assert!(list_branches(d.path()).unwrap().is_empty());
    }

    #[test]
    fn snapshot_active_then_activate_roundtrips_through_the_active_session() {
        let d = tempfile::tempdir().unwrap();
        let mut active = SessionState::new(ProjectShape::MultiService);
        active.artifacts.push(crate::session::MovementArtifact {
            movement: MovementId::Frame,
            section: "customer".into(),
        });
        save(d.path(), &active).unwrap();
        snapshot_active_to_branch(d.path(), "keep").unwrap();
        // Overwrite the active session, then activate the snapshot to restore it.
        save(d.path(), &SessionState::new(ProjectShape::SmallCli)).unwrap();
        activate_branch(d.path(), "keep").unwrap();
        assert_eq!(load(d.path()).unwrap().unwrap(), active);
    }

    #[test]
    fn snapshot_without_active_errors_and_activate_missing_errors() {
        let d = tempfile::tempdir().unwrap();
        assert!(matches!(
            snapshot_active_to_branch(d.path(), "x"),
            Err(DesignError::Store(_))
        ));
        assert!(matches!(
            activate_branch(d.path(), "nope"),
            Err(DesignError::Store(_))
        ));
    }
}

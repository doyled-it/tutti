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
    fn save_overwrites_a_previous_session_atomically() {
        let repo = tempfile::tempdir().unwrap();
        let mut s = SessionState::new(ProjectShape::SmallCli);
        save(repo.path(), &s).unwrap();
        s.ratify().unwrap();
        save(repo.path(), &s).unwrap();
        let resumed = load(repo.path()).unwrap().unwrap();
        assert_eq!(resumed.ratified.len(), 1);
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
}

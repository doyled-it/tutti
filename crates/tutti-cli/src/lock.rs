// SPDX-License-Identifier: AGPL-3.0-or-later
//! A PID-aware lock so two `tutti` runs cannot drive the same repo at once. Ports the
//! SOTTO engine's mkdir-lock with self-heal: a stale lock whose pid is dead is reclaimed.

use std::path::{Path, PathBuf};

/// Held for the duration of a run; removes the lock dir on drop.
pub struct PidLock {
    dir: PathBuf,
}

impl PidLock {
    /// Acquire the lock at `dir`. Err if a live process already holds it.
    pub fn acquire(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        match std::fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if Self::holder_alive(&dir) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "another tutti run holds the lock",
                    ));
                }
                // Stale lock from a crash: reclaim it.
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir(&dir)?;
            }
            Err(e) => return Err(e),
        }
        std::fs::write(dir.join("pid"), std::process::id().to_string())?;
        Ok(Self { dir })
    }

    fn holder_alive(dir: &Path) -> bool {
        let Ok(pid_str) = std::fs::read_to_string(dir.join("pid")) else {
            return false;
        };
        let Ok(pid) = pid_str.trim().parse::<i32>() else {
            return false;
        };
        // Signal 0 probes existence without killing.
        if unsafe { libc_kill(pid, 0) } == 0 {
            return true;
        }
        // A live process owned by another user returns -1 with errno EPERM: it exists,
        // so treat it as alive rather than reclaiming its lock. EPERM is 1 on Linux and
        // macOS; we compare the numeric value to avoid a libc crate dependency.
        const EPERM: i32 = 1;
        std::io::Error::last_os_error().raw_os_error() == Some(EPERM)
    }
}

impl Drop for PidLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// Minimal libc kill shim to avoid a libc dependency for one call.
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_while_held() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("lock.d");
        let _l = PidLock::acquire(&lock_path).unwrap();
        assert!(PidLock::acquire(&lock_path).is_err());
    }

    #[test]
    fn lock_released_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("lock.d");
        {
            let _l = PidLock::acquire(&lock_path).unwrap();
        }
        // After drop, re-acquire succeeds.
        assert!(PidLock::acquire(&lock_path).is_ok());
    }

    #[test]
    fn stale_lock_with_dead_pid_is_reclaimed() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("lock.d");
        std::fs::create_dir(&lock_path).unwrap();
        std::fs::write(lock_path.join("pid"), "999999").unwrap(); // almost certainly dead
        assert!(PidLock::acquire(&lock_path).is_ok());
    }
}

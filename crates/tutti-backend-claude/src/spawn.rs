// SPDX-License-Identifier: AGPL-3.0-or-later
//! Spawning helper shared by the two claude process entry points (`session::turn` and
//! `ClaudeBackend::run`), so their transient-failure handling cannot silently diverge.

use tokio::process::{Child, Command};

/// Spawn `cmd`, retrying briefly on ETXTBSY. Spawning can transiently fail with "text file
/// busy" (errno 26 on Linux and macOS) when the target program was very recently written and a
/// sibling process still holds a write handle to it across a fork. The real claude binary is
/// never freshly written, so in practice this only bites the hermetic tests (which write a
/// fake-claude script then exec it) under parallel load, but ETXTBSY is inherently transient
/// and safe to retry. Any other error, or exhausting the bounded attempts, surfaces immediately.
pub(crate) async fn spawn_with_etxtbsy_retry(cmd: &mut Command) -> std::io::Result<Child> {
    let mut attempt = 0u32;
    loop {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            // 26 == ETXTBSY on Linux and macOS.
            Err(e) if e.raw_os_error() == Some(26) && attempt < 10 => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

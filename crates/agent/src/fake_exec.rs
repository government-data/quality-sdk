//! Test support: fake executables written and run without "Text file
//! busy" (`ETXTBSY`).
//!
//! Tests run in parallel threads of one process. When a thread forks to run
//! a command while another holds a fake executable open for writing, the
//! child inherits that descriptor until it calls `exec`; running the
//! executable in that window fails with `ETXTBSY`. Writing takes the lock
//! alone and every run in a test shares it, so no fork happens while an
//! executable is open for writing.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{RwLock, RwLockReadGuard};

static FORKS: RwLock<()> = RwLock::new(());

/// Write `script` to `path` as an executable only its owner may run, and
/// close it before any test may fork again.
pub(crate) fn write(path: &Path, script: &str) {
    let _alone = FORKS.write().unwrap_or_else(|e| e.into_inner());
    let _ = std::fs::remove_file(path);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(path)
        .unwrap();
    file.write_all(script.as_bytes()).unwrap();
    file.sync_all().unwrap();
}

/// Held while a test runs a command: no executable is being written
/// meanwhile.
pub(crate) fn running() -> RwLockReadGuard<'static, ()> {
    FORKS.read().unwrap_or_else(|e| e.into_inner())
}

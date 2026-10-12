//! Read-before-write guard + stale detection (deliverable C).
//!
//! A session may write/edit/patch a tracked file only after a successful
//! read in the same session, and only while the on-disk content still
//! matches what the model saw (mtime + len + blake3). New files may be
//! written without a read. The model gets a plain-language error it can
//! recover from (re-read, then retry).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::error::{Result, ToolError};

#[derive(Clone, PartialEq, Eq)]
struct Snapshot {
    mtime: SystemTime,
    len: u64,
    hash: [u8; 32],
}

impl Snapshot {
    fn take(path: &Path) -> Result<Self> {
        let meta = std::fs::metadata(path)?;
        let hash = blake3::hash(&std::fs::read(path)?).into();
        Ok(Self {
            mtime: meta.modified()?,
            len: meta.len(),
            hash,
        })
    }
}

/// Session-scoped. Interior mutability: tools run concurrently.
pub struct ReadCache {
    seen: Mutex<HashMap<PathBuf, Option<Snapshot>>>,
}

impl Default for ReadCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadCache {
    pub fn new() -> Self {
        Self {
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Called by the read tool (and after our own writes) so the model is
    /// always editing state it has seen.
    pub fn record_read(&self, path: &Path) -> Result<()> {
        let snap = if path.exists() {
            Some(Snapshot::take(path)?)
        } else {
            None
        };
        self.seen.lock().unwrap().insert(path.to_path_buf(), snap);
        Ok(())
    }

    /// Gate for write/edit/apply_patch. `path` must be pre-resolved.
    pub fn check_writable(&self, path: &Path) -> Result<()> {
        let exists = path.exists();
        let entry = self.seen.lock().unwrap().get(path).cloned();
        match (entry, exists) {
            // Read this session, still identical on disk.
            (Some(Some(old)), true) => {
                let now = Snapshot::take(path)?;
                if now == old {
                    Ok(())
                } else {
                    Err(ToolError::Stale(format!(
                        "file changed since it was last read this session: {} - \
                         re-read it, then retry the edit",
                        path.display()
                    )))
                }
            }
            // Read when it did not exist and still doesn't: create allowed.
            (Some(None), false) => Ok(()),
            // Raced: told absent, now present (or vice versa) -> force re-read.
            (Some(None), true) => Err(ToolError::Stale(format!(
                "file appeared since it was last checked this session: {} - \
                 re-read it first",
                path.display()
            ))),
            (Some(Some(_)), false) => Err(ToolError::Stale(format!(
                "file was deleted since it was last read this session: {} - \
                 re-read to refresh state",
                path.display()
            ))),
            // Never read this session.
            (None, true) => Err(ToolError::Stale(format!(
                "write refused: read {} first this session",
                path.display()
            ))),
            (None, false) => Ok(()), // brand-new file needs no read
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_without_read_refused_then_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "hello").unwrap();
        let cache = ReadCache::new();
        assert!(matches!(cache.check_writable(&f), Err(ToolError::Stale(_))));
        cache.record_read(&f).unwrap();
        cache.check_writable(&f).unwrap();
    }

    #[test]
    fn stale_after_external_change() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "v1").unwrap();
        let cache = ReadCache::new();
        cache.record_read(&f).unwrap();
        std::fs::write(&f, "v2 different").unwrap();
        assert!(matches!(cache.check_writable(&f), Err(ToolError::Stale(_))));
    }

    #[test]
    fn own_write_refreshes_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "v1").unwrap();
        let cache = ReadCache::new();
        cache.record_read(&f).unwrap();
        std::fs::write(&f, "v2 by us").unwrap();
        cache.record_read(&f).unwrap(); // tool records its own write
        cache.check_writable(&f).unwrap();
    }

    #[test]
    fn new_file_needs_no_read_and_records_absence() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("new.txt");
        let cache = ReadCache::new();
        cache.check_writable(&f).unwrap(); // unknown + absent: create ok
        cache.record_read(&f).unwrap(); // records absence
        std::fs::write(&f, "hello").unwrap();
        cache.record_read(&f).unwrap();
        cache.check_writable(&f).unwrap();
    }

    #[test]
    fn deleted_after_read_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let cache = ReadCache::new();
        cache.record_read(&f).unwrap();
        std::fs::remove_file(&f).unwrap();
        assert!(matches!(cache.check_writable(&f), Err(ToolError::Stale(_))));
    }
}

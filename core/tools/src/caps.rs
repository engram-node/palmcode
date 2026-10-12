//! Output caps + spill files: a tool result keeps its head under byte/line
//! budgets; the full payload lives in a session-scoped spill file so the
//! model can grep/read slices from it (deliverables C/F).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::Result;

/// Per-tool output budget. Defaults are deliberately small: a result larger
/// than this almost always wants grep/slices instead of a full paste.
#[derive(Clone, Copy)]
pub struct Limits {
    pub max_lines: usize,
    pub max_bytes: usize,
}

/// Read tool: 2000-line cap per the M2 spec.
pub const READ: Limits = Limits {
    max_lines: 2000,
    max_bytes: 256 * 1024,
};
/// Listings/searches.
pub const SEARCH: Limits = Limits {
    max_lines: 1000,
    max_bytes: 128 * 1024,
};

#[derive(Debug)]
pub struct Capped {
    pub text: String,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub spilled_to: Option<PathBuf>,
}

impl Capped {
    pub fn truncated(&self) -> bool {
        self.spilled_to.is_some()
    }
}

/// Session-scoped spill sink: `<dir>/spill-<seq>-<blake3:8>.txt`.
pub struct Spiller {
    dir: PathBuf,
    seq: AtomicU64,
}

impl Spiller {
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            seq: AtomicU64::new(1),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Explicit spill for callers that already know the payload is over cap
    /// (e.g. large preimages captured by the agent).
    pub fn spill_file(&self, text: &str) -> Result<PathBuf> {
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let tag = blake3::hash(text.as_bytes()).to_hex();
        let path = self.dir.join(format!("spill-{n}-{}.txt", &tag[..8]));
        std::fs::write(&path, text)?;
        Ok(path)
    }

    /// Apply the cap. Under budget: text as-is. Over: keep the head, spill
    /// the full text, and append a pointer the model can act on.
    pub fn apply(&self, text: String, limits: Limits) -> Result<Capped> {
        let total_bytes = text.len();
        let total_lines = text.lines().count();
        if total_lines <= limits.max_lines && total_bytes <= limits.max_bytes {
            return Ok(Capped {
                text,
                total_lines,
                total_bytes,
                spilled_to: None,
            });
        }
        let spilled = self.spill_file(&text)?;
        // Byte-cut at a char boundary; then also respect the line cap on
        // the kept head.
        let mut cut = total_bytes.min(limits.max_bytes);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let head: String = text[..cut]
            .lines()
            .take(limits.max_lines)
            .collect::<Vec<_>>()
            .join("\n");
        let kept_lines = head.lines().count();
        let text = format!(
            "{head}\n[truncated: kept {kept_lines}/{total_lines} lines, \
             {}/{total_bytes} bytes; full output at {} - use grep/read \
             on that file to inspect further]",
            head.len(),
            spilled.display()
        );
        Ok(Capped {
            text,
            total_lines,
            total_bytes,
            spilled_to: Some(spilled),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_cap_passes_through() {
        let dir = tempfile::tempdir().unwrap();
        let s = Spiller::new(dir.path()).unwrap();
        let c = s.apply("small\noutput".to_string(), READ).unwrap();
        assert!(!c.truncated());
        assert_eq!(c.text, "small\noutput");
        assert_eq!(c.total_lines, 2);
    }

    #[test]
    fn over_line_cap_spills_and_points() {
        let dir = tempfile::tempdir().unwrap();
        let s = Spiller::new(dir.path()).unwrap();
        let big = (0..3000).map(|i| format!("line {i}\n")).collect::<String>();
        let c = s.apply(big.clone(), READ).unwrap();
        assert!(c.truncated());
        let spill = c.spilled_to.unwrap();
        assert_eq!(std::fs::read_to_string(&spill).unwrap(), big);
        assert!(spill.starts_with(dir.path()));
        assert!(c.text.contains("spill-1-"));
        assert!(c.text.lines().count() <= READ.max_lines + 2);
    }

    #[test]
    fn over_byte_cap_cuts_at_char_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let s = Spiller::new(dir.path()).unwrap();
        let big = "é".repeat(200 * 1024); // 2 bytes each
        let c = s.apply(big, READ).unwrap();
        assert!(c.truncated());
        let head_len = c.text.find("\n[truncated:").unwrap();
        assert!(c.text.is_char_boundary(head_len));
        assert!(head_len <= READ.max_bytes);
    }
}

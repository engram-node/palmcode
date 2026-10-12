//! Error type shared by all tools. Tool errors become tool *results* sent
//! back to the model (deliverable A: never crash the loop), so messages
//! must be model-readable, not just logs.

use std::fmt;

#[derive(Debug)]
pub enum ToolError {
    /// Path escapes every granted workspace root.
    Escape(String),
    /// Non-portable path construct (NTFS ADS, control chars, non-UTF-8).
    BadPath(String),
    Io(std::io::Error),
    /// Read-before-write guard: file changed since last read this session.
    Stale(String),
    /// Edit match failures carry the model-actionable hint.
    Match(String),
    /// Output/feature guard (binary file, size cap, context guard...).
    Guard(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Escape(p) => write!(f, "path escapes the granted workspace: {p}"),
            Self::BadPath(p) => write!(f, "unsafe or unsupported path: {p}"),
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Stale(m) => write!(f, "{m}"),
            Self::Match(m) => write!(f, "{m}"),
            Self::Guard(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ToolError {}

impl From<std::io::Error> for ToolError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, ToolError>;

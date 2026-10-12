//! palmcode-tools -- M2 file tools: read/write/edit/apply_patch/glob/grep/ls
//! (+ todo/question wired at the agent level).
//!
//! Everything goes through [`paths::RootSet`] before touching the
//! filesystem; tool errors are model-readable and never crash the loop.

pub mod error;
pub mod paths;

pub use error::{Result, ToolError};
pub use paths::RootSet;

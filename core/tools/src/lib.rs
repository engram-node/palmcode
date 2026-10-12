//! palmcode-tools -- M2 file tools: read/write/edit/apply_patch/glob/grep/ls
//! (+ todo/question wired at the agent level).
//!
//! Everything goes through [`paths::RootSet`] before touching the
//! filesystem; tool errors are model-readable and never crash the loop.

pub mod caps;
pub mod error;
pub mod guards;
pub mod paths;

pub use caps::{Capped, Limits, Spiller};
pub use error::{Result, ToolError};
pub use guards::ReadCache;
pub use paths::RootSet;

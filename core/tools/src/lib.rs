//! palmcode-tools -- M2 file tools: read/write/edit/apply_patch/glob/grep/ls
//! (+ todo/question wired at the agent level).
//!
//! Everything goes through [`paths::RootSet`] before touching the
//! filesystem; tool errors are model-readable and never crash the loop.

pub mod caps;
pub mod edit;
pub mod error;
pub mod glob;
pub mod grep;
pub mod guards;
pub mod list;
pub mod patch;
pub mod paths;
pub mod read;

pub use caps::{Capped, Limits, Spiller};
pub use error::{Result, ToolError};
pub use guards::ReadCache;
pub use paths::RootSet;

/// Shared per-session tool context.
pub struct Ctx<'a> {
    pub roots: &'a RootSet,
    pub reads: &'a ReadCache,
    pub spill: &'a Spiller,
}

/// Deterministic schema list for the LLM request (byte-stable order:
/// prefix-cache friendly, deliverable I).
pub fn schemas() -> Vec<serde_json::Value> {
    vec![
        read::schema(),
        list::schema(),
        glob::schema(),
        grep::schema(),
        edit::write_schema(),
        edit::edit_schema(),
        patch::schema(),
    ]
}

/// Dispatch one tool call. Unknown tool -> model-readable error.
pub fn run_tool(name: &str, args_json: &str, ctx: &Ctx) -> Result<Capped> {
    match name {
        "read" => {
            let args: read::Args = serde_json::from_str(args_json)
                .map_err(|e| ToolError::Guard(format!("invalid read args: {e}")))?;
            read::run(&args, ctx.roots, ctx.reads, ctx.spill)
        }
        "ls" => {
            let args: list::Args = serde_json::from_str(args_json)
                .map_err(|e| ToolError::Guard(format!("invalid ls args: {e}")))?;
            list::run(&args, ctx.roots, ctx.spill)
        }
        "glob" => {
            let args: glob::Args = serde_json::from_str(args_json)
                .map_err(|e| ToolError::Guard(format!("invalid glob args: {e}")))?;
            glob::run(&args, ctx.roots, ctx.spill)
        }
        "grep" => {
            let args: grep::Args = serde_json::from_str(args_json)
                .map_err(|e| ToolError::Guard(format!("invalid grep args: {e}")))?;
            grep::run(&args, ctx.roots, ctx.spill)
        }
        "write" => {
            let args: edit::WriteArgs = serde_json::from_str(args_json)
                .map_err(|e| ToolError::Guard(format!("invalid write args: {e}")))?;
            let summary = edit::run_write(&args, ctx.roots, ctx.reads)?;
            ctx.spill.apply(summary, caps::SEARCH)
        }
        "edit" => {
            let args: edit::EditArgs = serde_json::from_str(args_json)
                .map_err(|e| ToolError::Guard(format!("invalid edit args: {e}")))?;
            let out = edit::run_edit(&args, ctx.roots, ctx.reads)?;
            ctx.spill.apply(out.summary, caps::SEARCH)
        }
        "apply_patch" => {
            let args: patch::Args = serde_json::from_str(args_json)
                .map_err(|e| ToolError::Guard(format!("invalid apply_patch args: {e}")))?;
            let summary = patch::run(&args, ctx.roots, ctx.reads)?;
            ctx.spill.apply(summary, caps::SEARCH)
        }
        other => Err(ToolError::Guard(format!("unknown tool: {other}"))),
    }
}

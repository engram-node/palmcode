//! palmcode-tools -- M2 file tools: read/write/edit/apply_patch/glob/grep/ls
//! (+ todo/question wired at the agent level).
//!
//! Everything goes through [`paths::RootSet`] before touching the
//! filesystem; tool errors are model-readable and never crash the loop.

pub mod caps;
pub mod diff;
pub mod edit;
pub mod error;
pub mod glob;
pub mod grep;
pub mod guards;
pub mod list;
pub mod patch;
pub mod paths;
pub mod read;
pub mod syntax;

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
            let mut capped = ctx.spill.apply(summary, caps::SEARCH)?;
            syntax_note(ctx.roots, std::slice::from_ref(&args.path), &mut capped);
            Ok(capped)
        }
        "edit" => {
            let args: edit::EditArgs = serde_json::from_str(args_json)
                .map_err(|e| ToolError::Guard(format!("invalid edit args: {e}")))?;
            let out = edit::run_edit(&args, ctx.roots, ctx.reads)?;
            let mut capped = ctx.spill.apply(out.summary, caps::SEARCH)?;
            syntax_note(ctx.roots, std::slice::from_ref(&args.path), &mut capped);
            Ok(capped)
        }
        "apply_patch" => {
            let args: patch::Args = serde_json::from_str(args_json)
                .map_err(|e| ToolError::Guard(format!("invalid apply_patch args: {e}")))?;
            let patch_text = args.patch.clone();
            let summary = patch::run(&args, ctx.roots, ctx.reads)?;
            let mut capped = ctx.spill.apply(summary, caps::SEARCH)?;
            let files = patch::mentioned_files(&patch_text);
            syntax_note(ctx.roots, &files, &mut capped);
            Ok(capped)
        }
        other => Err(ToolError::Guard(format!("unknown tool: {other}"))),
    }
}

/// Deliverable C: append a tree-sitter syntax warning after writes/edits —
/// out of band after the cap so a full-size result never eats the warning.
fn syntax_note(roots: &RootSet, files: &[String], out: &mut Capped) {
    for rel in files.iter().take(9) {
        if let Some(w) = syntax::check_rooted(roots, rel) {
            out.text.push_str(&format!("\n\n[{rel}] {w}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> (tempfile::TempDir, RootSet, ReadCache, Spiller) {
        let dir = tempfile::tempdir().unwrap();
        let roots = RootSet::new(vec![dir.path().to_path_buf()]).unwrap();
        let spill = Spiller::new(dir.path().join("spill")).unwrap();
        (dir, roots, ReadCache::new(), spill)
    }

    fn run(name: &str, args: serde_json::Value, ctx: &Ctx) -> Result<Capped> {
        run_tool(name, &args.to_string(), ctx)
    }

    #[test]
    fn write_appends_syntax_warning_on_broken_code() {
        let (_d, roots, reads, spill) = env();
        std::fs::write(roots.roots()[0].join("a.rs"), "fn a() {}\n").unwrap();
        reads.record_read(&roots.roots()[0].join("a.rs")).unwrap();
        let ctx = Ctx {
            roots: &roots,
            reads: &reads,
            spill: &spill,
        };
        let out = run(
            "write",
            serde_json::json!({"path": "a.rs", "content": "fn main( { \n"}),
            &ctx,
        )
        .unwrap();
        assert!(out.text.contains("syntax: rust"), "{}", out.text);
    }

    #[test]
    fn write_clean_code_stays_silent() {
        let (_d, roots, reads, spill) = env();
        let ctx = Ctx {
            roots: &roots,
            reads: &reads,
            spill: &spill,
        };
        let out = run(
            "write",
            serde_json::json!({"path": "b.rs", "content": "fn main() {}\n"}),
            &ctx,
        )
        .unwrap();
        assert!(!out.text.contains("syntax:"), "{}", out.text);
        // unknown extension: no warning either (by design)
        let out = run(
            "write",
            serde_json::json!({"path": "b.txt", "content": "(((["}),
            &ctx,
        )
        .unwrap();
        assert!(!out.text.contains("syntax:"), "{}", out.text);
    }

    #[test]
    fn patch_checks_every_touched_known_file() {
        let (_d, roots, reads, spill) = env();
        std::fs::write(roots.roots()[0].join("old.py"), "def f():\n    return 1\n").unwrap();
        reads.record_read(&roots.roots()[0].join("old.py")).unwrap();
        let ctx = Ctx {
            roots: &roots,
            reads: &reads,
            spill: &spill,
        };
        let patch = "*** Begin Patch\n\
                     *** Add File: new.rs\n\
                     +fn main( { \n\
                     *** Update File: old.py\n\
                     @@\n\
                     -    return 1\n\
                     +    return 2\n\
                     *** End Patch\n";
        let out = run("apply_patch", serde_json::json!({"patch": patch}), &ctx).unwrap();
        assert!(out.text.contains("[new.rs] syntax: rust"), "{}", out.text);
        assert!(
            !out.text.contains("[old.py]"),
            "clean file silent: {}",
            out.text
        );
    }

    #[test]
    fn warning_survives_a_max_size_result() {
        let (_d, roots, reads, spill) = env();
        std::fs::write(roots.roots()[0].join("big.rs"), "fn a() {}\n").unwrap();
        reads.record_read(&roots.roots()[0].join("big.rs")).unwrap();
        let ctx = Ctx {
            roots: &roots,
            reads: &reads,
            spill: &spill,
        };
        let out = run(
            "write",
            serde_json::json!({"path": "big.rs", "content": "fn main( { /*unterminated" }),
            &ctx,
        )
        .unwrap();
        assert!(
            out.text.trim_end().ends_with("broke the file"),
            "warning is last, never capped: {}",
            out.text
        );
        assert!(out.text.contains("syntax: rust"));
    }
}

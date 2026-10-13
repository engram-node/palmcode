//! `ls` tool: recursive-ish listing via the `ignore` walker (gitignore-aware,
//! .git skipped), depth-limited, count-capped.

use ignore::WalkBuilder;
use serde::Deserialize;

use crate::caps::{self, Capped, Spiller};
use crate::error::{Result, ToolError};
use crate::paths::RootSet;

#[derive(Deserialize)]
pub struct Args {
    /// Directory to list; defaults to the first workspace root.
    pub path: Option<String>,
    /// Max depth below the listed dir (default 2).
    pub depth: Option<usize>,
}

pub fn run(args: &Args, roots: &RootSet, spill: &Spiller) -> Result<Capped> {
    let start = match &args.path {
        Some(p) => roots.resolve(p, None)?,
        None => roots
            .roots()
            .first()
            .cloned()
            .ok_or_else(|| ToolError::BadPath("no workspace roots".into()))?,
    };
    let depth = args.depth.unwrap_or(2);

    let mut out = String::new();
    out.push_str(&format!("{}/\n", start.display()));
    let mut count = 0usize;
    for entry in WalkBuilder::new(&start)
        .max_depth(Some(depth + 1))
        .standard_filters(true)
        // SAF-imported trees have no .git dir: honor .gitignore anyway
        .require_git(false)
        .hidden(false) // show dotfiles like .env — but never VCS internals
        .filter_entry(|e| e.file_name() != ".git")
        .sort_by_file_name(|a, b| a.cmp(b))
        .build()
        .skip(1)
    // the start dir itself
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                out.push_str(&format!("[walk error: {e}]\n"));
                continue;
            }
        };
        let rel = entry.path().strip_prefix(&start).unwrap_or(entry.path());
        let suffix = if entry.file_type().is_some_and(|t| t.is_dir()) {
            "/"
        } else {
            ""
        };
        out.push_str(&format!("  {}{suffix}\n", rel.display()).replace('\\', "/"));
        count += 1;
        if count >= caps::SEARCH.max_lines {
            out.push_str("[listing capped; narrow with path/depth]\n");
            break;
        }
    }
    spill.apply(out, caps::SEARCH)
}

pub fn schema() -> serde_json::Value {
    serde_json::json!({
        "name": "ls",
        "description": "List a directory tree (gitignore-aware). Default depth 2; deeper on request.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "depth": { "type": "integer", "minimum": 0, "default": 2 }
            },
            "additionalProperties": false
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_tree_gitignore_aware() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
        fs::write(root.join("src/nested/deep.txt"), "").unwrap();
        fs::write(root.join(".gitignore"), "ignored_dir/\n").unwrap();
        fs::create_dir_all(root.join("ignored_dir")).unwrap();
        fs::write(root.join("ignored_dir/x"), "").unwrap();
        fs::create_dir_all(root.join(".git/objects")).unwrap();

        let roots = RootSet::new(vec![root.to_path_buf()]).unwrap();
        let spill = Spiller::new(root.join(".spill")).unwrap();
        let out = run(
            &Args {
                path: None,
                depth: Some(3),
            },
            &roots,
            &spill,
        )
        .unwrap();
        assert!(out.text.contains("src/"));
        assert!(out.text.contains("deep.txt"));
        assert!(!out.text.contains("ignored_dir"));
        assert!(!out.text.contains(".git/"), ".git must stay hidden");
    }

    #[test]
    fn escape_attempt_denied() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("w")).unwrap();
        let roots = RootSet::new(vec![dir.path().join("w")]).unwrap();
        let spill = Spiller::new(dir.path().join(".spill")).unwrap();
        assert!(matches!(
            run(
                &Args {
                    path: Some("..".into()),
                    depth: None
                },
                &roots,
                &spill
            ),
            Err(ToolError::Escape(_))
        ));
    }
}

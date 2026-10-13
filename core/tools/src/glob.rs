//! `glob` tool: gitignore-aware file discovery, matches sorted by mtime
//! (newest first — the files you just wrote are what you're looking for).

use globset::{Glob, GlobSetBuilder};
use serde::Deserialize;

use crate::caps::{self, Capped, Spiller};
use crate::error::{Result, ToolError};
use crate::paths::RootSet;

#[derive(Deserialize)]
pub struct Args {
    /// e.g. "**/*.rs", "src/**/test_*.py"
    pub pattern: String,
    /// Directory to search; default first root.
    pub path: Option<String>,
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
    let glob = Glob::new(&args.pattern)
        .map_err(|e| ToolError::Guard(format!("bad glob '{}': {e}", args.pattern)))?;
    let mut builder = GlobSetBuilder::new();
    builder.add(glob);
    let set = builder
        .build()
        .map_err(|e| ToolError::Guard(format!("bad glob: {e}")))?;

    let mut hits: Vec<(std::time::SystemTime, String, bool)> = Vec::new();
    for entry in ignore::WalkBuilder::new(&start)
        .standard_filters(true)
        .hidden(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .skip(1)
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let rel = entry.path().strip_prefix(&start).unwrap_or(entry.path());
        if !set.is_match(rel) {
            continue;
        }
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        hits.push((mtime, rel.display().to_string().replace('\\', "/"), is_dir));
        if hits.len() >= caps::SEARCH.max_lines {
            break;
        }
    }
    hits.sort_by_key(|h| std::cmp::Reverse(h.0)); // newest first
    let mut out = String::new();
    for (_, rel, is_dir) in &hits {
        out.push_str(rel);
        if *is_dir {
            out.push('/');
        }
        out.push('\n');
    }
    if hits.is_empty() {
        out.push_str("[no matches]\n");
    } else if hits.len() >= caps::SEARCH.max_lines {
        out.push_str("[capped; narrow the pattern]\n");
    }
    spill.apply(out, caps::SEARCH)
}

pub fn schema() -> serde_json::Value {
    serde_json::json!({
        "name": "glob",
        "description": "Find files by glob pattern (gitignore-aware), newest first.",
        "input_schema": {
            "type": "object",
            "properties": {
                "pattern": { "type": "string" },
                "path": { "type": "string" }
            },
            "required": ["pattern"],
            "additionalProperties": false
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn env() -> (tempfile::TempDir, RootSet, Spiller) {
        let dir = tempfile::tempdir().unwrap();
        let roots = RootSet::new(vec![dir.path().to_path_buf()]).unwrap();
        let spill = Spiller::new(dir.path().join(".spill")).unwrap();
        (dir, roots, spill)
    }

    #[test]
    fn stars_and_newest_first() {
        let (dir, roots, spill) = env();
        let root = dir.path();
        fs::create_dir_all(root.join("src/sub")).unwrap();
        fs::write(root.join("src/a.rs"), "a").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        fs::write(root.join("src/sub/b.rs"), "b").unwrap();
        fs::write(root.join("readme.md"), "m").unwrap();

        let out = run(
            &Args {
                pattern: "**/*.rs".into(),
                path: None,
            },
            &roots,
            &spill,
        )
        .unwrap();
        let lines: Vec<&str> = out.text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("b.rs"), "newest first: {lines:?}");
    }

    #[test]
    fn bad_pattern_is_model_readable_error() {
        let (_d, roots, spill) = env();
        let err = run(
            &Args {
                pattern: "[unclosed".into(),
                path: None,
            },
            &roots,
            &spill,
        )
        .unwrap_err();
        assert!(matches!(err, ToolError::Guard(_)), "{err:?}");
    }
}

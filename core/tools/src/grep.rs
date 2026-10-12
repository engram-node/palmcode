//! `grep` tool: in-process regex search via grep-regex + grep-searcher over
//! the `ignore` walker. No bundled rg binary (per M2 spec). Output format
//! `path:lineno: text`, capped.

use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use serde::Deserialize;

use crate::caps::{self, Capped, Spiller};
use crate::error::{Result, ToolError};
use crate::paths::RootSet;

#[derive(Deserialize)]
pub struct Args {
    /// Rust regex.
    pub pattern: String,
    /// Directory/file to search; default first root.
    pub path: Option<String>,
    /// Only search files matching this glob, e.g. "*.rs".
    pub include: Option<String>,
    pub case_insensitive: Option<bool>,
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
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(args.case_insensitive.unwrap_or(false))
        .build(&args.pattern)
        .map_err(|e| ToolError::Guard(format!("bad regex '{}': {e}", args.pattern)))?;
    let include = match &args.include {
        Some(g) => Some(
            globset::GlobBuilder::new(g)
                .build()
                .map_err(|e| ToolError::Guard(format!("bad include glob '{g}': {e}")))?
                .compile_matcher(),
        ),
        None => None,
    };

    let mut out = String::new();
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(true)
        .build();

    let mut hits = 0usize;
    let mut files = 0usize;
    for entry in ignore::WalkBuilder::new(&start)
        .standard_filters(true)
        .hidden(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = entry.path().strip_prefix(&start).unwrap_or(entry.path());
        if let Some(inc) = &include
            && !inc.is_match(rel)
        {
            continue;
        }
        files += 1;
        let rel_str = rel.display().to_string().replace('\\', "/");
        let mut overflow = false;
        let result = searcher.search_path(
            &matcher,
            entry.path(),
            UTF8(|lineno, text| {
                out.push_str(&format!(
                    "{rel_str}:{lineno}: {}",
                    text.trim_end_matches('\n')
                ));
                out.push('\n');
                hits += 1;
                if hits >= caps::SEARCH.max_lines {
                    overflow = true;
                    return Ok(false); // stop this file early; note below
                }
                Ok(true)
            }),
        );
        if let Err(e) = result {
            // permission denied / unreadable: say so, keep walking
            out.push_str(&format!("[search error in {rel_str}: {e}]\n"));
        }
        if overflow {
            out.push_str("[match cap hit; narrow the pattern or path]\n");
            break;
        }
    }
    if hits == 0 {
        out.push_str(&format!("[no matches in {files} files]\n"));
    }
    spill.apply(out, caps::SEARCH)
}

pub fn schema() -> serde_json::Value {
    serde_json::json!({
        "name": "grep",
        "description": "Regex search file contents (gitignore-aware). Output: path:line: text.",
        "input_schema": {
            "type": "object",
            "properties": {
                "pattern": { "type": "string" },
                "path": { "type": "string" },
                "include": { "type": "string", "description": "filename glob like *.rs" },
                "case_insensitive": { "type": "boolean" }
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

    #[test]
    fn finds_line_numbers_and_include_filter() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "fn main() {\n  unsafe { x() }\n}\n").unwrap();
        fs::write(root.join("src/a.md"), "unsafe in docs\n").unwrap();
        let roots = RootSet::new(vec![root.to_path_buf()]).unwrap();
        let spill = Spiller::new(root.join(".spill")).unwrap();
        let out = run(
            &Args {
                pattern: "unsafe".into(),
                path: None,
                include: Some("*.rs".into()),
                case_insensitive: None,
            },
            &roots,
            &spill,
        )
        .unwrap();
        assert!(out.text.contains("src/a.rs:2:"));
        assert!(!out.text.contains("a.md"));
    }

    #[test]
    fn bad_regex_model_readable() {
        let dir = tempfile::tempdir().unwrap();
        let roots = RootSet::new(vec![dir.path().to_path_buf()]).unwrap();
        let spill = Spiller::new(dir.path().join(".spill")).unwrap();
        let err = run(
            &Args {
                pattern: "(unclosed".into(),
                path: None,
                include: None,
                case_insensitive: None,
            },
            &roots,
            &spill,
        )
        .unwrap_err();
        assert!(matches!(err, ToolError::Guard(_)));
    }

    #[test]
    fn warm_2k_files_fast() {
        // Host proxy for the 10k-file <300ms device budget; device number
        // recorded at M2 acceptance.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        for i in 0..2000 {
            fs::write(
                root.join(format!("src/f{i:04}.rs")),
                "fn f() {}\n// comment line\nmod m {}\n",
            )
            .unwrap();
        }
        fs::write(root.join("src/target.rs"), "needle here\n").unwrap();
        let roots = RootSet::new(vec![root.to_path_buf()]).unwrap();
        let spill = Spiller::new(root.join(".spill")).unwrap();
        // warm pass (populate OS cache)
        let _ = run(
            &Args {
                pattern: "fn f".into(),
                path: None,
                include: Some("*.rs".into()),
                case_insensitive: None,
            },
            &roots,
            &spill,
        )
        .unwrap();
        let t = std::time::Instant::now();
        let out = run(
            &Args {
                pattern: "needle".into(),
                path: None,
                include: None,
                case_insensitive: None,
            },
            &roots,
            &spill,
        )
        .unwrap();
        let elapsed = t.elapsed();
        assert!(out.text.contains("target.rs:1:"));
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "2k-file warm grep took {elapsed:?}"
        );
    }
}

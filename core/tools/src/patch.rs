//! `apply_patch` tool: `*** Begin Patch` / `*** End Patch` envelopes with
//! Add File / Delete File / Update File (+ `@@` hunks, `*** End of File`
//! anchor). Update hunks reuse the edit tier ladder; each file is written
//! once, atomically. (Move-to renames: not in M2 scope.)

use std::path::PathBuf;

use serde::Deserialize;

use crate::ReadCache;
use crate::edit;
use crate::error::{Result, ToolError};
use crate::paths::RootSet;

#[derive(Deserialize)]
pub struct Args {
    pub patch: String,
}

#[derive(Debug, PartialEq)]
enum Change {
    Context(String),
    Remove(String),
    Add(String),
}

#[derive(Debug)]
struct Hunk {
    changes: Vec<Change>,
    at_eof: bool,
}

#[derive(Debug)]
enum Op {
    Add { path: String, lines: Vec<String> },
    Delete { path: String },
    Update { path: String, hunks: Vec<Hunk> },
}

fn parse(patch: &str) -> std::result::Result<Vec<Op>, String> {
    let mut lines = patch.lines().peekable();
    match lines.next() {
        Some("*** Begin Patch") => {}
        _ => return Err("patch must start with '*** Begin Patch'".into()),
    }
    let mut ops = Vec::new();
    loop {
        let line = lines
            .next()
            .ok_or_else(|| "patch missing '*** End Patch'".to_string())?;
        match line.strip_prefix("*** ") {
            Some("End Patch") => break,
            Some(rest) if rest.starts_with("Add File: ") => {
                let path = rest["Add File: ".len()..].trim().to_string();
                let mut content = Vec::new();
                while let Some(l) = lines.peek() {
                    if l.starts_with("*** ") {
                        break;
                    }
                    let l = lines.next().unwrap();
                    content.push(l.strip_prefix('+').unwrap_or(l).to_string());
                }
                ops.push(Op::Add {
                    path,
                    lines: content,
                });
            }
            Some(rest) if rest.starts_with("Delete File: ") => {
                ops.push(Op::Delete {
                    path: rest["Delete File: ".len()..].trim().to_string(),
                });
            }
            Some(rest) if rest.starts_with("Update File: ") => {
                let path = rest["Update File: ".len()..].trim().to_string();
                let mut hunks: Vec<Hunk> = Vec::new();
                let mut cur = Hunk {
                    changes: Vec::new(),
                    at_eof: false,
                };
                let mut has_changes = false;
                while let Some(l) = lines.peek() {
                    let l = *l;
                    if l.starts_with("*** ") {
                        if l == "*** End of File" {
                            lines.next();
                            cur.at_eof = true;
                            continue;
                        }
                        break;
                    }
                    let l = lines.next().unwrap();
                    if let Some(_anchor) = l.strip_prefix("@@") {
                        if has_changes {
                            hunks.push(std::mem::replace(
                                &mut cur,
                                Hunk {
                                    changes: Vec::new(),
                                    at_eof: false,
                                },
                            ));
                            has_changes = false;
                        }
                        // anchor text is a human hint only; our matchers
                        // locate hunks by their content lines
                        continue;
                    }
                    has_changes = true;
                    if let Some(c) = l.strip_prefix('-') {
                        cur.changes.push(Change::Remove(c.to_string()));
                    } else if let Some(c) = l.strip_prefix('+') {
                        cur.changes.push(Change::Add(c.to_string()));
                    } else {
                        let c = l.strip_prefix(' ').unwrap_or(l);
                        cur.changes.push(Change::Context(c.to_string()));
                    }
                }
                if has_changes || cur.at_eof {
                    hunks.push(cur);
                }
                if hunks.is_empty() {
                    return Err(format!("Update File: {path}: no hunks"));
                }
                ops.push(Op::Update { path, hunks });
            }
            _ => return Err(format!("unrecognized patch line: {line}")),
        }
    }
    if ops.is_empty() {
        return Err("empty patch".into());
    }
    Ok(ops)
}

/// Build (old, new) hunk texts: old = context+removed lines, new =
/// context+added lines.
fn hunk_texts(h: &Hunk) -> (String, String) {
    let mut old = String::new();
    let mut new = String::new();
    for c in &h.changes {
        match c {
            Change::Context(l) => {
                old.push_str(l);
                old.push('\n');
                new.push_str(l);
                new.push('\n');
            }
            Change::Remove(l) => {
                old.push_str(l);
                old.push('\n');
            }
            Change::Add(l) => {
                new.push_str(l);
                new.push('\n');
            }
        }
    }
    (
        old.trim_end_matches('\n').to_string(),
        new.trim_end_matches('\n').to_string(),
    )
}

pub fn run(args: &Args, roots: &RootSet, reads: &ReadCache) -> Result<String> {
    let ops = parse(&args.patch).map_err(ToolError::Match)?;

    // Phase 1: resolve every path up front (no partial application of an
    // envelope with an escaping path).
    let mut resolved: Vec<PathBuf> = Vec::new();
    for op in &ops {
        let path = match op {
            Op::Add { path, .. } | Op::Delete { path } | Op::Update { path, .. } => path,
        };
        resolved.push(roots.resolve(path, None)?);
    }

    // Phase 2: compute all new contents.
    let mut actions: Vec<(PathBuf, Option<String>, String)> = Vec::new(); // (path, Option<new_text>, note)
    for (op, path) in ops.iter().zip(&resolved) {
        match op {
            Op::Add { lines, .. } => {
                if path.exists() {
                    return Err(ToolError::Match(format!(
                        "Add File refused, already exists: {}",
                        path.display()
                    )));
                }
                actions.push((path.clone(), Some(lines.join("\n") + "\n"), "added".into()));
            }
            Op::Delete { .. } => {
                reads.check_writable(path)?;
                actions.push((path.clone(), None, "deleted".into()));
            }
            Op::Update { hunks, .. } => {
                reads.check_writable(path)?;
                let mut ft = edit::load(path)?;
                for (i, h) in hunks.iter().enumerate() {
                    let (old, new) = hunk_texts(h);
                    let applied = if h.at_eof && old.is_empty() {
                        // pure EOF append
                        let mut t = ft.text.clone();
                        t.push_str(&new);
                        t.push('\n');
                        t
                    } else {
                        edit::replace_in_text(&ft.text, &old, &new, false)
                            .map_err(|m| {
                                ToolError::Match(format!(
                                    "{} (hunk {}): {m}",
                                    path.display(),
                                    i + 1
                                ))
                            })?
                            .0
                    };
                    ft.text = applied;
                }
                actions.push((path.clone(), Some(ft.text.clone()), "updated".into()));
            }
        }
    }

    // Phase 3: write everything.
    let mut notes = Vec::new();
    for (path, content, note) in actions {
        match content {
            Some(_) => {
                let ft = edit::load(&path).unwrap_or(edit::FileText {
                    bom: false,
                    crlf: false,
                    text: String::new(),
                });
                let new_ft = edit::FileText {
                    bom: ft.bom,
                    crlf: ft.crlf,
                    text: content.unwrap().replace("\r\n", "\n"),
                };
                edit::atomic_write(&path, &edit::encode(&new_ft))?;
                reads.record_read(&path)?;
            }
            None => {
                std::fs::remove_file(&path)?;
                reads.record_read(&path)?;
            }
        }
        notes.push(format!("{} {}", note, path.display()));
    }
    Ok(notes.join("\n"))
}

/// Every file an envelope mentions (agent: preimage capture before running).
pub fn mentioned_files(patch: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("*** ") {
            for kind in ["Add File: ", "Delete File: ", "Update File: "] {
                if let Some(p) = rest.strip_prefix(kind) {
                    out.push(p.trim().to_string());
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

pub fn schema() -> serde_json::Value {
    serde_json::json!({
        "name": "apply_patch",
        "description": "Multi-file patch: *** Begin Patch / *** Add|Update|Delete File / @@ hunks / *** End Patch.",
        "input_schema": {
            "type": "object",
            "properties": {
                "patch": { "type": "string" }
            },
            "required": ["patch"],
            "additionalProperties": false
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn env() -> (tempfile::TempDir, RootSet, ReadCache) {
        let dir = tempfile::tempdir().unwrap();
        let roots = RootSet::new(vec![dir.path().to_path_buf()]).unwrap();
        (dir, roots, ReadCache::new())
    }

    #[test]
    fn fixture_basic_update() {
        let (_d, roots, reads) = env();
        let p = roots.roots()[0].join("src/main.rs");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(
            &p,
            include_str!("../fixtures/patches/basic_update.input.rs"),
        )
        .unwrap();
        reads.record_read(&p).unwrap();
        let note = run(
            &Args {
                patch: include_str!("../fixtures/patches/basic_update.patch").into(),
            },
            &roots,
            &reads,
        )
        .unwrap();
        assert!(note.contains("updated"));
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            include_str!("../fixtures/patches/basic_update.expected.rs")
        );
    }

    #[test]
    fn fixture_multi_update_add_delete() {
        let (_d, roots, reads) = env();
        let a = roots.roots()[0].join("a.txt");
        let b = roots.roots()[0].join("b.txt");
        fs::write(&a, "old line\n").unwrap();
        fs::write(&b, "bye\n").unwrap();
        reads.record_read(&a).unwrap();
        reads.record_read(&b).unwrap();
        run(
            &Args {
                patch: include_str!("../fixtures/patches/multi.patch").into(),
            },
            &roots,
            &reads,
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&a).unwrap(), "new line\n");
        assert_eq!(
            fs::read_to_string(roots.roots()[0].join("c.txt")).unwrap(),
            "brand new\nfile here\n"
        );
        assert!(!b.exists());
    }

    #[test]
    fn fixture_eof_append() {
        let (_d, roots, reads) = env();
        let p = roots.roots()[0].join("doc.md");
        fs::write(&p, include_str!("../fixtures/patches/eof.input.md")).unwrap();
        reads.record_read(&p).unwrap();
        run(
            &Args {
                patch: include_str!("../fixtures/patches/eof.patch").into(),
            },
            &roots,
            &reads,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            include_str!("../fixtures/patches/eof.expected.md")
        );
    }

    #[test]
    fn fixture_fail_mismatch_names_file_and_hunk() {
        let (_d, roots, reads) = env();
        let a = roots.roots()[0].join("a.txt");
        fs::write(&a, "old line\n").unwrap();
        reads.record_read(&a).unwrap();
        let err = run(
            &Args {
                patch: include_str!("../fixtures/patches/fail_mismatch.patch").into(),
            },
            &roots,
            &reads,
        )
        .unwrap_err();
        match err {
            ToolError::Match(m) => {
                assert!(m.contains("hunk 1"), "{m}");
                assert!(m.contains("a.txt"), "{m}");
            }
            other => panic!("{other:?}"),
        }
        // nothing applied (a.txt untouched)
        assert_eq!(fs::read_to_string(&a).unwrap(), "old line\n");
    }

    #[test]
    fn malformed_envelope_is_model_readable() {
        let (_d, roots, reads) = env();
        let err = run(
            &Args {
                patch: "nope".into(),
            },
            &roots,
            &reads,
        )
        .unwrap_err();
        assert!(matches!(err, ToolError::Match(_)));
    }
}

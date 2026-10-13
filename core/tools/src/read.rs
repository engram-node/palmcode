//! `read` tool: line-numbered, offset/limit, binary refusal with size,
//! long-line truncation. Output goes through the shared cap/spill.

use std::io::BufRead;
use std::io::BufReader;

use crate::ReadCache;
use crate::caps::{self, Capped, Spiller};
use crate::error::{Result, ToolError};
use crate::paths::RootSet;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Args {
    pub path: String,
    /// 1-based starting line.
    pub offset: Option<usize>,
    /// Max lines to return (may still be capped).
    pub limit: Option<usize>,
}

const LONG_LINE: usize = 2000;
const BINARY_SNIFF: usize = 8192;

pub fn run(args: &Args, roots: &RootSet, reads: &ReadCache, spill: &Spiller) -> Result<Capped> {
    let path = roots.resolve(&args.path, None)?;
    let meta = std::fs::metadata(&path)?;
    if !meta.is_file() {
        return Err(ToolError::BadPath(format!(
            "not a file: {}",
            path.display()
        )));
    }

    let mut sniff = vec![0u8; BINARY_SNIFF];
    let n = {
        use std::io::Read;
        let mut f = std::fs::File::open(&path)?;
        f.read(&mut sniff).unwrap_or(0)
    };
    if sniff[..n].contains(&0) {
        return Err(ToolError::Guard(format!(
            "binary file, refusing to dump ({} bytes): {}",
            meta.len(),
            path.display()
        )));
    }

    let file = std::fs::File::open(&path)?;
    let reader = BufReader::new(file);
    let offset = args.offset.unwrap_or(1).max(1);
    let limit = args.limit.unwrap_or(caps::READ.max_lines);

    let mut out = String::new();
    let mut non_utf8 = false;
    let mut continues_at: Option<usize> = None;
    for (idx, line) in reader.lines().enumerate() {
        let lineno = idx + 1;
        if lineno < offset {
            continue;
        }
        if lineno - offset >= limit {
            continues_at = Some(lineno);
            break;
        }
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                non_utf8 = true;
                format!("[decode error: {e}]")
            }
        };
        let line = if line.chars().count() > LONG_LINE {
            format!(
                "{}… [line truncated at {LONG_LINE} chars]",
                line.chars().take(LONG_LINE).collect::<String>()
            )
        } else {
            line
        };
        out.push_str(&lineno.to_string());
        out.push_str(": ");
        out.push_str(&line);
        out.push('\n');
    }
    if non_utf8 {
        out.insert_str(0, "[note: non-UTF-8 bytes encountered]\n");
    }

    reads.record_read(&path)?;
    let mut capped = spill.apply(out, caps::READ)?;
    // The paging marker must survive the cap: append after capping.
    if let Some(next) = continues_at {
        capped.text.push_str(&format!(
            "[stopped at limit {limit}; file continues — pass offset={next}]\n"
        ));
    }
    Ok(capped)
}

pub fn schema() -> serde_json::Value {
    serde_json::json!({
        "name": "read",
        "description": "Read a text file with 1-based line numbers. offset/limit page through long files.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path relative to the workspace (or absolute inside it)" },
                "offset": { "type": "integer", "minimum": 1 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 2000 }
            },
            "required": ["path"],
            "additionalProperties": false
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Env {
        _dir: tempfile::TempDir,
        roots: RootSet,
        reads: ReadCache,
        spill: Spiller,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let roots = RootSet::new(vec![dir.path().to_path_buf()]).unwrap();
        let spill = Spiller::new(dir.path().join(".spill")).unwrap();
        Env {
            _dir: dir,
            roots,
            reads: ReadCache::new(),
            spill,
        }
    }

    #[test]
    fn numbered_lines_and_offset_limit() {
        let e = env();
        fs::write(
            e.roots.roots()[0].join("f.txt"),
            (1..=10).map(|i| format!("l{i}\n")).collect::<String>(),
        )
        .unwrap();
        let a = Args {
            path: "f.txt".into(),
            offset: Some(3),
            limit: Some(2),
        };
        let out = run(&a, &e.roots, &e.reads, &e.spill).unwrap();
        assert!(out.text.starts_with("3: l3\n4: l4\n"));
        assert!(out.text.contains("offset=5"));
    }

    #[test]
    fn binary_refused_with_size() {
        let e = env();
        let mut data = b"abc".to_vec();
        data.push(0);
        data.extend_from_slice(&[1u8; 100]);
        fs::write(e.roots.roots()[0].join("b.bin"), &data).unwrap();
        let a = Args {
            path: "b.bin".into(),
            offset: None,
            limit: None,
        };
        let err = run(&a, &e.roots, &e.reads, &e.spill).unwrap_err();
        match err {
            ToolError::Guard(m) => assert!(m.contains("binary") && m.contains("bytes")),
            other => panic!("expected Guard, got {other:?}"),
        }
    }

    #[test]
    fn long_line_truncated() {
        let e = env();
        fs::write(
            e.roots.roots()[0].join("l.txt"),
            format!("{}\nshort\n", "x".repeat(3000)),
        )
        .unwrap();
        let a = Args {
            path: "l.txt".into(),
            offset: None,
            limit: None,
        };
        let out = run(&a, &e.roots, &e.reads, &e.spill).unwrap();
        assert!(out.text.contains("line truncated at 2000 chars"));
        assert!(out.text.contains("2: short"));
    }

    #[test]
    fn read_regardless_escapes_via_rootset() {
        let e = env();
        let a = Args {
            path: "../nope".into(),
            offset: None,
            limit: None,
        };
        assert!(matches!(
            run(&a, &e.roots, &e.reads, &e.spill),
            Err(ToolError::Escape(_))
        ));
    }

    #[test]
    fn overflow_spills() {
        let e = env();
        fs::write(
            e.roots.roots()[0].join("big.txt"),
            (1..=6000)
                .map(|i| format!("line {i}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let a = Args {
            path: "big.txt".into(),
            offset: None,
            limit: None,
        };
        let out = run(&a, &e.roots, &e.reads, &e.spill).unwrap();
        assert!(out.text.contains("stopped at limit 2000"));
        assert!(!out.truncated()); // paging marker, not spill
    }
}

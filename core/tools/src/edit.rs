//! `write` + `edit` tools.
//!
//! Edit match tiers, in order of preference: exact; indentation-flexible
//! (per-line content equal after leading whitespace, constant indent delta);
//! line-trimmed; whitespace-normalized (per line); block-anchor (first/last
//! line anchors trim-exact + ≥60% interior similarity, unique candidate
//! required — strictly-best score wins, ties fail). Note indentation-
//! flexible precedes line-trimmed: every one of its hits is also a
//! line-trimmed hit, but the constant-delta constraint is the more precise
//! signal. The landing tier is named in the tool result (`[exact]`, …).
//!
//! Line endings (LF/CRLF) and BOM are preserved verbatim.

use std::path::Path;

use serde::Deserialize;

use crate::ReadCache;
use crate::error::{Result, ToolError};
use crate::paths::RootSet;

// ---------- shared text plumbing ----------

pub(crate) struct FileText {
    pub bom: bool,
    pub crlf: bool,
    pub text: String, // LF-normalized working copy
}

pub(crate) fn load(path: &Path) -> Result<FileText> {
    let bytes = std::fs::read(path)?;
    let (bom, body) = match bytes.strip_prefix(b"\xEF\xBB\xBF") {
        Some(rest) => (true, rest),
        None => (false, &bytes[..]),
    };
    let raw = String::from_utf8_lossy(body);
    let crlf_count = raw.matches("\r\n").count();
    let lf_only = raw.matches('\n').count().saturating_sub(crlf_count);
    let crlf = crlf_count > lf_only;
    let text = raw.replace("\r\n", "\n");
    Ok(FileText { bom, crlf, text })
}

pub(crate) fn encode(ft: &FileText) -> Vec<u8> {
    let body = if ft.crlf {
        ft.text.replace('\n', "\r\n")
    } else {
        ft.text.clone()
    };
    let mut out = Vec::with_capacity(body.len() + 3);
    if ft.bom {
        out.extend_from_slice(b"\xEF\xBB\xBF");
    }
    out.extend_from_slice(body.as_bytes());
    out
}

/// Atomic-ish write: temp file in the same directory, then rename. Parents
/// created; existing file's permission bits preserved on unix.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    let old_mode = std::fs::metadata(path).ok().map(|m| {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode()
    });
    let mut tmp = path.to_path_buf();
    tmp.set_extension(format!(
        "palmcode-tmp-{}-{}",
        std::process::id(),
        blake3::hash(bytes)
            .to_hex()
            .get(..8)
            .map(str::to_owned)
            .unwrap_or_default()
    ));
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    if let Some(mode) = old_mode {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode));
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ---------- matchers ----------

/// Which fallback tier produced the match (recorded for edit-bench stats).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Exact,
    LineTrimmed,
    WhitespaceNormalized,
    IndentFlexible,
    BlockAnchor,
}

/// A hit: byte range in the LF-normalized text + first line number (1-based).
#[derive(Debug, Clone)]
pub struct Hit {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut v = vec![0];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            v.push(i + 1);
        }
    }
    v
}

fn hit_at(text: &str, starts: &[usize], start_line: usize, end_line: usize) -> Hit {
    let start_byte = starts[start_line - 1];
    // Range stops BEFORE the trailing newline of the last matched line, so
    // splicing a replacement never eats the line separator.
    let end_byte = if end_line < starts.len() {
        starts[end_line].saturating_sub(1)
    } else {
        text.len()
    };
    Hit {
        start_byte,
        end_byte,
        start_line,
    }
}

fn old_lines(old: &str) -> Vec<&str> {
    old.trim_end_matches('\n').split('\n').collect()
}

/// All hits at a given tier by comparing the old block against each
/// same-length line window with a per-line predicate.
fn window_hits(
    text: &str,
    old: &str,
    tier: Tier,
    pred: impl Fn(&[&str], &[&str]) -> bool,
) -> Vec<Hit> {
    let starts = line_starts(text);
    let lines: Vec<&str> = text.lines().collect();
    let ol = old_lines(old);
    if ol.is_empty() || lines.len() < ol.len() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for i in 0..=lines.len() - ol.len() {
        if pred(&lines[i..i + ol.len()], &ol) {
            hits.push(hit_at(text, &starts, i + 1, i + ol.len()));
        }
    }
    let _ = tier;
    hits
}

fn exact_hits(text: &str, old: &str) -> Vec<Hit> {
    if old.is_empty() {
        return Vec::new();
    }
    let starts = line_starts(text);
    let mut hits = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find(old) {
        let start = from + i;
        let end = start + old.len();
        let start_line = starts.partition_point(|&p| p <= start);
        hits.push(Hit {
            start_byte: start,
            end_byte: end,
            start_line,
        });
        from = start + 1;
    }
    hits
}

pub(crate) fn find(text: &str, old: &str) -> (Option<Tier>, Vec<Hit>) {
    let hits = exact_hits(text, old);
    if !hits.is_empty() {
        return (Some(Tier::Exact), hits);
    }
    for tier in [
        Tier::IndentFlexible,
        Tier::LineTrimmed,
        Tier::WhitespaceNormalized,
        Tier::BlockAnchor,
    ] {
        let hits = match tier {
            Tier::LineTrimmed => window_hits(text, old, tier, |win, ol| {
                win.iter().zip(ol).all(|(a, b)| a.trim() == b.trim())
            }),
            Tier::WhitespaceNormalized => window_hits(text, old, tier, |win, ol| {
                win.iter().zip(ol).all(|(a, b)| {
                    a.split_whitespace().collect::<String>()
                        == b.split_whitespace().collect::<String>()
                })
            }),
            Tier::IndentFlexible => window_hits(text, old, tier, |win, ol| {
                // constant indent delta per block, non-empty lines only
                let mut delta: Option<isize> = None;
                for (a, b) in win.iter().zip(ol) {
                    if b.trim().is_empty() && a.trim().is_empty() {
                        continue;
                    }
                    let da = a.len() - a.trim_start().len();
                    let db = b.len() - b.trim_start().len();
                    if a.trim_start() != b.trim_start() {
                        return false;
                    }
                    match delta {
                        None => delta = Some(da as isize - db as isize),
                        Some(d) if d != da as isize - db as isize => return false,
                        _ => {}
                    }
                }
                true
            }),
            Tier::BlockAnchor => block_anchor_hits(text, old),
            Tier::Exact => unreachable!(),
        };
        if !hits.is_empty() {
            return (Some(tier), hits);
        }
    }
    (None, Vec::new())
}

/// Block-anchor tier: first+last lines anchor (trim-exact); the interior
/// only needs a SIMILARITY — ≥ 60% of the old block's non-blank interior
/// lines match whitespace-normalized. The match must be UNAMBIGUOUS: one
/// candidate, or a strictly-best score over all others (ties fail, the
/// model must supply more context — fuzzy multi-hits guess wrong too
/// often to be allowed silently).
fn block_anchor_hits(text: &str, old: &str) -> Vec<Hit> {
    const THRESHOLD: f64 = 0.6;
    let starts = line_starts(text);
    let lines: Vec<&str> = text.lines().collect();
    let ol = old_lines(old);
    if ol.is_empty() || lines.len() < ol.len() {
        return Vec::new();
    }
    let norm = |s: &str| -> String { s.split_whitespace().collect::<Vec<_>>().join(" ") };
    let interior_blank = |i: usize| ol[i].trim().is_empty();
    let interior_nonblank = (1..ol.len().saturating_sub(1))
        .filter(|&i| !interior_blank(i))
        .count();

    let mut scored: Vec<(f64, Hit)> = Vec::new();
    for i in 0..=lines.len() - ol.len() {
        let win = &lines[i..i + ol.len()];
        if win[0].trim() != ol[0].trim() || win[ol.len() - 1].trim() != ol[ol.len() - 1].trim() {
            continue;
        }
        if interior_nonblank == 0 {
            scored.push((1.0, hit_at(text, &starts, i + 1, i + ol.len())));
            continue;
        }
        let matched = (1..ol.len() - 1)
            .filter(|&j| !interior_blank(j) && norm(win[j]) == norm(ol[j]))
            .count();
        let score = matched as f64 / interior_nonblank as f64;
        if score >= THRESHOLD {
            scored.push((score, hit_at(text, &starts, i + 1, i + ol.len())));
        }
    }
    match scored.len() {
        0 => Vec::new(),
        1 => scored.into_iter().map(|(_, h)| h).collect(),
        _ => {
            // strictly-best wins; any tie at the top = ambiguous → no hit
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            if scored[0].0 > scored[1].0 {
                vec![scored.into_iter().next().unwrap().1]
            } else {
                Vec::new()
            }
        }
    }
}

/// Closest match: window with the most per-line (trimmed) equality.
fn closest(text: &str, old: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let ol = old_lines(old);
    if ol.is_empty() || lines.len() < ol.len() {
        return None;
    }
    let mut best = (0usize, 0usize);
    for i in 0..=lines.len() - ol.len() {
        let score = lines[i..i + ol.len()]
            .iter()
            .zip(&ol)
            .filter(|(a, b)| !b.trim().is_empty() && a.trim() == b.trim())
            .count();
        if score > best.1 {
            best = (i, score);
        }
    }
    (best.1 > 0).then_some((best.0 + 1, best.0 + ol.len()))
}

// ---------- write tool ----------

#[derive(Deserialize)]
pub struct WriteArgs {
    pub path: String,
    pub content: String,
}

pub fn run_write(args: &WriteArgs, roots: &RootSet, reads: &ReadCache) -> Result<String> {
    let path = roots.resolve(&args.path, None)?;
    if path.is_dir() {
        return Err(ToolError::BadPath(format!(
            "path is a directory: {}",
            path.display()
        )));
    }
    reads.check_writable(&path)?;
    // Preserve an existing file's EOL style when rewriting it fully.
    let (bom, crlf) = if path.exists() {
        let ft = load(&path)?;
        (ft.bom, ft.crlf)
    } else {
        (false, false)
    };
    let ft = FileText {
        bom,
        crlf,
        text: args.content.replace("\r\n", "\n"),
    };
    let bytes = encode(&ft);
    atomic_write(&path, &bytes)?;
    reads.record_read(&path)?;
    Ok(format!(
        "wrote {} bytes ({} lines) to {}",
        bytes.len(),
        ft.text.lines().count(),
        path.display()
    ))
}

pub fn write_schema() -> serde_json::Value {
    serde_json::json!({
        "name": "write",
        "description": "Create or fully overwrite a file (atomic). Read it first this session if it exists.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        }
    })
}

// ---------- edit tool ----------

#[derive(Deserialize)]
pub struct EditArgs {
    pub path: String,
    #[serde(rename = "oldString")]
    pub old_string: String,
    #[serde(rename = "newString")]
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug)]
pub struct EditOutcome {
    pub summary: String,
    pub tier_used: Tier,
    pub replacements: usize,
}

/// Shared splicer: apply `old -> new` to in-memory text, with the tier
/// ladder, uniqueness rule, and colon-free error text (the caller prefixes
/// the file path). Returns (new_text, tier, replacements).
pub(crate) fn replace_in_text(
    text: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> std::result::Result<(String, Tier, usize), String> {
    let (tier, hits) = find(text, old);
    let tier = match tier {
        Some(t) if !hits.is_empty() => t,
        _ => {
            let hint = match closest(text, old) {
                Some((a, b)) => {
                    let lines: Vec<&str> = text.lines().collect();
                    let snippet: String = lines[a - 1..b.min(lines.len())]
                        .iter()
                        .take(8)
                        .map(|l| format!("  {l}\n"))
                        .collect();
                    format!("; closest match at lines {a}-{b}:\n{snippet}")
                }
                None => String::new(),
            };
            return Err(format!("oldString not found{hint}"));
        }
    };
    if hits.len() > 1 && !replace_all {
        let lines: Vec<String> = hits.iter().map(|h| h.start_line.to_string()).collect();
        return Err(format!(
            "found {} matches at lines {}; add context to make oldString unique, \
             or set replaceAll=true",
            hits.len(),
            lines.join(", ")
        ));
    }
    let mut hits = hits;
    if !replace_all {
        hits.truncate(1);
    }
    let replacements = hits.len();
    let first_line = hits.last().map(|h| h.start_line).unwrap_or(0);
    let mut out = text.to_string();
    // right-to-left so byte offsets stay valid
    for h in hits.iter().rev() {
        out.replace_range(h.start_byte..h.end_byte, new);
    }
    let _ = first_line;
    Ok((out, tier, replacements))
}

pub fn run_edit(args: &EditArgs, roots: &RootSet, reads: &ReadCache) -> Result<EditOutcome> {
    if args.old_string.is_empty() {
        return Err(ToolError::Match("oldString must be non-empty".into()));
    }
    if args.old_string == args.new_string {
        return Err(ToolError::Match(
            "oldString and newString are identical; nothing to do".into(),
        ));
    }
    let path = roots.resolve(&args.path, None)?;
    reads.check_writable(&path)?;
    let mut ft = load(&path)?;

    let (new_text, tier, replacements) = replace_in_text(
        &ft.text,
        &args.old_string,
        &args.new_string,
        args.replace_all,
    )
    .map_err(|m| ToolError::Match(format!("{}: {m}", path.display())))?;
    ft.text = new_text;
    let bytes = encode(&ft);
    atomic_write(&path, &bytes)?;
    reads.record_read(&path)?;
    Ok(EditOutcome {
        // tier first: event previews truncate long paths, and the tier must
        // survive into edit-bench traces
        summary: format!(
            "edited [{}] {}: {} replacement(s)",
            tier_name(tier),
            path.display(),
            replacements,
        ),
        tier_used: tier,
        replacements,
    })
}

fn tier_name(t: Tier) -> &'static str {
    match t {
        Tier::Exact => "exact",
        Tier::LineTrimmed => "line-trimmed",
        Tier::WhitespaceNormalized => "whitespace-normalized",
        Tier::IndentFlexible => "indentation-flexible",
        Tier::BlockAnchor => "block-anchor",
    }
}

pub fn edit_schema() -> serde_json::Value {
    serde_json::json!({
        "name": "edit",
        "description": "Replace oldString with newString. Must match uniquely (or replaceAll). Whitespace-tolerant fallbacks; read the file first.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "oldString": { "type": "string" },
                "newString": { "type": "string" },
                "replaceAll": { "type": "boolean" }
            },
            "required": ["path", "oldString", "newString"],
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

    fn setup_file(
        roots: &RootSet,
        reads: &ReadCache,
        name: &str,
        bytes: &[u8],
    ) -> std::path::PathBuf {
        let p = roots.roots()[0].join(name);
        if let Some(par) = p.parent() {
            fs::create_dir_all(par).unwrap();
        }
        fs::write(&p, bytes).unwrap();
        reads.record_read(&p).unwrap();
        p
    }

    fn edit(
        roots: &RootSet,
        reads: &ReadCache,
        path: &str,
        old: &str,
        new: &str,
    ) -> Result<EditOutcome> {
        run_edit(
            &EditArgs {
                path: path.into(),
                old_string: old.into(),
                new_string: new.into(),
                replace_all: false,
            },
            roots,
            reads,
        )
    }

    #[test]
    fn golden_exact() {
        let (_d, roots, reads) = env();
        setup_file(&roots, &reads, "a.rs", b"fn main() {\n  hello();\n}\n");
        let out = edit(&roots, &reads, "a.rs", "  hello();", "  world();").unwrap();
        assert_eq!(out.tier_used, Tier::Exact);
        assert_eq!(
            fs::read_to_string(roots.roots()[0].join("a.rs")).unwrap(),
            "fn main() {\n  world();\n}\n"
        );
    }

    #[test]
    fn golden_crlf_preserved() {
        let (_d, roots, reads) = env();
        setup_file(&roots, &reads, "w.c", b"int a;\r\nint b;\r\nint c;\r\n");
        // model sends LF-only snippet; file is CRLF
        edit(&roots, &reads, "w.c", "int b;", "int B;").unwrap();
        let bytes = fs::read(roots.roots()[0].join("w.c")).unwrap();
        assert_eq!(bytes, b"int a;\r\nint B;\r\nint c;\r\n");
    }

    #[test]
    fn golden_bom_and_crlf_preserved() {
        let (_d, roots, reads) = env();
        setup_file(&roots, &reads, "u.txt", b"\xEF\xBB\xBFa\r\nb\r\n");
        edit(&roots, &reads, "u.txt", "b", "c").unwrap();
        assert_eq!(
            fs::read(roots.roots()[0].join("u.txt")).unwrap(),
            b"\xEF\xBB\xBFa\r\nc\r\n"
        );
    }

    #[test]
    fn golden_tabs_vs_spaces_fallback() {
        let (_d, roots, reads) = env();
        setup_file(&roots, &reads, "t.rs", b"fn f() {\n\tlet x = 1;\n}\n");
        let out = edit(
            &roots,
            &reads,
            "t.rs",
            "fn f() {\n    let x = 1;\n}",
            "fn f() {\n\tlet x = 2;\n}",
        )
        .unwrap();
        assert_eq!(out.tier_used, Tier::LineTrimmed);
        assert!(
            fs::read_to_string(roots.roots()[0].join("t.rs"))
                .unwrap()
                .contains("let x = 2;")
        );
    }

    #[test]
    fn golden_internal_ws_normalized_hits() {
        let (_d, roots, reads) = env();
        setup_file(&roots, &reads, "n.c", b"int   compute(int  x)\n{\n}\n");
        // two-line context avoids the trim-equality of the single line
        let out = edit(
            &roots,
            &reads,
            "n.c",
            "int compute(int x)\n{",
            "int compute2(int x)\n{",
        )
        .unwrap();
        assert_eq!(out.tier_used, Tier::WhitespaceNormalized);
    }

    #[test]
    fn golden_indent_flexible_constant_delta() {
        let (_d, roots, reads) = env();
        setup_file(
            &roots,
            &reads,
            "i.rs",
            b"mod m {\n  fn f() {\n    body();\n  }\n}\n",
        );
        let out = edit(
            &roots,
            &reads,
            "i.rs",
            "fn f() {\n  body();\n}",
            "fn g() {\n  other();\n}",
        )
        .unwrap();
        assert_eq!(out.tier_used, Tier::IndentFlexible);
        assert!(
            fs::read_to_string(roots.roots()[0].join("i.rs"))
                .unwrap()
                .contains("fn g()")
        );
    }

    #[test]
    fn golden_block_anchor_fuzzy_unique() {
        let (_d, roots, reads) = env();
        setup_file(
            &roots,
            &reads,
            "b.rs",
            b"fn alpha() {\n    one();\n    two();\n    three();\n    four();\n}\n",
        );
        // anchors trim-exact; interior = one/XXX/three/four vs one/two/three/four
        // → 3/4 matched = 75% ≥ 60% threshold → hit
        let out = edit(
            &roots,
            &reads,
            "b.rs",
            "fn alpha() {\n    one();\n    XXX();\n    three();\n    four();\n}",
            "fn alpha() {\n    changed();\n}",
        );
        assert!(out.is_ok(), "3/4=75% interior must hit: {out:?}");
        assert_eq!(out.unwrap().tier_used, Tier::BlockAnchor);
    }

    #[test]
    fn block_anchor_below_threshold_and_ties_fail() {
        let (_d, roots, reads) = env();
        setup_file(
            &roots,
            &reads,
            "b2.rs",
            b"fn alpha() {\n    one();\n    two();\n    three();\n    four();\n}\n",
        );
        // interior one/XXX/YYY/four vs one/two/three/four → 2/4 = 50% < 60%
        let err = edit(
            &roots,
            &reads,
            "b2.rs",
            "fn alpha() {\n    one();\n    XXX();\n    YYY();\n    four();\n}",
            "fn alpha() {\n    changed();\n}",
        );
        assert!(err.is_err(), "50% interior must not hit");
        // BOTH candidates above threshold with equal scores → ambiguous, fail
        setup_file(
            &roots,
            &reads,
            "b3.rs",
            b"fn a() {\n    keep1();\n    keep2();\n    keep3();\n    z();\n}\nfn a() {\n    keep1();\n    keep2();\n    keep3();\n    z();\n}\n",
        );
        let err2 = edit(
            &roots,
            &reads,
            "b3.rs",
            "fn a() {\n    keep1();\n    keep2();\n    keep3();\n    CHANGED();\n}",
            "fn a() {\n    replaced();\n}",
        );
        assert!(err2.is_err(), "tied block-anchor candidates must fail");
    }

    #[test]
    fn golden_repeated_block_needs_context() {
        let (_d, roots, reads) = env();
        setup_file(
            &roots,
            &reads,
            "r.rs",
            b"    call();\nfoo\n    call();\nfoo\n",
        );
        let err = edit(&roots, &reads, "r.rs", "    call();", "    done();").unwrap_err();
        match err {
            ToolError::Match(m) => {
                assert!(m.contains("found 2 matches"), "{m}");
                assert!(m.contains("lines 1, 3"), "{m}");
            }
            _ => panic!("{err:?}"),
        }
    }

    #[test]
    fn golden_replace_all() {
        let (_d, roots, reads) = env();
        setup_file(&roots, &reads, "r.rs", b"x=1;\nx=1;\ny\n");
        let out = run_edit(
            &EditArgs {
                path: "r.rs".into(),
                old_string: "x=1;".into(),
                new_string: "z=2;".into(),
                replace_all: true,
            },
            &roots,
            &reads,
        )
        .unwrap();
        assert_eq!(out.replacements, 2);
        assert_eq!(
            fs::read_to_string(roots.roots()[0].join("r.rs")).unwrap(),
            "z=2;\nz=2;\ny\n"
        );
    }

    #[test]
    fn golden_not_found_offers_closest() {
        let (_d, roots, reads) = env();
        setup_file(&roots, &reads, "c.rs", b"fn alpha() {\n    work();\n}\n");
        let err = edit(&roots, &reads, "c.rs", "fn beta() {\n    work();\n}", "x").unwrap_err();
        match err {
            ToolError::Match(m) => {
                assert!(m.contains("not found"), "{m}");
                assert!(m.contains("closest match at lines 1-3"), "{m}");
                assert!(m.contains("work();"), "{m}");
            }
            _ => panic!("{err:?}"),
        }
    }

    #[test]
    fn golden_identical_strings_rejected() {
        let (_d, roots, reads) = env();
        setup_file(&roots, &reads, "s.rs", b"a\n");
        assert!(edit(&roots, &reads, "s.rs", "a", "a").is_err());
    }

    #[test]
    fn golden_stale_guard_blocks_edit() {
        let (_d, roots, reads) = env();
        let p = setup_file(&roots, &reads, "g.rs", b"a\n");
        fs::write(&p, b"b\n").unwrap(); // external change
        let err = edit(&roots, &reads, "g.rs", "b", "c").unwrap_err();
        assert!(matches!(err, ToolError::Stale(_)));
    }

    #[test]
    fn write_creates_parents_and_records() {
        let (_d, roots, reads) = env();
        let out = run_write(
            &WriteArgs {
                path: "deep/nested/new.txt".into(),
                content: "hello\n".into(),
            },
            &roots,
            &reads,
        )
        .unwrap();
        assert!(out.contains("wrote 6 bytes"));
        assert_eq!(
            fs::read_to_string(roots.roots()[0].join("deep/nested/new.txt")).unwrap(),
            "hello\n"
        );
        // immediate edit allowed without a separate read (own write records)
        edit(&roots, &reads, "deep/nested/new.txt", "hello", "bye").unwrap();
    }

    #[test]
    fn write_overwrite_requires_read() {
        let (_d, roots, reads) = env();
        let p = roots.roots()[0].join("e.txt");
        fs::write(&p, "data").unwrap();
        let missing_read = run_write(
            &WriteArgs {
                path: "e.txt".into(),
                content: "x".into(),
            },
            &roots,
            &reads,
        );
        assert!(matches!(missing_read, Err(ToolError::Stale(_))));
        reads.record_read(&p).unwrap();
        run_write(
            &WriteArgs {
                path: "e.txt".into(),
                content: "x\r\n".into(),
            },
            &roots,
            &reads,
        )
        .unwrap();
        // CRLF in model content normalized to the file's existing LF style
        assert_eq!(fs::read_to_string(&p).unwrap(), "x\n");
    }
}

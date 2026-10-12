//! Unified diff for tool cards (deliverable H): trims the common
//! prefix/suffix and emits ONE hunk with context — small agent edits are
//! single-region in practice.
//! shortcut: multi-region changes render as one big hunk; upgrade to Myers
//! (similar crate) when cards need surgical multiple hunks.

/// Unified diff between two texts; empty string when identical.
pub fn unified_diff(path: &str, old: &str, new: &str, context: usize) -> String {
    if old == new {
        return String::new();
    }
    let old_l: Vec<&str> = old.lines().collect();
    let new_l: Vec<&str> = new.lines().collect();
    let mut start = 0usize;
    while start < old_l.len().min(new_l.len()) && old_l[start] == new_l[start] {
        start += 1;
    }
    let mut end_old = old_l.len();
    let mut end_new = new_l.len();
    while end_old > start && end_new > start && old_l[end_old - 1] == new_l[end_new - 1] {
        end_old -= 1;
        end_new -= 1;
    }
    let ctx_from = start.saturating_sub(context);
    let mut out = format!("--- a/{path}\n+++ b/{path}\n@@\n");
    for l in &old_l[ctx_from..start] {
        out.push_str(&format!(" {l}\n"));
    }
    for l in &old_l[start..end_old] {
        out.push_str(&format!("-{l}\n"));
    }
    for l in &new_l[start..end_new] {
        out.push_str(&format!("+{l}\n"));
    }
    let suf_old_end = (end_old + context).min(old_l.len());
    for l in &old_l[end_old..suf_old_end] {
        out.push_str(&format!(" {l}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_is_empty() {
        assert_eq!(unified_diff("a.rs", "x\n", "x\n", 3), "");
    }

    #[test]
    fn single_region_hunk_with_context() {
        let old =
            "fn a() {\n    one();\n    old_call();\n    three();\n    four();\n    five();\n}\n";
        let new =
            "fn a() {\n    one();\n    new_call();\n    three();\n    four();\n    five();\n}\n";
        let d = unified_diff("a.rs", old, new, 1);
        assert!(d.starts_with("--- a/a.rs\n+++ b/a.rs\n@@\n"));
        // context prefix " " + source line; +/- lines keep their own indent
        assert!(
            d.contains("     one();\n-    old_call();\n+    new_call();\n     three();\n"),
            "{d}"
        );
        assert!(!d.contains("five();"), "only 1 context line: {d}");
    }

    #[test]
    fn append_only() {
        let d = unified_diff("f.txt", "a\n", "a\nb\n", 0);
        assert_eq!(d, "--- a/f.txt\n+++ b/f.txt\n@@\n+b\n");
    }

    #[test]
    fn delete_only() {
        let d = unified_diff("f.txt", "a\nb\n", "a\n", 0);
        assert_eq!(d, "--- a/f.txt\n+++ b/f.txt\n@@\n-b\n");
    }
}

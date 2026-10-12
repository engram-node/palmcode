//! Post-edit syntax sanity check (deliverable C): after write/edit/patch the
//! touched file is parsed with a tree-sitter grammar (9 languages). Errors
//! are REPORTED as a one-line warning appended to the tool result — never
//! fail an edit: the model gets the hint and can repair in the next call.

use std::sync::Mutex;

use tree_sitter::{Language, Parser};

use crate::paths::RootSet;

/// 2 MiB: grammars are fast, but pathological minified blobs are not worth
/// the CPU (and their errors would not be actionable anyway).
const MAX_CHECK_BYTES: u64 = 2 * 1024 * 1024;

fn language_for(path: &str) -> Option<(&'static str, Language)> {
    let ext = path.rsplit('.').next().unwrap_or("");
    Some(match ext {
        "rs" => ("rust", tree_sitter_rust::LANGUAGE.into()),
        "py" | "pyi" => ("python", tree_sitter_python::LANGUAGE.into()),
        "js" | "mjs" | "cjs" | "jsx" => ("javascript", tree_sitter_javascript::LANGUAGE.into()),
        "ts" => (
            "typescript",
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        ),
        "tsx" => ("tsx", tree_sitter_typescript::LANGUAGE_TSX.into()),
        "json" => ("json", tree_sitter_json::LANGUAGE.into()),
        "c" | "h" => ("c", tree_sitter_c::LANGUAGE.into()),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => ("cpp", tree_sitter_cpp::LANGUAGE.into()),
        "go" => ("go", tree_sitter_go::LANGUAGE.into()),
        "java" => ("java", tree_sitter_java::LANGUAGE.into()),
        _ => return None,
    })
}

static PARSER: Mutex<Option<Parser>> = Mutex::new(None);

/// Parse `content` as the language of `path`; Some(warning) iff the tree
/// has an error. Unknown extensions -> None (silent, by design).
pub fn check_text(path: &str, content: &str) -> Option<String> {
    if content.len() as u64 > MAX_CHECK_BYTES {
        return None;
    }
    let (lang_name, language) = language_for(path)?;
    let mut guard = PARSER.lock().ok()?;
    let parser = guard.get_or_insert_with(Parser::new);
    parser.set_language(&language).ok()?;
    let tree = parser.parse(content, None)?;
    if !tree.root_node().has_error() {
        return None;
    }
    // first error node, depth-first
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            let start = node.start_position();
            return Some(format!(
                "syntax: {lang_name} reports {} at {}:{} - the edit likely broke the file",
                if node.is_missing() {
                    "a missing node"
                } else {
                    "an error"
                },
                start.row + 1,
                start.column + 1,
            ));
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.has_error() || child.is_error() || child.is_missing() {
                stack.push(child);
            }
        }
    }
    Some(format!("syntax: {lang_name} reports parse errors"))
}

/// Rooted variant: read the file fresh from disk (post-write view).
pub fn check_rooted(roots: &RootSet, rel: &str) -> Option<String> {
    let abs = roots.resolve(rel, None).ok()?;
    let meta = std::fs::metadata(&abs).ok()?;
    if !meta.is_file() || meta.len() > MAX_CHECK_BYTES {
        return None;
    }
    let content = std::fs::read_to_string(&abs).ok()?;
    check_text(rel, &content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_clean(path: &str, code: &str) {
        assert_eq!(check_text(path, code), None, "{path} should parse clean");
    }

    fn assert_err(path: &str, code: &str) {
        let w = check_text(path, code).unwrap_or_else(|| panic!("{path} should flag errors"));
        assert!(w.contains("syntax:"), "{w}");
    }

    #[test]
    fn nine_grammars_flag_broken_and_pass_clean() {
        assert_clean("a.rs", "fn main() { println!(\"hi\"); }\n");
        assert_err("a.rs", "fn main( { \n");

        assert_clean("a.py", "def f():\n    return 1\n");
        assert_err("a.py", "def f(:\n    return\n");

        assert_clean("a.js", "const f = (x) => x + 1;\n");
        assert_err("a.js", "const f = (x) => {\n");

        assert_clean("a.ts", "const f: (x: number) => number = (x) => x;\n");
        assert_err("a.ts", "const f: (x: number) => = (x) => x;\n");

        assert_clean("a.tsx", "export const C = () => <div>x</div>;\n");
        assert_err("a.tsx", "export const C = () => <div>;\n");

        assert_clean("a.json", "{\"a\": [1, 2, {\"b\": true}]}\n");
        assert_err("a.json", "{\"a\": [1, 2,}\n");

        assert_clean("a.c", "int main(void) { return 0; }\n");
        assert_err("a.c", "int main(void { return 0 }\n");

        assert_clean("a.cpp", "template <class T> T f(T v) { return v; }\n");
        assert_err("a.cpp", "template <class T> f(T v) { return }\n");

        assert_clean("a.go", "package main\nfunc main() {}\n");
        assert_err("a.go", "package main\nfunc main( {}\n");

        assert_clean("a.java", "class A { void f() {} }\n");
        assert_err("a.java", "class A { void f( {} }\n");
    }

    #[test]
    fn unknown_extension_is_silent() {
        assert_eq!(check_text("a.kt", "not even kotlin-ish ((("), None);
        assert_eq!(check_text("Makefile", "(((("), None);
    }

    #[test]
    fn oversized_file_skipped() {
        let big = "fn f() {}\n".repeat(300_000); // ~3.3 MB
        assert_eq!(check_text("a.rs", &big), None);
    }
}

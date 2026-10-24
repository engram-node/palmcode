//! Bash command classifier (deliverable F): parse one `sh -c` script and
//! decide Allow (provably read-only), Ask (mutating/unparseable/unknown) or
//! Deny (escapes the workspace). Hand-rolled conservative scanner -- any
//! construct not positively understood is Ask, never Allow. Command
//! substitutions (`$(...)`, backticks), subshells, process substitutions and
//! unquoted here-doc bodies are recursed into and classified as scripts.
//!
//! Invariant: an Allow verdict means no expansion, redirection, subshell or
//! command name in the script can write anything or leave the workspace.

use std::path::{Path, PathBuf};

/// Ask/Deny carry a model- and user-readable reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Allow,
    Ask(String),
    Deny(String),
}

/// Paths a script may mention absolutely: the workspace plus scratch space.
/// (/dev/null-style literals are handled separately.)
pub struct ClassCtx {
    pub workspace: PathBuf,
    pub extra_roots: Vec<PathBuf>,
}

impl ClassCtx {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            extra_roots: Vec::new(),
        }
    }
}

/// Commands that cannot write and cannot run other commands.
const READ_ONLY: &[&str] = &[
    "basename",
    "cat",
    "cut",
    "date",
    "dd",
    "df",
    "diff",
    "dirname",
    "du",
    "echo",
    "false",
    "file",
    "fgrep",
    "grep",
    "egrep",
    "head",
    "id",
    "ls",
    "md5sum",
    "nproc",
    "printenv",
    "printf",
    "pwd",
    "readlink",
    "realpath",
    "rg",
    "seq",
    "sha1sum",
    "sha256sum",
    "sha512sum",
    "sleep",
    "sort",
    "stat",
    "tail",
    "tr",
    "true",
    "uname",
    "uniq",
    "wc",
    "which",
    "whoami",
];

/// Network-capable: never Allow.
const NETWORK: &[&str] = &[
    "curl", "wget", "ftp", "ifconfig", "ip", "nc", "ncat", "netcat", "nmap", "ping", "rsync",
    "scp", "sftp", "ssh", "telnet",
];

/// Interpreters/build tools: whatever they run is opaque -> Ask.
const INTERPRETERS: &[&str] = &[
    "awk", "busybox", "cmake", "dash", "expect", "irb", "java", "javac", "lua", "make", "node",
    "perl", "php", "python", "python3", "ruby", "sed", "sqlite3",
];

/// Wrappers whose first real argument is itself a command to classify.
const WRAPPERS: &[&str] = &["env", "command", "nice", "nohup", "time", "timeout"];

fn rank(o: &Outcome) -> u8 {
    match o {
        Outcome::Allow => 1,
        Outcome::Ask(_) => 2,
        Outcome::Deny(_) => 3,
    }
}

fn max(a: Outcome, b: Outcome) -> Outcome {
    if rank(&a) >= rank(&b) { a } else { b }
}

/// Upgrade `v` in place if `other` is stricter (no clone of payloads).
fn absorb(v: &mut Outcome, other: Outcome) {
    if rank(&other) > rank(v) {
        *v = other;
    }
}

pub fn classify(cmd: &str, ctx: &ClassCtx) -> Outcome {
    match scan(cmd, ctx) {
        Ok(v) => v,
        Err(reason) => Outcome::Ask(format!("unparseable command: {reason}")),
    }
}

// ---------------------------------------------------------------------------
// One-pass scanner over a script body
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Word {
    lit: String,
    dynamic: bool,
    done: bool,
}

impl Word {
    fn is_assignment(&self) -> bool {
        if self.dynamic {
            return false;
        }
        let Some(eq) = self.lit.find('=') else {
            return false;
        };
        eq > 0
            && self.lit[..eq]
                .chars()
                .enumerate()
                .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
    }
}

fn is_meta(c: u8) -> bool {
    matches!(
        c,
        b' ' | b'\t'
            | b'\r'
            | b'\n'
            | b'\''
            | b'"'
            | b'`'
            | b'\\'
            | b'$'
            | b';'
            | b'&'
            | b'|'
            | b'('
            | b')'
            | b'<'
            | b'>'
            | b'{'
            | b'}'
    )
}

fn push_word(words: &mut Vec<Word>, lit: &str, dynamic: bool) {
    if words.last().map(|w| w.done).unwrap_or(true) {
        words.push(Word::default());
    }
    let cur = words.last_mut().unwrap();
    if dynamic {
        cur.dynamic = true;
    } else {
        cur.lit.push_str(lit);
    }
}

fn dyn_word(words: &mut Vec<Word>) {
    if words.last().map(|w| w.done).unwrap_or(true) {
        words.push(Word::default());
    }
    words.last_mut().unwrap().dynamic = true;
}

fn end_word(words: &mut [Word]) {
    if let Some(w) = words.last_mut() {
        w.done = true;
    }
}

#[allow(unused_assignments)] // flush_cmd!'s trailing at_word_start set is unread at EOF
fn scan(script: &str, ctx: &ClassCtx) -> Result<Outcome, String> {
    let b = script.as_bytes();
    let mut i = 0usize;
    let mut verdict = Outcome::Allow;
    let mut words: Vec<Word> = Vec::new();
    let mut heredocs: Vec<(String, bool)> = Vec::new(); // (delimiter, expand)
    let mut at_word_start = true;

    macro_rules! flush_cmd {
        () => {
            end_word(&mut words);
            verdict = max(verdict, classify_command(&words, ctx)?);
            words.clear();
            at_word_start = true;
        };
    }

    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' | b'\r' => {
                end_word(&mut words);
                at_word_start = true;
                i += 1;
            }
            b'\n' => {
                flush_cmd!();
                i += 1;
                for (delim, expand) in std::mem::take(&mut heredocs) {
                    i = consume_heredoc(b, i, &delim, expand, ctx, &mut verdict)?;
                }
            }
            b'#' if at_word_start => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'\'' => {
                let end = find_quote(b, i + 1, b'\'')?;
                push_word(&mut words, &script[i + 1..end], false);
                at_word_start = false;
                i = end + 1;
            }
            b'"' => {
                let end = find_dquote(b, i, ctx, &mut verdict)?;
                let dynamic = has_dynamic(&script[i + 1..end]);
                let inner = strip_dquote_meta(&script[i + 1..end]);
                push_word(&mut words, &inner, dynamic);
                at_word_start = false;
                i = end + 1;
            }
            b'`' => {
                let end = find_quote(b, i + 1, b'`')?;
                verdict = max(verdict, classify(&script[i + 1..end], ctx));
                dyn_word(&mut words);
                at_word_start = false;
                i = end + 1;
            }
            b'\\' => {
                if i + 1 >= b.len() {
                    return Err("trailing backslash".into());
                }
                push_word(&mut words, &script[i + 1..i + 2], false);
                at_word_start = false;
                i += 2;
            }
            b'$' => {
                i = handle_dollar(script, i, ctx, &mut verdict, &mut words)?;
                at_word_start = false;
            }
            b';' => {
                flush_cmd!();
                i += 1;
            }
            b'&' => {
                flush_cmd!();
                i += if i + 1 < b.len() && b[i + 1] == b'&' {
                    2
                } else {
                    1
                };
            }
            b'|' => {
                flush_cmd!();
                i += if i + 1 < b.len() && b[i + 1] == b'|' {
                    2
                } else {
                    1
                };
            }
            b'(' | b'{' => {
                let (open, close) = if c == b'(' {
                    (b'(', b')')
                } else {
                    (b'{', b'}')
                };
                flush_cmd!();
                let end = match_delim(b, i, open, close)?;
                verdict = max(verdict, classify(&script[i + 1..end], ctx));
                i = end + 1;
            }
            b')' | b'}' => return Err("unbalanced closing delimiter".into()),
            b'<' | b'>' => {
                end_word(&mut words);
                i = handle_redirect(script, i, ctx, &mut verdict, &mut heredocs)?;
                at_word_start = true;
            }
            _ => {
                // fd-prefixed redirect (2>file, 2>&1): digits glued to < or >
                if c.is_ascii_digit() {
                    let mut j = i;
                    while j < b.len() && b[j].is_ascii_digit() {
                        j += 1;
                    }
                    if j < b.len() && (b[j] == b'<' || b[j] == b'>') {
                        i = j;
                        continue;
                    }
                }
                let start = i;
                while i < b.len() && !is_meta(b[i]) {
                    i += 1;
                }
                push_word(&mut words, &script[start..i], false);
                at_word_start = false;
            }
        }
    }
    flush_cmd!();
    for (delim, expand) in heredocs {
        i = consume_heredoc(b, i, &delim, expand, ctx, &mut verdict)?;
    }
    Ok(verdict)
}

// ---------------------------------------------------------------------------
// Quote / substitution scanning
// ---------------------------------------------------------------------------

fn find_quote(b: &[u8], start: usize, q: u8) -> Result<usize, String> {
    let mut i = start;
    while i < b.len() {
        if b[i] == b'\\' && q == b'`' {
            i += 2;
            continue;
        }
        if b[i] == q {
            return Ok(i);
        }
        i += 1;
    }
    Err(format!("unterminated {}", q as char))
}

/// Scan a double-quoted string starting at `i` (the opening quote),
/// classifying `$(...)`/backtick contents. Returns the closing quote index.
fn find_dquote(b: &[u8], i: usize, ctx: &ClassCtx, verdict: &mut Outcome) -> Result<usize, String> {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return Ok(j),
            b'`' => {
                let end = find_quote(b, j + 1, b'`')?;
                absorb(
                    verdict,
                    classify(
                        std::str::from_utf8(&b[j + 1..end]).map_err(|_| "utf8")?,
                        ctx,
                    ),
                );
                j = end + 1;
            }
            b'$' if j + 1 < b.len() && b[j + 1] == b'(' => {
                let end = match_delim(b, j + 1, b'(', b')')?;
                absorb(
                    verdict,
                    classify(
                        std::str::from_utf8(&b[j + 2..end]).map_err(|_| "utf8")?,
                        ctx,
                    ),
                );
                j = end + 1;
            }
            _ => j += 1,
        }
    }
    Err("unterminated double quote".into())
}

fn has_dynamic(s: &str) -> bool {
    s.contains('$') || s.contains('`')
}

fn strip_dquote_meta(s: &str) -> String {
    s.replace("\\\"", "\"").replace("\\\\", "\\")
}

/// `$((...))`, `$(...)`, `${...}`, `$VAR`: classify substitutions, mark the
/// current word dynamic. Returns the next index.
fn handle_dollar(
    script: &str,
    i: usize,
    ctx: &ClassCtx,
    verdict: &mut Outcome,
    words: &mut Vec<Word>,
) -> Result<usize, String> {
    let b = script.as_bytes();
    if i + 1 >= b.len() {
        dyn_word(words);
        return Ok(i + 1);
    }
    match b[i + 1] {
        b'(' if i + 2 < b.len() && b[i + 2] == b'(' => {
            let end = match_delim(b, i + 1, b'(', b')')?; // arithmetic: no commands
            dyn_word(words);
            Ok(end + 1)
        }
        b'(' => {
            let end = match_delim(b, i + 1, b'(', b')')?;
            absorb(verdict, classify(&script[i + 2..end], ctx));
            dyn_word(words);
            Ok(end + 1)
        }
        b'{' => {
            let end = match_delim(b, i + 1, b'{', b'}')?;
            let inner = &script[i + 2..end];
            if inner.contains('$') {
                absorb(verdict, classify_expansions(inner, ctx)?);
            }
            dyn_word(words);
            Ok(end + 1)
        }
        _ => {
            // $VAR / $? / $$ ...
            let mut j = i + 1;
            while j < b.len() && ((b[j] as char).is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            dyn_word(words);
            Ok(if j == i + 1 { i + 2 } else { j })
        }
    }
}

/// Classify `$(...)`/backtick substitutions inside free text (here-doc bodies,
/// `${...}` defaults): the surrounding text is data, not commands.
fn classify_expansions(text: &str, ctx: &ClassCtx) -> Result<Outcome, String> {
    let b = text.as_bytes();
    let mut i = 0;
    let mut verdict = Outcome::Allow;
    while i < b.len() {
        match b[i] {
            b'`' => {
                let end = find_quote(b, i + 1, b'`')?;
                verdict = max(verdict, classify(&text[i + 1..end], ctx));
                i = end + 1;
            }
            b'$' if i + 1 < b.len() && b[i + 1] == b'(' => {
                let end = match_delim(b, i + 1, b'(', b')')?;
                let inner = if i + 2 < b.len() && b[i + 2] == b'(' {
                    &text[i + 3..end.saturating_sub(1)]
                } else {
                    &text[i + 2..end]
                };
                verdict = max(verdict, classify(inner, ctx));
                i = end + 1;
            }
            b'\\' => i += 2,
            _ => i += 1,
        }
    }
    Ok(verdict)
}

/// Index of the closing delimiter matching `open` at `i`, quote- and
/// nesting-aware.
fn match_delim(b: &[u8], i: usize, open: u8, close: u8) -> Result<usize, String> {
    let mut depth = 0usize;
    let mut j = i;
    while j < b.len() {
        match b[j] {
            b'\'' => j = find_quote(b, j + 1, b'\'')?,
            b'"' => {
                j += 1;
                while j < b.len() && b[j] != b'"' {
                    if b[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                if j >= b.len() {
                    return Err("unterminated double quote".into());
                }
            }
            b'`' => j = find_quote(b, j + 1, b'`')?,
            b'\\' => j += 1,
            c if c == open => depth += 1,
            c if c == close => {
                depth -= 1;
                if depth == 0 {
                    return Ok(j);
                }
            }
            _ => {}
        }
        j += 1;
    }
    Err("unbalanced opening delimiter".into())
}

// ---------------------------------------------------------------------------
// Redirects and here-docs
// ---------------------------------------------------------------------------

fn handle_redirect(
    script: &str,
    i: usize,
    ctx: &ClassCtx,
    verdict: &mut Outcome,
    heredocs: &mut Vec<(String, bool)>,
) -> Result<usize, String> {
    let b = script.as_bytes();
    let at = |k: usize, c: u8| k < b.len() && b[k] == c;

    // operator
    let (op, mut j) = if b[i] == b'>' {
        if at(i + 1, b'>') {
            (">>", i + 2)
        } else if at(i + 1, b'&') {
            (">&", i + 2)
        } else if at(i + 1, b'|') {
            (">|", i + 2)
        } else {
            (">", i + 1)
        }
    } else if at(i + 1, b'<') {
        if at(i + 2, b'<') {
            ("<<<", i + 3)
        } else if at(i + 2, b'-') {
            ("<<-", i + 3)
        } else {
            ("<<", i + 2)
        }
    } else if at(i + 1, b'>') {
        ("<>", i + 2)
    } else if at(i + 1, b'&') {
        ("<&", i + 2)
    } else {
        ("<", i + 1)
    };

    while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
        j += 1;
    }

    // fd-dup form (2>&1, 1>&2): target is a bare digit
    if op == ">&" || op == "<&" {
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        return Ok(j);
    }

    if op == "<<" || op == "<<-" {
        let (delim, expand, end) = read_heredoc_delim(script, j)?;
        if delim.is_empty() {
            return Err("here-doc without delimiter".into());
        }
        heredocs.push((delim, expand));
        return Ok(end);
    }

    if op == "<<<" {
        // here-string: expanded data; substitutions inside still run
        let end = read_target_word(b, j);
        absorb(verdict, classify_expansions(&script[j..end], ctx)?);
        return Ok(end);
    }

    let end = read_target_word(b, j);
    let target = &script[j..end];
    if target.is_empty() {
        // `>(...)` / `<(...)` process substitution: the '(' arm recurses
        return Ok(end);
    }
    absorb(
        verdict,
        if op == "<" {
            check_read_target(target, ctx)
        } else {
            check_write_target(target, ctx)
        },
    );
    Ok(end)
}

fn read_heredoc_delim(script: &str, j: usize) -> Result<(String, bool, usize), String> {
    let b = script.as_bytes();
    if j < b.len() && (b[j] == b'\'' || b[j] == b'"') {
        let end = find_quote(b, j + 1, b[j])?;
        return Ok((script[j + 1..end].to_string(), false, end + 1));
    }
    let mut end = j;
    while end < b.len() && !is_meta(b[end]) {
        end += 1;
    }
    Ok((script[j..end].to_string(), true, end))
}

fn read_target_word(b: &[u8], mut j: usize) -> usize {
    while j < b.len() && !is_meta(b[j]) {
        j += 1;
    }
    j
}

fn consume_heredoc(
    b: &[u8],
    start: usize,
    delim: &str,
    expand: bool,
    ctx: &ClassCtx,
    verdict: &mut Outcome,
) -> Result<usize, String> {
    let mut i = start.min(b.len());
    loop {
        let line_end = b[i..]
            .iter()
            .position(|c| *c == b'\n')
            .map(|p| i + p)
            .unwrap_or(b.len());
        let line = String::from_utf8_lossy(&b[i..line_end]);
        if line.trim() == delim {
            return Ok(line_end + 1);
        }
        if expand {
            absorb(verdict, classify_expansions(&line, ctx)?);
        }
        if line_end >= b.len() {
            return Err(format!("unterminated here-doc {delim}"));
        }
        i = line_end + 1;
    }
}

// ---------------------------------------------------------------------------
// Commands and path policy
// ---------------------------------------------------------------------------

fn classify_command(words: &[Word], ctx: &ClassCtx) -> Result<Outcome, String> {
    let Some(cmd) = words.iter().find(|w| !w.is_assignment()) else {
        return Ok(Outcome::Allow); // pure assignments
    };
    let name = cmd.lit.trim();
    let mut verdict = Outcome::Allow;

    // every literal that looks like a path is containment-checked
    for w in words.iter().filter(|w| !w.is_assignment()) {
        if w.dynamic {
            verdict = max(
                verdict,
                Outcome::Ask("argument comes from an expansion".into()),
            );
        } else {
            verdict = max(verdict, check_read_target(&w.lit, ctx));
        }
    }

    if cmd.dynamic {
        return Ok(max(
            verdict,
            Outcome::Ask("command name comes from an expansion".into()),
        ));
    }

    let args: Vec<&str> = words
        .iter()
        .filter(|w| !w.is_assignment())
        .skip(1)
        .map(|w| w.lit.trim())
        .collect();

    let v = match name {
        "find" => {
            if args.iter().any(|a| {
                matches!(
                    *a,
                    "-delete"
                        | "-exec"
                        | "-execdir"
                        | "-ok"
                        | "-okdir"
                        | "-fls"
                        | "-fprint"
                        | "-fprint0"
                        | "-fprintf"
                )
            }) {
                Outcome::Ask("find with a write/exec action".into())
            } else {
                Outcome::Allow
            }
        }
        "sort" if args.contains(&"-o") => Outcome::Ask("sort -o writes a file".into()),
        "sh" | "bash" | "ash" | "dash" | "zsh" | "rbash" => {
            // only `sh -c <script>`: classify the script text itself
            match args
                .iter()
                .position(|a| *a == "-c")
                .and_then(|p| args.get(p + 1))
            {
                Some(script) => classify(script, ctx),
                None => Outcome::Ask("shell without -c runs an opaque script".into()),
            }
        }
        _ if WRAPPERS.contains(&name) => {
            // wrappers run the first non-flag, non-assignment argument;
            // duration-like tokens (`timeout 5`, `nice -n 5`) are wrapper args
            let mut inner: Vec<Word> = words
                .iter()
                .filter(|w| !w.is_assignment())
                .skip(1)
                .cloned()
                .collect();
            while inner
                .first()
                .map(|w| {
                    w.lit.starts_with('-')
                        || w.lit.contains('=')
                        || w.lit
                            .chars()
                            .all(|c| c.is_ascii_digit() || matches!(c, '.' | 's' | 'm' | 'h' | 'd'))
                })
                .unwrap_or(false)
            {
                inner.remove(0);
            }
            if inner.is_empty() {
                Outcome::Allow
            } else {
                classify_command(&inner, ctx)?
            }
        }
        "git" => {
            let sub = args.first().copied().unwrap_or("");
            if matches!(
                sub,
                "status"
                    | "log"
                    | "diff"
                    | "show"
                    | "blame"
                    | "grep"
                    | "rev-parse"
                    | "ls-files"
                    | "ls-tree"
                    | "describe"
                    | "shortlog"
                    | "version"
                    | "help"
            ) {
                Outcome::Allow
            } else {
                Outcome::Ask(format!("git {sub} can mutate the repository"))
            }
        }
        "eval" | "exec" | "source" | "." => Outcome::Ask(format!("{name} runs opaque code")),
        _ if READ_ONLY.contains(&name) => Outcome::Allow,
        _ if NETWORK.contains(&name) => Outcome::Ask(format!("{name} touches the network")),
        _ if INTERPRETERS.contains(&name) => Outcome::Ask(format!("{name} runs opaque code")),
        _ => Outcome::Ask(format!(
            "command `{name}` is not on the read-only allowlist"
        )),
    };
    Ok(max(verdict, v))
}

/// A write redirection target: /dev/null is free, inside-workspace writes ask
/// (`.palmcode/**` always asks, per ADR 0012), outside-workspace writes deny.
fn check_write_target(target: &str, ctx: &ClassCtx) -> Outcome {
    let t = target.trim();
    if t.is_empty() || is_dev_null(t) {
        return Outcome::Allow;
    }
    match resolve(t, ctx) {
        Some(p) => {
            if p.starts_with(&ctx.workspace) {
                if p.starts_with(ctx.workspace.join(".palmcode")) {
                    Outcome::Ask(format!("writes .palmcode/ ({t})"))
                } else {
                    Outcome::Ask(format!("writes {t}"))
                }
            } else {
                Outcome::Deny(format!("write outside the workspace: {t}"))
            }
        }
        None => Outcome::Deny(format!("write outside the workspace: {t}")),
    }
}

fn check_read_target(target: &str, ctx: &ClassCtx) -> Outcome {
    let t = target.trim();
    if t.is_empty() || is_dev_null(t) || t.starts_with('-') {
        return Outcome::Allow; // flags are not paths
    }
    // only path-looking literals are containment-checked
    let pathish = t.starts_with('/') || t.starts_with('~') || t.split('/').any(|s| s == "..");
    if !pathish {
        return Outcome::Allow;
    }
    match resolve(t, ctx) {
        Some(p)
            if p.starts_with(&ctx.workspace)
                || ctx.extra_roots.iter().any(|r| p.starts_with(r)) =>
        {
            Outcome::Allow
        }
        _ => Outcome::Deny(format!("outside the workspace: {t}")),
    }
}

fn is_dev_null(t: &str) -> bool {
    matches!(
        t,
        "/dev/null" | "/dev/stdin" | "/dev/stdout" | "/dev/stderr"
    )
}

/// Lexical resolution (no filesystem access): workspace-relative joins, `~`
/// maps to the workspace home (the bash tool sets HOME = workspace root).
/// `None` = not statically resolvable (`~user` = someone else's home).
fn resolve(t: &str, ctx: &ClassCtx) -> Option<PathBuf> {
    let expanded: PathBuf = if let Some(rest) = t.strip_prefix("~/") {
        ctx.workspace.join(rest)
    } else if t == "~" {
        ctx.workspace.clone()
    } else if t.starts_with('~') {
        return None; // ~user: another user's home directory
    } else if t.starts_with('/') {
        PathBuf::from(t)
    } else {
        ctx.workspace.join(t)
    };
    Some(normalize(&expanded))
}

fn normalize(p: &Path) -> PathBuf {
    use std::path::Component::*;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            ParentDir => {
                out.pop();
            }
            CurDir => {}
            Normal(s) => out.push(s),
            RootDir => out.push("/"),
            Prefix(pr) => out.push(pr.as_os_str()),
        }
    }
    out
}

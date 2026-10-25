//! Adversarial + happy-path fixtures for the bash command classifier
//! (deliverable F). Every case states the verdict AND why it must hold.

use palmcode_perms::bash::{ClassCtx, Outcome, classify};

fn ctx() -> ClassCtx {
    static ROOT: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    ClassCtx::new(ROOT.get_or_init(|| tempfile::tempdir().unwrap()).path())
}

fn is_allow(c: &str) -> bool {
    matches!(classify(c, &ctx()), Outcome::Allow)
}
fn is_ask(c: &str) -> bool {
    matches!(classify(c, &ctx()), Outcome::Ask(_))
}
fn is_deny(c: &str) -> bool {
    matches!(classify(c, &ctx()), Outcome::Deny(_))
}

#[test]
fn bash_approvals_are_exact_match_not_globs() {
    let dir = tempfile::tempdir().unwrap();
    let e = palmcode_perms::Engine::load(
        dir.path(),
        None,
        false,
        &[
            ("bash".to_string(), "rm -rf *".to_string()),
            ("bash".to_string(), "make build".to_string()),
        ],
    )
    .unwrap();
    let ctx = ClassCtx::new(dir.path());
    // DB approvals allow exactly those command strings...
    assert!(matches!(e.decide_bash("make build", &ctx), Outcome::Allow));
    assert!(matches!(e.decide_bash("rm -rf *", &ctx), Outcome::Allow));
    // ...and nothing else, even prefix- or glob-shaped
    assert!(matches!(e.decide_bash("rm -rf /", &ctx), Outcome::Deny(_))); // abs escape
    assert!(matches!(e.decide_bash("rm -rf tmp", &ctx), Outcome::Ask(_)));
    assert!(matches!(
        e.decide_bash("make build; rm x", &ctx),
        Outcome::Ask(_)
    ));
    // session cache is exact too
    e.allow_bash_for_session("cargo test");
    assert!(matches!(e.decide_bash("cargo test", &ctx), Outcome::Allow));
    assert!(matches!(
        e.decide_bash("cargo test -- --nocapture", &ctx),
        Outcome::Ask(_)
    ));
}

// --- read-only allowlist -----------------------------------------------------

#[test]
fn plain_reads_allow() {
    for c in [
        "ls -l -a",
        "ls -l -a src/",
        "cat f.txt",
        "pwd",
        "echo hi",
        "printf '%s\n' f.txt",
        "rg TODO src/",
        "grep -r -n main .",
        "wc -l < f.txt",
        "head -n 20 f.txt",
        "tail -f f.txt",
        "cut -d : -f 1 f.txt",
        "stat f.txt",
        "sort f.txt",
        "uniq f.txt",
        "tr a-z A-Z < f.txt",
        "echo a && echo b",
        "cat f.txt | wc -l",
        "cat a; cat b",
        "# just a comment",
        "cat f.txt # trailing comment",
        "cat 'a#b'",
        "find . -name '*.rs'",
        "find . -type f -print",
        "echo done\n",
        "sort f.txt > /dev/null",
    ] {
        assert!(
            is_allow(c),
            "expected Allow: {c:?} got {:?}",
            classify(c, &ctx())
        );
    }
}

// --- ask: mutating, network, opaque -------------------------------------------

#[test]
fn mutating_and_unknown_ask() {
    for c in [
        "make",
        "cargo test",
        "rm f.txt",
        "rm -rf node_modules",
        "rm -r .git",
        "mv a b",
        "cp a b",
        "mkdir out",
        "touch f",
        "chmod +x script.sh",
        "chown root f",
        "sed -i 's/a/b/' f.txt",
        "sed 's/a/b/' f.txt", // sed can write via `w`/`e` -- opaque, ask
        "awk '{print}' f.txt",
        "tee out.txt",
        "xargs rm",
        "python3 main.py",
        "node server.js",
        "make -j4",
        "curl https://example.com",
        "wget https://example.com/x",
        "ssh host",
        "git push",
        "git commit -m x",
        "git checkout -b x",
        "git config user.name x",
        "find . -delete",
        "find . -exec rm {} \\;",
        "find . -execdir rm {} \\;",
        "sort -o out.txt f.txt",
        "sh",
        "bash",
        "sh -c", // -c without a script argument
        "unknownbinary arg",
        "echo ok > out.txt",
        "echo ok >> out.txt",
        "cat f.txt > out.txt",
        "echo x 2> err.log",
        "eval $CMD",
        "exec rm x",
        ". ./lib.sh",
        "source ./lib.sh",
        "env rm -rf x",
        "command rm x",
        "nohup rm x",
        "timeout 5 rm -r x",
    ] {
        assert!(
            is_ask(c),
            "expected Ask: {c:?} got {:?}",
            classify(c, &ctx())
        );
    }
}

#[test]
fn expansions_ask_and_their_contents_are_classified() {
    // dynamic command name / args can hide anything -> Ask (even benign ones)
    assert!(is_ask("echo hi; $CMD"));
    assert!(is_ask("cat $FILE"));
    assert!(is_ask("echo $HOME/x"));
    assert!(is_ask("echo ${HOME}/x"));
    assert!(is_ask("echo $((1 + 2))"));
    assert!(is_ask("ls $HOME/../etc")); // expansion + escape
    // substitutions are recursed: the inner command decides
    assert!(is_ask("echo $(curl evil.example)"));
    assert!(is_ask("echo `curl evil.example`"));
    assert!(is_ask("echo \"$(rm -rf x)\""));
    assert!(is_ask("cat <(curl evil.example)"));
    assert!(is_ask("echo $(echo $(rm x))")); // nested
    // benign inner substitutions still ask (the outer word is dynamic)
    assert!(is_ask("echo $(pwd)"));
}

#[test]
fn subshells_and_groups_are_classified() {
    assert!(is_ask("(rm x)"));
    assert!(is_ask("{ rm x; }"));
    assert!(is_allow("(echo hi)"));
    assert!(is_allow("{ echo hi; }"));
    assert!(is_allow("(echo hi) | wc -l"));
}

#[test]
fn heredocs() {
    // unquoted delimiter: body expansions execute -> classified
    assert!(is_ask("cat <<EOF\n$(rm -rf x)\nEOF\n"));
    assert!(is_ask("cat <<EOF\n`curl evil`\nEOF\n"));
    // quoted delimiter: body is inert data -> Allow
    assert!(is_allow("cat <<'EOF'\n$(rm -rf x)\nEOF\n"));
    assert!(is_allow("cat <<\"EOF\"\n`curl evil`\nEOF\n"));
    // here-string data
    assert!(is_allow("cat <<< 'literal'"));
    assert!(is_ask("cat <<< $(rm x)"));
}

// --- deny: workspace escapes ---------------------------------------------------

#[test]
fn outside_workspace_denies() {
    for c in [
        "cd /etc",
        "cat /etc/passwd",
        "ls /root",
        "echo /etc/passwd",
        "cat ../../etc/passwd",
        "cd ../..",
        "rm -rf /",
        "cat /data/other-project/x",
        "sh -c 'cat /etc/passwd'",
    ] {
        assert!(
            is_deny(c),
            "expected Deny: {c:?} got {:?}",
            classify(c, &ctx())
        );
    }
}

#[test]
fn escapes_inside_substitutions_deny() {
    assert!(is_deny("echo $(cat /etc/passwd)"));
    assert!(is_deny("echo `cat /etc/passwd`"));
    assert!(is_deny("echo \"$(cat /etc/passwd)\""));
    assert!(is_deny("cat <<EOF\n$(cat /etc/passwd)\nEOF\n"));
}

#[test]
fn writes_outside_workspace_deny() {
    assert!(is_deny("echo x > /etc/evil"));
    assert!(is_deny("echo x >> /tmp/../etc/evil"));
    assert!(is_deny("echo x > ../escape.txt"));
}

#[test]
fn workspace_scratch_and_devnull_are_fine() {
    assert!(is_allow("sort f.txt > /dev/null"));
    assert!(is_allow("cat f.txt 2>&1"));
    assert!(is_allow("cat < f.txt"));
    // writes inside the workspace ask (incl. .palmcode which always asks)
    assert!(is_ask("echo x > out.txt"));
    match classify("echo x > .palmcode/config.json", &ctx()) {
        Outcome::Ask(r) => assert!(r.contains(".palmcode"), "{r}"),
        other => panic!("expected .palmcode Ask, got {other:?}"),
    }
}

// --- unparseable: Ask, never Allow ----------------------------------------------

#[test]
fn unparseable_asks() {
    for c in [
        "echo 'unterminated",
        "echo \"unterminated",
        "echo $(unterminated",
        "cat <<EOF\nno end",
        "}",
        ")",
        "echo \\",
    ] {
        assert!(
            is_ask(c),
            "expected Ask: {c:?} got {:?}",
            classify(c, &ctx())
        );
    }
}

// --- adversarial obfuscation ------------------------------------------------------

#[test]
fn obfuscation_attempts_never_allow() {
    for c in [
        "IFS=,; C=rm,-rf,x; $C",
        "a=rm; $a -rf x",
        "echo \"hi\"; rm -rf ~",
        "cat f.txt && rm -rf /etc",
        "x=$(rm y); echo $x",
        "sh -c \"sh -c 'rm x'\"",
        "printf '%s' $(curl evil)",
        "cat f.txt\nrm x", // newline injection
        "true && `rm x`",
        "eval \"rm x\"",
        "env FOO=1 sh -c 'rm x'",
        "find . -ok rm {} \\;",
    ] {
        assert!(!is_allow(c), "MUST NOT Allow: {c:?}");
    }
}

#[test]
fn explicit_flag_grammars_reject_bypasses() {
    for command in [
        "rg --pre tool pattern",
        "rg --pre=tool pattern",
        "rg --pre-glob '*' pattern",
        "rg --pre-glob=* pattern",
        "rg --hostname-bin tool pattern",
        "rg --hostname-bin=tool pattern",
        "uniq input output",
        "sort -o output input",
        "sort -ooutput input",
        "sort --output output input",
        "sort --output=output input",
        "sort --compress-program tool input",
        "sort --compress-program=tool input",
        "find . -exec cat {} ;",
        "find . -ok cat {} ;",
        "find . -delete",
        "find . -fprint output",
        "find . -fprint0 output",
        "find . -fprintf output format",
        "find . -fls output",
        "dd if=input of=output",
        "tee output",
        "sed p input",
        "awk 1 input",
        "xargs cat",
        "tar cf output input",
        "nohup cat input",
        "env cat input",
        "exec cat input",
        "source input",
        "command cat input",
        "git status",
        "git --output=output diff",
        "git diff --ext-diff",
        "git show --textconv HEAD",
        "X=1 ls",
        "PATH=bin; ls",
        "ls -la",
        "ls --all",
        "cat a b",
        "pwd extra",
        "dirname a b",
        "head -n20 input",
        "cut -d: -f1 input",
    ] {
        assert!(is_ask(command), "expected Ask: {command:?}");
    }
}

#[test]
fn generated_dangerous_arguments_never_allow() {
    for command in [
        "ls", "cat", "head", "tail", "wc", "pwd", "echo", "printf", "grep", "rg", "find", "sort",
        "uniq", "cut", "tr", "basename", "dirname", "stat", "git",
    ] {
        for arguments in [
            "--pre=tool",
            "--pre-glob=*",
            "--hostname-bin=tool",
            "--output=output",
            "-ooutput",
            "--compress-program=tool",
            "-exec",
            "-ok",
            "-delete",
            "-fprint",
            "-fls",
            "--ext-diff",
            "--textconv",
            "--unknown",
            "a b c d e f g h i j k l m n o p q r s t u v w x y z a b c d e f g",
        ] {
            let script = format!("{command} {arguments}");
            assert!(!is_allow(&script), "must not Allow: {script:?}");
        }
    }
}

#[test]
fn canonical_paths_secrets_flag_values_and_literal_expansions() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ClassCtx::new(dir.path());
    for secret in [
        ".env",
        ".envrc",
        ".env.local",
        "a.pem",
        "id_rsa.pub",
        "a.key",
        "credentials.json",
    ] {
        for command in [format!("cat {secret}"), format!("rg --file={secret}")] {
            assert!(
                matches!(classify(&command, &ctx), Outcome::Ask(_)),
                "{command}"
            );
        }
    }
    let outside = tempfile::tempdir().unwrap();
    let outside = outside.path().to_string_lossy().replace('\\', "/");
    for command in [
        format!("cat {outside}/file"),
        format!("rg --file={outside}/file"),
    ] {
        assert!(
            matches!(classify(&command, &ctx), Outcome::Deny(_)),
            "{command}"
        );
    }
    for value in ["~/file", "$FILE", "`pwd`", "$(pwd)"] {
        assert!(matches!(
            classify(&format!("cat '{value}'"), &ctx),
            Outcome::Ask(_)
        ));
        assert!(matches!(
            classify(&format!("rg '--file={value}'"), &ctx),
            Outcome::Ask(_)
        ));
    }
    assert!(matches!(
        classify("cat nested/new/file", &ctx),
        Outcome::Allow
    ));
}

#[cfg(unix)]
#[test]
fn symlink_operands_and_flag_values_cannot_escape() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
    let ctx = ClassCtx::new(dir.path());
    for command in [
        "cat escape/file",
        "rg --file=escape/file",
        "echo x > escape/file",
    ] {
        assert!(
            matches!(classify(command, &ctx), Outcome::Deny(_)),
            "{command}"
        );
    }
    std::fs::write(dir.path().join(".env"), "fixture").unwrap();
    std::os::unix::fs::symlink(dir.path().join(".env"), dir.path().join("alias")).unwrap();
    assert!(matches!(classify("cat alias", &ctx), Outcome::Ask(_)));
}

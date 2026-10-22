//! Adversarial + happy-path fixtures for the bash command classifier
//! (deliverable F). Every case states the verdict AND why it must hold.

use palmcode_perms::bash::{ClassCtx, Outcome, classify};

fn ctx() -> ClassCtx {
    ClassCtx::new("/data/project")
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

// --- read-only allowlist -----------------------------------------------------

#[test]
fn plain_reads_allow() {
    for c in [
        "ls -la",
        "ls -la src/",
        "cat f.txt",
        "pwd",
        "echo hi",
        "printf '%s\n' f.txt",
        "rg TODO src/",
        "grep -rn main .",
        "wc -l < f.txt",
        "head -n 20 f.txt",
        "tail -f f.txt",
        "cut -d: -f1 f.txt",
        "diff a.txt b.txt",
        "stat f.txt",
        "file f.txt",
        "which sh",
        "realpath ./x",
        "sort f.txt",
        "uniq f.txt",
        "tr a-z A-Z < f.txt",
        "date",
        "uname -a",
        "true",
        "false",
        "echo a && echo b",
        "cat f.txt | wc -l",
        "cat a; cat b",
        "# just a comment",
        "X=1 ls",
        "cat f.txt # trailing comment",
        "cat 'a#b'",
        "git status",
        "git log --oneline",
        "git diff HEAD",
        "git show HEAD:file.txt",
        "git grep pattern",
        "find . -name '*.rs'",
        "find . -type f -print",
        "echo done\n",
        "sha256sum f.txt",
        "md5sum -c sums.txt",
        "sort f.txt > /dev/null",
        "env",
        "nice cat f.txt",
        "nohup echo ok",
        "timeout 5 cat f.txt",
        "cat ~/.notes", // HOME == workspace root (bash tool env contract)
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
        "cat ~/../etc/passwd",
        "cat ~root/.bashrc",
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

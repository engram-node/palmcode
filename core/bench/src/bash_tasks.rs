use std::path::Path;

pub struct Task {
    pub id: &'static str,
    pub prompt: &'static str,
    pub files: &'static [(&'static str, &'static str)],
    /// Trusted verifier, never written into the model's workspace.
    pub verify: &'static str,
    pub require_error: bool,
    pub require_timeout: bool,
}

macro_rules! task {
    ($id:literal, $prompt:literal, [$($path:literal => $body:literal),*], $verify:literal) => {
        Task { id: $id, prompt: $prompt, files: &[$(($path, $body)),*], verify: $verify,
            require_error: false, require_timeout: false }
    };
}

pub const TASKS: &[Task] = &[
    task!("fix-shell-test", "Run sh test.sh. Fix greet.sh so the test passes; preserve test.sh.",
        ["greet.sh" => "printf 'helo\\n'\n", "test.sh" => "test \"$(sh greet.sh)\" = hello\n"],
        "test \"$(sh greet.sh)\" = hello && test \"$(cat test.sh)\" = 'test \"$(sh greet.sh)\" = hello'"),
    task!("awk-log", "Use awk to total successful request bytes per user from requests.log. Write sorted totals.tsv as user, a tab, total.",
        ["requests.log" => "alice 200 10\nbob 500 99\nalice 200 7\nbob 200 3\n"],
        "printf 'alice\t17\nbob\t3\n' | cmp - totals.tsv"),
    task!("sed-log", "Use sed to replace literal token= values with REDACTED in access.log, writing clean.log. Preserve other fields and access.log.",
        ["access.log" => "GET /a token=demo123 status=200\nGET /b token=demo456 status=404\n"],
        "printf 'GET /a token=REDACTED status=200\nGET /b token=REDACTED status=404\n' | cmp - clean.log"),
    task!("find-xargs-rename", "Use find with NUL delimiters and xargs to rename all .old files under data to .txt. Preserve contents and spaces in names.",
        ["data/a.old" => "alpha\n", "data/two words.old" => "beta\n", "data/sub/c.old" => "gamma\n"],
        "test ! -e data/a.old && test ! -e 'data/two words.old' && test ! -e data/sub/c.old && test \"$(cat data/a.txt)\" = alpha && test \"$(cat 'data/two words.txt')\" = beta && test \"$(cat data/sub/c.txt)\" = gamma"),
    task!(
        "create-script",
        "Create sum.sh accepting integer arguments and printing their sum. Run it with 3 4 5, saving output to result.txt.",
        [],
        "test \"$(sh sum.sh 3 4 5)\" = 12 && test \"$(sh sum.sh -2 5)\" = 3 && test \"$(cat result.txt)\" = 12"
    ),
    task!("local-git", "Use local git only: init, set repository-local identity Bench / bench@example.invalid, commit note.txt, create branch work, change note.txt to new, commit, and save git diff --no-ext-diff --no-textconv HEAD~1 HEAD -- note.txt in change.diff. Do not use remotes.",
        ["note.txt" => "old\n"],
        "test \"$(git branch --show-current)\" = work && test \"$(git rev-list --count HEAD)\" = 2 && test \"$(git show HEAD:note.txt)\" = new && git diff --no-ext-diff --no-textconv HEAD~1 HEAD -- note.txt | cmp - change.diff"),
    task!("grep-change", "Find OLD_PORT references with grep; change them to NEW_PORT in both config files. Run sh verify.sh; preserve that script.",
        ["a.conf" => "OLD_PORT=8000\n", "nested/b.conf" => "OLD_PORT=9000\n", "verify.sh" => "! grep -r OLD_PORT --include='*.conf' .\n"],
        "test \"$(cat a.conf)\" = NEW_PORT=8000 && test \"$(cat nested/b.conf)\" = NEW_PORT=9000"),
    task!("text-transform", "Trim surrounding spaces, drop blank lines, lowercase and sort unique entries in names.txt into normalized.txt.",
        ["names.txt" => "  BOB \nalice\n\nBob\n ALICE  \n"], "printf 'alice\nbob\n' | cmp - normalized.txt"),
    task!("restructure", "Move src/a.txt and src/sub/b.txt into archive preserving their relative paths, remove the empty src directories, and write a sorted manifest.txt of file paths beneath archive (without archive/ prefix).",
        ["src/a.txt" => "A\n", "src/sub/b.txt" => "B\n"],
        "test ! -e src && test \"$(cat archive/a.txt)\" = A && test \"$(cat archive/sub/b.txt)\" = B && printf 'a.txt\nsub/b.txt\n' | cmp - manifest.txt && (cd archive && find . -type f | sed 's|^./||' | sort) | cmp - manifest.txt"),
    Task {
        id: "timeout-recovery",
        prompt: "Call bash with command 'sleep 30' and timeout_ms 1000. After observing timeout, use bash to write recovered to recovered.txt. Do not wait the full sleep.",
        files: &[],
        verify: "test \"$(cat recovered.txt)\" = recovered",
        require_error: true,
        require_timeout: true,
    },
    Task {
        id: "failure-recovery",
        prompt: "First run sh work.sh and observe its failure. Fix it to print ready, then run it and save output in result.txt.",
        files: &[("work.sh", "exit 7\n")],
        verify: "test \"$(sh work.sh)\" = ready && test \"$(cat result.txt)\" = ready",
        require_error: true,
        require_timeout: false,
    },
    task!("join-data", "Join users.tsv and scores.tsv on ID and write scores-by-name.tsv sorted by name, name then tab then score. Use shell tools.",
        ["users.tsv" => "2\tbob\n1\talice\n", "scores.tsv" => "1\t9\n2\t7\n"], "printf 'alice\t9\nbob\t7\n' | cmp - scores-by-name.tsv"),
    task!("tar-roundtrip", "Create bundle.tar from input, extract it into restored, preserving paths and contents.",
        ["input/a.txt" => "one\n", "input/sub/b.txt" => "two\n"], "test \"$(tar -xOf bundle.tar input/a.txt)\" = one && test \"$(tar -xOf bundle.tar input/sub/b.txt)\" = two && diff -r input restored/input"),
    task!("count-errors", "Count each ERROR code in service.log into errors.tsv sorted by code, tab-separated code and count; ignore INFO.",
        ["service.log" => "ERROR E2\nINFO E1\nERROR E1\nERROR E2\n"], "printf 'E1\t1\nE2\t2\n' | cmp - errors.tsv"),
    task!("idempotent-script", "Create ensure.sh which ensures exactly one enabled=true line in settings.conf, preserving existing lines. Run it twice.",
        ["settings.conf" => "name=demo\n"], "sh ensure.sh && sh ensure.sh && printf 'name=demo\nenabled=true\n' | cmp - settings.conf"),
];

pub fn seed(task: &Task, root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    for (path, content) in task.files {
        let target = root.join(path);
        std::fs::create_dir_all(target.parent().unwrap())?;
        std::fs::write(target, content)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fifteen_unique_tasks_seed_isolated_workspaces() {
        let mut names = std::collections::HashSet::new();
        assert_eq!(TASKS.len(), 15);
        for task in TASKS {
            assert!(names.insert(task.id));
            assert!(!task.verify.is_empty());
            let dir = tempfile::tempdir().unwrap();
            seed(task, dir.path()).unwrap();
            for (path, text) in task.files {
                assert_eq!(
                    std::fs::read_to_string(dir.path().join(path)).unwrap(),
                    *text
                );
            }
        }
    }
}

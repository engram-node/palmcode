# 0015. bash command handling: classifier, tool contract, command catalog

Date: 2026-10-23. Status: accepted. Scope: M3 deliverables D, F, H (plus the
wiring that carries them).

## Context

M3 adds a `bash` tool: one fresh `sh -c` per call on Android and hosts. The
model writes arbitrary shell text, so permission decisions cannot be made on
a path argument; they must be made on the parsed command. Exec mechanics
(process groups, shebangs, kill semantics) are ADR 0014; this record covers
classification, the tool contract, approval caching and the shipped command
catalog.

## Decisions

1. **Classifier: hand-rolled conservative scanner** (`core/perms/src/bash.rs`).
   Word/quote/heredoc/comment aware, recurses into `$(...)`, backticks,
   subshells, `sh -c <script>`, process substitution and wrapper commands.
   Unparseable input is `Ask`, never `Allow`.
2. **Three verdicts.** `Allow` only for provably read-only commands (name
   allowlist, no mutating flags/redirects); `Ask(reason)` for mutating,
   network, interpreter, wrapper, dynamic or unparseable input;
   `Deny(reason)` for workspace escapes (absolute paths outside the
   workspace, `..` escapes, `~user`, write redirects leaving the workspace).
3. **Dynamic arguments are `Ask` even when benign** (`echo $HOME/x`): the
   value is unknowable statically and the scanner must never guess in the
   permissive direction.
4. **`HOME` = workspace root** at exec time; the classifier maps `~/...` to
   the same place. `~user/...` is `Deny` (someone else's home). This keeps
   the classifier decoupled from the executor environment while both agree
   on where home is.
5. **Bash approvals are exact command strings**, in a session set and in
   `project_approvals` rows keyed by workspace id (`tool = "bash"`). They
   never enter the glob rule layers: a command containing `*` must not
   broaden the match.
6. **A bash call forces the serial execution branch** for its whole batch
   (mutating; strict call order). Preimage capture remains write-tools-only.
7. **Tool contract** (implements 0014): fresh `sh -c`, new session/pgid +
   PDEATHSIG, stdin closed, env allowlist (PATH, HOME, TMPDIR, LANG, TERM,
   and `git config --global` equivalents pinned to
   `safe.directory = <workspace>` only), timeout 120 s default / 600 s max
   with TERM -> 2 s -> KILL kill-tree, merged stdout+stderr, head/tail
   capture + full spill file, pidfile + orphan sweep. Scripts are invoked as
   `sh script` because `#!/usr/bin/env` cannot exec (0014).
8. **Android userland discovery via `dladdr`** on our own symbol: the native
   lib dir holds `libbusybox.so` / `librg.so` / `libgit.so` (jniLibs
   packaging, 0007). A symlink farm named by applet (367 names, the pinned
   busybox build's `--list` output) is built once under the app-private
   spill base; `PATH` serves the farm only; `sh` runs as `<farm>/sh` so
   basename applet dispatch applies. Hosts use the platform `sh`.
9. **Live output is batched**: merged stdout+stderr is emitted as
   `ToolOutput` events at 4 KiB or 250 ms boundaries, then flushed at exit.
   The FFI boundary stays coarse (0011): never per-chunk calls.
10. **Command catalog (H)** is static text in the system prompt, placed
    before the volatile env block so the prompt head stays the prefix-cache
    key: the 367 busybox applets plus `rg` (ripgrep 15) and `git` (2.x) notes
    and the `sh script` convention. Source of truth:
    `core/exec/src/applets.rs` (generated from the pinned busybox build).

## Rejected

- **tree-sitter-bash** (or another C grammar) for classification: build and
  byte-repro cost for what a conservative scanner gets right by refusing.
  If false-positive `Ask` rates annoy in practice, revisit with data.
- **Allow-by-default with a denylist**: obfuscation in the fixtures
  (`core/perms/tests/bash.rs`) defeats name matching alone.
- **Glob-matched bash approvals**: `*` and `?` in commands are common and
  would silently widen grants.
- **Passing the real `HOME` into the classifier**: couples classification to
  executor env; one fixed convention is checkable in both directions.
- **An FFI parameter for the native lib dir**: JNI/Kotlin churn for what
  `dladdr` answers in-process. Rejected in favor of discovery; if discovery
  ever misfires the fallback is the host executor and tests will notice.
- **Per-chunk streaming events**: defeats the batching rule and floods the
  reducer on fast output.

# 0011: M2 tool-loop architecture

## Context

M2 adds file tools + permissions + workspaces to the M1 chat harness.
Zero new product concepts; everything hangs off the three existing seams:
`ChatRequest.tools`, `ClientEvent::Tools`, `run_turn`.

## Decisions

**Crates:** new `core/tools` (Executor-trait impls, path safety, matchers,
output caps/spills, tree-sitter check); new `core/perms` (rule engine);
`core/agent` gets the tool loop; `core/store` gains tables.

**Tool loop (deliverable A).** After assistant finish_reason="tool_calls":
accumulated `ToolCall`s dispatch — read-only tools (`read`, `grep`, `glob`,
`ls`, `todoread`, `question`?) run via `tokio::task::JoinSet` (results
emitted in original call order), write tools (`write`, `edit`,
`apply_patch`, `todowrite`) run serially. Tool results append to
`api_messages` as `role:"tool"` and the SAME `run_turn` task loops the
chat request until a non-tool finish. Cap iterations (default 40) to bound
bad loops; per-loop context-guard: estimated prompt >= 85% of
context_window -> TurnError "context nearly full, start a new session"
(compaction is M4).

**Executor trait (AGENTS.md).** All host I/O through `core/exec::Executor`;
tools never touch `std::process`. `HostExecutor` (host-cli bench) and
`AndroidExecutor` remain the two impls; M2 tools use only filesystem APIs.

**Path safety (C).** One module `tools::paths`: canonicalize + symlink
resolution, reject `..` after normalization, root-escape rejection,
Unicode NFKC compare, case-fold on Windows/lower compare. Every tool call
legitimates against the session's granted workspace set + explicit
approvals. Adversarial tests: symlinks, `..`, `\\?\`, unicode lookalikes,
case tricks, NTFS streams.

**Guards (C).** `tools::readcache`: per-session read-before-write with
(path, mtime, blake3) entries; write/edit/apply_patch fail stale-detected
with the fresh snippet. Output: line/byte caps; spill files under
`<app-files>/spill/<session>/<uuid>.txt`, referenced from the tool result.
Tree-sitter post-edit check for: rust, kotlin, java, python, js/ts, json,
yaml, markdown, shell (9 grammars; native size measured, recorded in
BENCHMARKS.md: tree-sitter adds ~1.5-3 MB; APK balloon was solved by R8).

**Permissions (D).** Decision = layer lookup: builtin defaults <- user
global <- project `.palmcode/config.json`. Rule = {tool glob, path glob,
effect allow|ask|deny}. Defaults: read-inside-workspace = allow; writes =
ask ("accept edits for session" shortcut); outside workspace = deny unless
approved; `.git/**` writes deny; secret-like reads (.env*, *.pem, id_rsa*,
*.key) = ask. Approval sheet options map to scopes: once / session
(engine-held set) / project (persisted) / deny. Tool output NEVER mutates
rules (rules load from disk+UI only).

**Persistence (F).** New tables: `tool_calls` (id, message_id, name, args,
status, duration_ms, output spill ref, preimage ref), `file_preimages`
(call_id, path, content|hash), `spill_refs` inside events. Crash recovery:
tool calls left "running" become synthetic "interrupted, outcome unknown"
results; resume never re-runs a started call. `file_preimages` enable the
M2 diff view (+ M4 undo). DB size in settings + retention: preimages/spills
older than 30 days GC'd; VACUUM on boot when freelist > 20%.

**Token accounting (G).** EMA (alpha 0.35) of bytes-per-token per model,
seeded from response `usage.prompt_tokens` vs request byte skin; feeds the
85% guard with a 5% safety margin; calibration error logged to the session
DB meta table.

**System prompt (I).** Byte-stable prefix: [static agent instructions +
tool definitions (deterministic order) + GLM-family template wrapper] then
trailing environment block (date, workspace paths, storage status).
Cache-hit rate validated against Grid cached_tokens per turn, recorded in
BENCHMARKS.md + VERIFY.md.

**UI (H).** Tool cards keyed by call_id (LazyColumn), collapsible; unified
diff from preimages (own +/-/context painter, no syntax lib); approval
sheet; workspace screens; auto-scroll pins footer at message end; toks
clamp < 250 ms.

**edit-bench (K).** `core/bench --mode edits`: 20 tasks under
`core/bench/edit-tasks/` (intent + workspace snapshot + checker script in
Rust), run --grid, output markdown table to stdout.

## Rejected

- Parallel tool execution for write tools (serial is correct + simpler in
  crash recovery).
- ripgrep binary reuse for grep: crosses the no-binary-in-repo + F-Droid
  line at runtime (userland ships in M3, not M2); in-process `ignore` +
  `grep-*` crates instead.
- Full git integration in workspaces (explicitly out of scope).

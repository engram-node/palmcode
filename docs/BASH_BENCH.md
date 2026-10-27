# Bash benchmark

Build from `core`: `cargo build -p palmcode-bench --bin bash_bench --release`.
The host needs sh, awk, sed, find, xargs, grep, sort, cmp, diff, tar and git
on PATH (MSYS2 on Windows). All tool/verifier children use HostExecutor.
On Windows, put the MSYS2 `usr/bin` directory ahead of system sh/git on
the process PATH so nested shells resolve to the same host userland.
The git fixture uses `git diff --no-ext-diff --no-textconv` because the
executor deliberately disables external diff programs.

Set `PALMCODE_LLM_BASE_URL`, `PALMCODE_LLM_API_KEY` and `PALMCODE_LLM_MODEL`
in the host process environment. App provider settings are not used. The
key is not a CLI argument. To run manually from `core` in PowerShell:

```powershell
cargo run -p palmcode-bench --bin bash_bench --release -- --repeats 3 --model "$env:PALMCODE_LLM_MODEL" --trace-dir "$env:TEMP/palmcode-bash-results"
```

Use a new trace directory for each invocation; existing result files are
never overwritten. `--model` overrides the model environment variable.
`--task text-transform` selects a single task. `--list` lists the fixtures
without reading provider configuration or making requests. No live run
has been performed by the coding agent.

Fixtures: failing shell test, awk log totals, sed redaction, NUL-safe bulk
rename, script creation/execution, local git init/commits/branch/diff,
grep-driven configuration change, text normalization, directory restructure
with manifest, timeout recovery, exit-failure recovery, table join, tar
round trip, error-code counts and idempotent script creation.

Every repeat runs both approval modes in separate fresh trusted workspaces.
The runner answers remaining approvals allow-once, counts ASKS, and keeps
hard asks in the auto-approve mode. It stops a task after the agent's bounded
iteration limit or the benchmark wall deadline. Verification requires a
successful final reply, a bash call and the deterministic checks; recovery
tasks also require an observed failure/timeout. Only a completed task can
pass. Each task has independent SQLite/session state; no user workspace is
used. The verifier deadline is separate from task wall time.

Stdout is JSON Lines: one result per task/repeat/mode, followed by one
summary for each mode. Paste both summary lines for comparison. Optional
trace files retain the per-run numeric result; they omit model/provider
identity and all command/output content. Record model identity privately
when comparing your runs.

Metric definitions:

- `turns`: model iterations, including a failed/cancelled iteration when it
  returns; request retry counts are separate. It is not the number of tools.
- Tokens: sum of reported usage across iterations. Any missing request usage
  makes the aggregate unknown, not zero. The task-level prompt total differs
  from the app's last-prompt display. A failed request may have unreported usage.
- `tool_error_rate`: failed tool results divided by attempted tool calls.
  Deliberate timeout/failure fixtures contribute errors even when they pass.
- `wall_ms`: submission through terminal event/deadline, excluding setup and
  verification. Request TTFT/decode rate are in the `requests` array.
- `decode_tps`: reported completion tokens divided by request wall time minus
  TTFT; windows below 250 ms are null. It includes transport/scheduling cost.
- `cache_hit_rate`: summed reported cached tokens / summed prompt tokens;
  unknown if any cache count is missing or the ratio is invalid. No inferred hits.
- Summaries provide per-repeat pass rates, population standard deviation,
  per-task ASKS arrays, and min/max/mean/nearest-rank p50/p95 distributions
  of observed metrics. One repeat cannot establish stability. Missing
  observations are excluded and their count is shown. Ratio distributions
  describe runs, not a token-weighted aggregate across the whole suite.

Offline checks: `cargo test -p palmcode-bench -- --test-threads=1` and
`cargo test -p palmcode-agent --test tool_loop -- --test-threads=1`.
The fixture/verifier and numeric-metrics tests do not use a live endpoint.
See ADR 0029 and the closure table for remaining acceptance gaps.

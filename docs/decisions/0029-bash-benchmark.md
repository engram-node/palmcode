# 0029: Paired bash benchmark with numeric diagnostics

Use the existing Engine, permission classifier, tools and HostExecutor.
Do not create a second agent loop. An optional EventSink request-metrics
callback reports numeric statistics once per model iteration; Android's
existing sink keeps the default no-op, so its FFI protocol is unchanged.
MessageFinished only exposes the last prompt's usage, which is insufficient
for total benchmark tokens. The callback preserves per-request usage and
reports missing values as unknown instead of estimates.

Run each deterministic fixture in a new trusted temporary workspace with
auto-approve off and on. Both modes answer remaining Ask events allow-once;
count those events before answering. Hard asks still count with auto-approve
enabled. This measures policy friction, not human response time. Alternate
mode order across repeats; do not claim provider-cache isolation. Deterministic
fixtures/verifiers do not make provider sampling or latency deterministic.

Use immutable verifier strings outside the model workspace, executed by
HostExecutor with a short deadline and the same environment sanitation.
Require observed errors for recovery fixtures and an observed timeout for
the timeout fixture, in addition to filesystem checks. Script/test inputs
explicitly marked immutable are checked too. File outcomes are graded;
the benchmark does not prove that the model chose a requested utility such
as awk instead of an equivalent implementation.

Traces contain task IDs and numeric statistics only: no commands, previews,
provider URLs, model aliases, request bodies, output or keys. Keep the key
in a zeroizing host wrapper; never pass it to verifiers/child environments.
Refuse trace overwrites. Retaining full command traces for debugging was
rejected because model text can echo sensitive material.

Report per-repeat pass rates and population standard deviation, per-task
ASKS, per-run metrics, and distributions of observed request timing. Missing
usage/cache is null; decode windows shorter than a quarter second are not
reported as meaningful throughput. This is a host benchmark, not Android
latency or process-kill acceptance. Live runs are reserved for the maintainer.

Reference solutions exposed the executor's deliberate empty diff.external
override: plain git diff tries to execute an empty external program. Keep
that protection and use --no-ext-diff --no-textconv for built-in diffs in
the fixture, verifier and prompt catalog. Relaxing repository-helper
protections merely to make the benchmark pass was rejected. On Windows,
nested sh must resolve to MSYS2 through the host process PATH too.

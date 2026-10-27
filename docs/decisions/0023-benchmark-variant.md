# 0023: Isolated mock-only benchmark variant

Create bench with release settings, a separate application id and debug
signing. Its activity, runner service and argument tests live only in bench
source sets. The runner has no Compose UI or provider-store access; it
configures an empty credential in memory, uses a private workspace and runs
scripted turns against a numeric loopback HTTP endpoint. The model field
comes from PALMCODE_LLM_MODEL, or is empty for the model-agnostic mock.
No actual provider configuration is read or written.

The explicit URL intent extra is a benchmark-only exception to ADR 0013's
endpoint environment rule, as requested. Validate scheme, numeric loopback,
port, turn count and absence of credential/query/fragment fields. Do not
print endpoint, model, arguments, errors or tool output. Rust emits existing
OVERHEAD phase measurements through alog; the runner adds PHASE wall-time
and tool-count lines. Wall time is not presented as overhead.

Preserve non-exported components. The requested direct adb am-start command
cannot launch a non-exported activity from the shell UID. Use the debug
package's run-as UID for a pending device check instead. The bench variant
is debuggable to permit run-as; AGP consequently disables JVM optimization,
although the Rust library remains the release build. Exporting a command
execution entry point or weakening release packaging was rejected.

The no-display activity starts a bounded, non-sticky foreground runner with
a wake lock, Stop action and dataSync timeout cancellation. Benchmark
artifacts are not part of the F-Droid release build.

Suggested maintainer check (not executed):

```
adb reverse tcp:8787 tcp:8787
adb shell run-as dev.palmcode.bench am start -n dev.palmcode.bench/.BenchActivity --es url http://127.0.0.1:8787/v1 --ei turns 50
adb logcat -s palmcode:I
```

Start the existing host scripts mock with the desired SSE fixture sequence
beforehand. Repeat tool/final-response fixtures for each desired tool turn;
the mock reuses its last response when the script list is exhausted.

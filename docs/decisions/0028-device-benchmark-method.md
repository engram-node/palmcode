# 0028: Reproducible device benchmark entry and timing

Keep both benchmark activities non-exported. A bench-source-set-only
instrumentation entry starts them from adb; release has no instrumentation
or benchmark classes. Direct shell activity launch cannot satisfy the
non-exported requirement. The earlier run-as proposal remains unverified.

Use the existing mock over a numeric loopback adb reverse mapping, with an
empty credential. Each fresh session executes a read of benchmark.txt and
a final text response. Keep the benchmark database between invocations,
but generate tool-call IDs unique across invocations. Reusing fixture IDs
caused an initial incomplete run to fail closed on the database constraint;
discard it rather than clearing persisted state to hide the failure.

The headless runs measure independent short sessions, not growing context.
The optional UI uses the actual transcript MessageBubble and tool cards in
a LazyColumn. It temporarily wakes/keeps the benchmark activity visible;
it does not change device settings or require taps. This exercises read
cards, not scrolling or killing a live bash tool. Keep those manual checks
pending. Bench uses optimized native Rust but debuggable, unoptimized JVM
code; it is not a production-release/provider benchmark.

Pace synthetic content events on absolute deadlines. Sleeping for a fresh
interval after every write accumulated host timer drift. Each paced final
response contains 300 identical content events plus terminal framing; an
event is a synthetic token, not a tokenizer measurement. Preserve measured
server stream durations with device timings.

Report nearest-rank percentiles of complete runs only. Reject missing gap
or turn samples; retain numeric per-turn samples and artifact hashes.
Collect logcat only for the benchmark PID/tag, never clear global logs,
and collect gfxinfo as text. The runner removes provider environment
variables from child processes and cleans up its own mock/reverse mapping.

The inherited gap endpoint preceded serialization. ADR 0027 defines the
corrected dispatch endpoint. Older samples are explicitly marked
pre-dispatch and excluded from the final budget comparison. Measurement
corrections are not performance optimization iterations.

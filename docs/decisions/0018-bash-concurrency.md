# 0018: Bound active bash executions across sessions

Use one process-wide four-slot gate immediately before the bash executor.
Hold the slot through executor cleanup; release it with an RAII guard on
success, error, or unwind. Waiting calls check cancellation at most every
50 ms and never spawn after cancellation is observed. Execution timeout
starts at execution, not while waiting for a slot.

A per-session gate was rejected because multiple sessions could exceed the
application budget. A new async dependency was unnecessary for the existing
blocking tool execution path. This is not a descendant-process limit or a
guarantee against Android process reclamation. Scheduling is not FIFO.

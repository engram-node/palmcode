# 0026: Bounded SQLite writer with durable barriers

One named worker owns the SQLite connection from open through close. All
operations enter its FIFO, bounded to 64 jobs; producers backpressure at
capacity. Drain up to 32 adjacent writes into BEGIN IMMEDIATE/COMMIT without
an artificial batching delay. Existing compound operations use savepoints
inside that transaction. Reads and maintenance are barriers on the same
worker, avoiding stale projections and a second connection's cache policy.

Streaming snapshots enqueue owned cumulative content. Tool intents are
batched into one acknowledged transaction before any tool executes. Final
message acknowledgement follows commit; a failed tool-intent/outcome or
final commit reports an error rather than continuing as if durable. Close
drains and joins the queue. Any failed write batch rolls back all its jobs
and fails closed until reopen; queued snapshots cannot silently disappear
behind a successful later barrier. A process crash may lose unacknowledged
snapshots, as it may lose the in-flight snapshot in the previous design.
Acknowledged tool intents and outcomes retain unknown-outcome recovery and
are never automatically replayed.

Keep WAL, foreign_keys=ON, busy_timeout=5000 and explicitly synchronous=FULL.
FULL syncs the WAL at commit; NORMAL can lose committed transactions on
power loss. Trading that guarantee for lower latency was rejected. Hardware
must still honor syncs. Keep checkpoint defaults rather than introducing
unmeasured checkpoint tuning. In-memory tests use foreign keys as well.

SQLite references: https://www.sqlite.org/pragma.html#pragma_synchronous
and https://www.sqlite.org/wal.html

Tests cover FIFO snapshot/event order, close/reopen recovery, entire-batch
rollback and sticky failure, queue backpressure, worker ownership, pragma
values, and prevention of tool execution after persistence failure.

# 0025: Keep foreground startup unblocked during turn submission

Native send can synchronously wait for token counting before registering the
turn future. Calling it on Android's main thread can delay the foreground
service's notification beyond its startup deadline. Submit native work on
Dispatchers.IO from a process-owned supervisor scope; keep run tracking and
service lifecycle changes on the main thread. A screen-owned coroutine was
rejected because navigation should not cancel a submitted run.

Retain Stop requests while submission is pending and signal native Stop
again after submission returns. Terminal events clear the pending request;
a later turn gets fresh state. Submission failure cancels native work,
releases run lifetime and shows a generic error without logging provider
or exception contents. A JVM regression covers pending Stop and renewal.

Release lint crashed on chained synthetic Java notification properties in
Kotlin analysis. Explicit platform getters avoid the crash without disabling
lint. Final debug, bench and release assemblies, release vital lint, and
both JVM test variants pass.

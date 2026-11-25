# 0019: Restore tool cards through transcript records

Include persisted tool calls in the existing transcript FFI response, with
an 8192-character output tail per card. Keep the original status and duration;
show interrupted calls as outcome unknown. Do not read spill files during
transcript loading. Replaying events was rejected because persisted tool
records already provide the authoritative result and events are not a log.

Bash cards use a bounded lazy list. Dragging pauses follow until explicitly
resumed. The Kill action stops the whole turn using the existing cancellation
protocol, including the shared bash cancellation token. Stopping discards
the cached tool context so a subsequent turn gets a fresh token. Session
permission grants in that context are consequently cleared on Stop; this
is conservative but may cause extra approval prompts. Individual-command
cancellation is not introduced in this workstream.

Diagnostics runs fixed shell/ripgrep/git smoke commands through the existing
executor and four-slot gate, with a ten-second timeout and an allowlisted
environment. It uses an app-private directory, never the user's workspace.

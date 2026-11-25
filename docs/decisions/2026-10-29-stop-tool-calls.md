# Dispatch calls independently of the finish label

New device diagnostics showed two empty assistant turns whose HTTP streams
contained one named, completed tool call each, with finish category `stop`.
The agent dispatched calls only when finish_reason equaled `tool_calls`.
Consequently, it discarded valid calls and finalized a textless success. The
earlier fragment counters alone had not established that calls were incomplete.

Use the accumulated tool calls to decide whether the agent continues through
tool execution. A provider finish label does not erase the requested calls.
Dispatch still passes through the existing argument validation, permissions,
approval protocol and tool runtime. Without a workspace, display the existing
missing-workspace explanation instead of an empty answer. Preserve tool results
and cards, and request the follow-up model response through the ordinary loop.

Reject provider-specific model-name checks and accepting only the `tool_calls`
finish label. Tests reproduce two normal replies followed by a stop-labeled
read call and final answer, stop-labeled write denial, and the workspace-free
case. Do not replay previously dropped calls from historical empty turns.

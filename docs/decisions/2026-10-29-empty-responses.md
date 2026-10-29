# Compatible response shapes and empty-response errors

Device metadata showed three completed assistant turns with nonzero token
counts and no stored text, reasoning or error parts. UI inspection also showed
only metrics. The original response envelopes were not captured, so their exact
shape remains unverified; no live request was replayed during diagnosis.

The parser previously accepted only `choices[0].delta.content` strings while
silently ignoring other content shapes. Accept a complete `message` inside SSE
and typed text parts as well. Prefer an actual delta payload, otherwise use a
message snapshot; append only the suffix missing from previously streamed text.
Do not stringify arbitrary objects or media into user-visible answers.

Reject a completed stream containing no visible text, reasoning or tool calls
with an explicit error. Usage counts do not prove an answer was received. Such
decode errors are not retried automatically. Emit FirstToken only on actual
text/reasoning/tool payload, excluding role and usage envelopes. Keep numeric
stream byte counts and whitelisted finish categories in Android logs, with no
response contents. Previously saved empty turns display an explanatory label.

Rejected: showing a successful metrics-only bubble, inventing response text from
usage, and logging raw provider envelopes to diagnose compatibility. Historical
missing replies cannot be reconstructed from token counts. Live compatibility
still needs an owner retry; additional unsupported formats will produce an error.

# Complete streams and validate tool-only replies

Numeric device logs showed three replies with zero text/reasoning bytes and
one tool-call delta each. Their persisted assistant messages had token counts
but no text, error or tool cards. The client had counted any tool fragment as
a usable reply, while the agent accumulator discarded nameless fragments.

Validate tool-only success against accumulated named calls, not fragment count.
Empty or whitespace-only function names are incomplete. Report an explicit
decode error when neither visible output nor a named call exists. This failure
is not automatically retried; no tool can execute from a nameless fragment.
Persist the error via the ordinary agent protocol instead of a metrics-only
success. Record only counts of names, complete calls and EOF-flushed events.

Also consume the SSE parser's pending event on EOF: the parser supports
unterminated final lines, but the HTTP client previously never called finish.
This can lose final answer text or the function name that completes a call.
Conversely, stop consuming the HTTP body immediately on the SSE [DONE] marker;
waiting for socket/body EOF can stall or time out an already completed answer.

The original provider envelopes were not recorded. EOF loss and late body
closure are reproduced by mocks, not yet proven to explain that endpoint's
exact wire behavior. Owner-provider retry remains pending. Reject raw envelope
logging, automatic paid retries and treating token counts as answer content.

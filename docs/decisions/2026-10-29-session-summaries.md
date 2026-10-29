# One-line conversation titles

Replace timestamp-based chat titles with a short LLM-generated conversation
summary. New sessions start as `New chat`. After a successful textual reply,
Android schedules title generation on Dispatchers.IO. Legacy numeric titles
are backfilled one at a time after active chat runs finish. Failed attempts
are not retried on every list refresh; a process restart or another completed
reply permits retry. Empty chats, interrupted-only chats and existing descriptive
titles make no request. The UI hides legacy numeric placeholders while waiting.

Use the configured fast model, falling back to the main model when blank.
Send only the first user prompt (2000 characters) and latest completed assistant
text (1000 characters), with a dedicated title instruction, no tools and a
256-token output limit. Exclude reasoning and tool output. Title generation
uses the existing authenticated client; credentials never enter the prompt.
Allow at most 90 seconds, including retries, to accommodate slow compatible
endpoints. Failures do not affect transcript or turn status. This introduces
one additional successful model request per automatically titled conversation;
backfilling old conversations also consumes provider tokens.

Store the normalized, at-most-80-character one-line title through the existing
bounded SQLite writer. A compare-and-set update preserves a title changed while
the network request was in flight. Do not add metadata replies to the transcript
or event sequence. The caller refreshes session projections after persistence.

Rejected: generating the title before the main response (adds visible latency),
using only prompt truncation (not an LLM summary), sending full history (unbounded
cost and unnecessary tool content), and parallel history backfills (provider load).
Title work never gates Send, though an already-started title request can overlap
a subsequently started chat. Process death may interrupt optional metadata work;
no foreground service or wake lock is held for title generation.

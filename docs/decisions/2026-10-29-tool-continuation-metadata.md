# Preserve provider continuation metadata on tool calls

After a successful bash command, the owner endpoint rejected the follow-up
request with HTTP 400 for a missing thought_signature. Google documents that
the OpenAI-compatible interface requires echoing the tool call's
extra_content.google.thought_signature unchanged with the function response
(Generate Content guide, Thought signatures, OpenAI compatibility section).
Palmcode had extracted only call ID, function name and arguments.

Carry opaque extra_content JSON through parsing, accumulation and assistant
tool-call serialization. Metadata may arrive before or after argument deltas;
keep it attached to the correct call index. Do not interpret it as executable
arguments, user-visible reasoning or model configuration. Omit the field when
absent so existing request serialization stays unchanged. Debug implementations
redact its contents, including stored tool rows.

Persist metadata alongside each pending tool intent in the same bounded-writer
transaction before execution. Add a nullable tool_calls.extra_content column by
an additive idempotent migration. Reconstruct signed calls from storage for
normal and interrupted history without replaying already executed tools.

Rejected: dropping provider extension fields, inventing bypass signatures,
disabling validation, or retrying a successful command to obtain a new signature.
Existing calls written before this fix have no recoverable continuation state;
retain their tool cards and outcomes. For an affected conversation, start a new
chat and refer to the existing checkout rather than cloning it again.

# Omit disabled continuous-usage extension

A physical-device chat failed with HTTP 400: the server rejected the unknown
`continuous_usage_stats` field inside `stream_options`. The builder sent that
field even when the feature was disabled (`false`). This is a server extension,
so disabling it must omit it from the request entirely. Keep standard
`include_usage: true`; send `continuous_usage_stats: true` only on explicit opt-in.

Do not infer support for this request extension merely from receiving multiple
standard usage chunks during Test connection. Report it as supported only when
the extension was actually requested and accepted, so probing cannot silently
enable an untested field for the next saved profile.

Keep identical serialization for probe and agent requests via their shared
builder. Preserve usage accounting and opt-in support. Retrying every HTTP 400
with reduced options was rejected: it hides configuration errors and adds an
unnecessary failing request. Regression coverage checks both flag states and
the actual mock-server wire body for a chat containing tools. No live-provider
request is necessary to reproduce or verify the request-shape correction.

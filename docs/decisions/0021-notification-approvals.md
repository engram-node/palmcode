# 0021: Notification actions share the approval protocol

Use an explicit non-exported receiver and immutable pending intents. Match
request id, session id and a random process nonce against a currently pending
approval before resolving it through the same FFI entry point as the sheet.
Old process notifications never resume or approve a command; clear them at
startup. Unknown actions, stale ids and missing fields fail closed. No
project grant is available from a notification.

Use a high-importance channel only while backgrounded. Foreground entry
cancels notifications and exposes the existing sheet. Notification text is
generic to avoid exposing command arguments or workspace paths on the lock
screen. Tapping opens the session. Notification denial leaves the sheet as
the approval path. Full-screen intents and automatic retries were rejected.

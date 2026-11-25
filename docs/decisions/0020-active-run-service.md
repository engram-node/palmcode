# 0020: Foreground lifetime for active turns

Use `dataSync` for user-started agent turns: local workspace file processing
and transfer to the user-configured endpoint fit its documented use cases.
Declare its Android 14 permission and type. `remoteMessaging` is for text
message transfer between devices; `shortService` cannot cover long turns;
`specialUse` is unnecessary when an existing type fits.

Source: https://developer.android.com/develop/background-work/services/fgs/service-types

Start from the foreground Send/Resume action before submitting the turn.
Stop when all tracked turns finish, fail or are interrupted. Hold one partial
wake lock only for this lifetime. The notification Stop action cancels every
active turn, including bash. Do not persist service state or automatically
restart/retry interrupted work. On the Android dataSync timeout, cancel
turns and stop the service immediately. Doze can still defer networking;
existing network deadlines report an interrupted/error result for manual
continuation. A wake lock is not a network exemption.

Request notification permission once and explain battery optimization once;
opening battery settings is optional. Do not request a direct exemption.
Notification denial does not prevent foreground service execution. Device
verification of timeout, wake-lock release and screen-off behavior is pending.

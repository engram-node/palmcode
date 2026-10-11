# NOTES — carried-forward housekeeping

Not code yet. Each item names the milestone that consumes it.

## UI-driving / device-session gotchas (any milestone that adb-drives the phone)

- The phone display sleeps during long adb runs and taps/dumps get weird
  (half-painted screens, keyevents landing in other apps). Set
  `settings put system screen_off_timeout 600000` + `input keyevent KEYCODE_WAKEUP`
  + `wm dismiss-keyguard` at the start of every session. Restore the timeout
  afterwards.
- `input text` needs `%s` for spaces; some `%`-combos get eaten by the IME.
  Prefer short messages.
- uiautomator XML escapes non-ASCII as entities (`·` → `&#183;`); scripts
  saved as ANSI (no BOM) make PowerShell mangle XPath literals containing
  them. Match on ASCII-only substrings (timestamps `:` work for status rows).
- `<` in TextButton labels is stored as `&lt; Chats` in the dump; XPath with
  a literal `<` will never match. Use `contains(@text,'Chats')`.

## M2 (chat UX follow-ups)

- A long streaming message fills the viewport: the metrics/interrupted/Resume
  footer sits below the fold until the user scrolls. Consider pinning the
  footer or auto-scrolling to the message END when a turn finishes.
- `MessageFinished.decode_tps` can show silly values (7202 tok/s) when the
  whole decode lands in ~10 ms — clamp or suppress when decode window < 250 ms.

## M3 (userland wiring)

- **busybox applet symlinks must be named after applets**, with the binary
  itself shipped/aliased as exactly `busybox`. busybox dispatches on
  `argv[0]` basename; invoking the binary as e.g. `palmcode-busybox`
  fails with "applet not found". Ship as `libbusybox.so` (extract-to-disk
  packaging like the exec probe) and symlink `files/bin/busybox` ->
  `lib/.../libbusybox.so`, plus one symlink per applet
  (`files/bin/sh -> busybox`, ...). Verified on-device 2026-10-11
  (motorola edge 50 neo, Android 16): 367 applets run when named
  correctly, fail otherwise.
- **Nested exec regression pass before M3 ships**: `find | xargs`,
  subshells, `sed -i`, `tar` pipelines on a second physical device —
  ideally Samsung plus one Xiaomi/Oppo — and on one Android 14-or-older
  device or emulator image. Current on-device evidence is a single
  motorola device on Android 16 only.

## Release packaging (M3+; recorded now)

- **Release APK is arm64-v8a only.** x86_64 userland binaries must be
  excluded from release builds (they exist only for emulator dev/debug).
  Current state: release arm64 APK contains no userland binaries at all
  (verified by APK listing 2026-10-11; userland still lives outside the
  repo as spike artifacts) — the exclusion must be re-verified once M3
  wires userland into `jniLibs`. Release arm64 reference size:
  22.04 MB unsigned (splits config emits x86_64+universal for dev;
  restrict `assembleRelease` outputs to arm64 when userland lands — an
  x86_64 release with userland would needlessly carry rg/busybox x86_64).

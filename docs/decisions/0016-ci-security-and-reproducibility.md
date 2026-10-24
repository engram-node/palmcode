# 0016: CI security updates and BusyBox reproducibility

Date: 2026-10-24. Status: accepted.

PR #7 run 39911955809 passed Rust formatting, Clippy and tests, but
`cargo deny check` rejected rustls 0.23.45 (RUSTSEC-2026-0346 through
0349) and rustls-webpki 0.103.15 (RUSTSEC-2026-0382). Update to
rustls 0.23.46 and rustls-webpki 0.103.16 with their published checksums
and vendored sources. Keep the advisory gate; ignoring the advisories
would leave affected dependencies in the shipped app.

The current advisory scan also flags Tokio 1.53.2 (RUSTSEC-2026-0386,
AsyncFd I/O safety). Update to the patched 1.53.3 release in the same
lockfile and vendor refresh.

The same run built both release APK sets successfully. The extracted
file comparison differed only for libbusybox.so on both ABIs. BusyBox's
scripts/kconfig/confdata.c embeds the configuration time in
AUTOCONF_TIMESTAMP, consumed by libbb/messages.c in the version banner.
Set KCONFIG_NOTIMESTAMP=1 before defconfig and oldconfig to omit that
time using upstream's supported switch. Keep byte-exact APK comparison.

Rejected alternatives: a fixed SOURCE_DATE_EPOCH would retain an artificial
banner date; ignoring native libraries in the comparison would hide actual
differences. No upstream source patch or binary artifact is needed.

## Device smoke test follow-up

The scripted bench server must set its std TCP listener nonblocking before
passing it to Tokio, matching the paced bench server. A blocking listener
stalled the initial local device test; the setup is now consistent.

The debug-to-minified-release upgrade test exposed stale applet links:
Android moved the native library directory while existing app-private
symlinks retained the previous APK path. Refresh symlinks whose targets
differ from the currently loaded library directory. Preserve ordinary
files at those paths. A Unix regression test simulates removal of the old
library, then checks the updated link and an idempotent second call.
Clearing app data or manually deleting the farm would hide this upgrade
failure and discard persistent state; neither is the repair.

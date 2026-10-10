# 0004: Userland version pins (busybox 1.38.0, git 2.56.0, ripgrep 15.2.0)

Date: 2026-10-10. Status: draft — sha256 values are filled during the M3
spikes (spike A: git cross-build feasibility).

## Context

Section 5 of the brief requires busybox, ripgrep, and real upstream git built
from pinned, checksum-verified source with the NDK.

## Picks (latest stable, 2026-10-10)

- **git 2.56.0** (kernel.org tarball, 2026-09-28)
- **busybox 1.38.0** (busybox.net tarball, 2026-05-13). BusyBox's own news page
  labels the release "unstable", but the 1.38 series is the current stable
  branch (post-1.37) and Debian/busybox.net ship the tarball; if spike A finds
  a defect, fall back to 1.37.0 and amend.
- **ripgrep 15.2.0** (GitHub release tarball, 2026-07-15): gitignore-matching
  fixes; built as a Rust binary with the NDK toolchain, not via the prebuilt
  release binaries.

## Build-system notes for spike A

- git cross-compile with NDK: configure/make with
  `--host=<triple>`, `NO_PERL`, `NO_GETTEXT`, `NO_ICONV` (bionic lacks iconv),
  `NO_TCLTK`; HTTPS remotes need curl + OpenSSL/BoringSSL cross-built first —
  evaluate effort vs. the libgit2 fallback at spike time.
- busybox for Android needs bionic-friendly config (no static glibc, disable
  applets needing unavailable headers) — a defconfig is part of M3.

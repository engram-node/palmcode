#!/usr/bin/env bash
# Regenerate the Rust command catalog from the reviewed applet allowlist.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
printf '%s\n' \
  '//! Curated BusyBox applets; generated from userland/busybox-applets.txt.' \
  '//! Checked against the compiled applet table by userland/build.sh.' \
  '' 'pub const BUSYBOX_APPLETS: &[&str] = &['
awk 'NF && $1 !~ /^#/ {printf "    \"%s\",\n", $1}' "$ROOT/userland/busybox-applets.txt"
printf '%s\n' '];'

# 0005: Project license GPL-3.0-only; JNA under its Apache-2.0 option

Date: 2026-10-10. Status: accepted.

## Decision

License the project itself **GPL-3.0-only**.

Rationale: we distribute GPL-licensed programs (busybox GPL-2.0+, git GPL-2.0)
inside the APK as separate executed binaries. GPL-3.0 for the app keeps our
licensing unambiguous and matches Termux (GPL-3.0), the closest distribution
analog. Dependencies must remain permissive-FOSS only (see `core/deny.toml`),
which is compatible with a GPL-shaped application.

## Alternatives rejected

- Apache-2.0: common for Rust/Cargo ecosystems and what `cargo new` defaults
  to, but offers no protection for the app and no benefit here — BYOK app,
  no SDK ambitions.
- MIT: same reasoning as Apache-2.0.

## JNA

JNA is dual-licensed LGPL-2.1+/Apache-2.0. The UniFFI Kotlin runtime depends
on it; select the Apache-2.0 option and list it in THIRD_PARTY_LICENSES (M5).

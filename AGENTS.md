# AGENTS.md

Agent-facing project notes. Humans: read `README.md`.

## Rules that will get contributions rejected

- **No binaries in the repo.** No checked-in `.so`, `.jar`, executables, or
  prebuilt libs (F-Droid scanner). The Gradle wrapper jar is the single
  sanctioned exception. Everything else (Rust std is fine — it is vendored
  source in `core/vendor/`, userland binaries) must be built from
  checksum-verified source at build time.
- **No proprietary or non-free dependencies.** Run `cargo deny check` before
  merging. Gradle deps only from Maven Central / Google with FOSS licenses.
- **API keys never touch disk, logs, or child-process environments.**
  Keystore-encrypted at rest (Kotlin), in-memory only in Rust, zeroize on
  drop, redact in `tracing`.
- **Work milestone by milestone.** Do not start M(n) until M(n-1) acceptance
  criteria pass. Never claim Android behavior without running it on an
  emulator or device; keep `docs/VERIFY.md` accurate.
- **Record non-obvious decisions in `docs/decisions/`** including rejected
  alternatives.

## Build & test

```
cd core && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
./gradlew assembleDebug        # full app incl. Rust + bindings
```

Rust workspace lives in `core/`. Android app in `android/`. Toolchain and
dependency pins: `docs/decisions/0002-toolchain-pins.md`,
`userland/versions.env`, `gradle/libs.versions.toml`, `rust-toolchain.toml`.

## Conventions

- Commit messages/docs: terse, factual. No emoji.
- Rust: edition 2024, `core/vendor` committed for offline builds, Cargo.lock
  committed.
- Events: the core emits typed events with monotonic sequence numbers; the UI
  is a reducer over them. Keep the FFI boundary coarse — a handful of calls
  plus one event stream; batch deltas, never per-token FFI calls.
- Executors: tools never call `std::process` directly; they go through the
  `Executor` trait in `core/exec`.

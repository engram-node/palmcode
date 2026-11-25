# 0030: Update patched Rust dependencies

Date: 2026-11-24. Status: accepted.

The advisory gate identified bundled SQLite before 3.54.0 through
`libsqlite3-sys` 0.30.1 (`RUSTSEC-2026-0459`) and a stack overflow in
`url::Url::origin` through `url` 2.5.8 (`RUSTSEC-2026-0592`).

Update `rusqlite` to 0.41.1, which uses `libsqlite3-sys` 0.39.1 and bundled
SQLite 3.54.1, and update `url` to 2.5.9. Refresh `Cargo.lock` and committed
vendor sources together. Keep the advisory gate enabled.

Rejected alternative: ignoring either advisory would keep vulnerable code in
the application and contradict the repository's dependency policy.

# 0008: git userland — source, sizes, exec-path handling, TLS plan

Date: 2026-10-10. Status: verified build+run (ADR 0006 has the spike recipe);
this ADR records what ships and how HTTPS is handled.

## Source & reproducibility

- `git-2.56.0.tar.xz` from kernel.org (stable release, not an rc),
  sha256 `26c56c2...d789d3` (full hash in `userland/versions.env`,
  verified against the signed `sha256sums.asc` at download time).
- Built from source only; no prebuilt binaries enter the repo.

## Git's Rust-by-default component (2.56)

`make` defaults to building the experimental Rust object
(`ifndef NO_RUST → WITH_RUST, cargo build of libgitcore.a`). We set
`NO_RUST=YesPlease`. Rationale:

- The Rust component is opt-in acceleration; the C build is the complete,
  canonical git. Nothing user-visible is lost.
- Cross-compiling git's Rust parts would require a full cargo cross
  toolchain inside the MSYS2 userland build for a component we don't use.
- Revisit if upstream makes a Rust-compiled path load-bearing.

## Measured sizes (2026-10-10)

| Binary | ABI | unstripped | stripped |
|---|---|---|---|
| git 2.56.0 | aarch64 | 19.8 MB | **3.5 MB** |
| git 2.56.0 | x86_64 | 19.1 MB | **3.75 MB** |
| M0 release APK (FFI + Compose + JNA, both ABIs) | — | — | 23.5 MB |

Pending measurement in M3: busybox 1.38.0 (~1–1.5 MB typical), ripgrep 15.2.0
(~4–6 MB typical). APK budget < 30 MB single-ABI: reachable; JNA will need
ABI pruning (it ships armv7/mips/x86 dispatch libs today) — task noted for M6.

## Baked-in paths: GIT_EXEC_PATH, templates, maintenance

From the spike:

- `E..`-prefix paths from configure are meaningless on-device
  (`C:/msys64/usr/...git-core/templates` — produces a harmless warning).
- M3 solution: the executor sets `GIT_EXEC_PATH` / `GIT_TEMPLATE_DIR` (or a
  suitable `--prefix`/config.mak-inc at build time) into the app's private
  dir (`files/bin`, `files/share/git-core/templates`). The
  "cannot run maintenance" warning disappears once `GIT_EXEC_PATH` resolves.
- No code patch required — configuration only.

## TLS plan for HTTPS clone/push

| Option | size/ABI (est) | Notes |
|---|---|---|
| curl + OpenSSL static | +2.5–3 MB | upstream default pairing; OpenSSL 3.x is Apache-2.0; biggest build time |
| **curl + mbedTLS static** (chosen) | +1–1.5 MB | smallest solid option; Apache-2.0; curl supports mbedTLS natively |
| curl + rustls | +1 MB | rustls-for-libcurl is experimental in curl docs; extra moving parts |
| no HTTPS in CLI git; gix in core does network | +0 | contradicts "real git CLI" for remote ops; keep as documented fallback |

Decision: **curl + mbedTLS, both statically linked into the userland build**,
source-pinned in `versions.env` with sha256 like git. Spike in M3; measure
the delta against the numbers above.

F-Droid impact: all components FOSS and source-built — no scanner issues
expected; build time grows by a few minutes; no extra binaries in the repo.

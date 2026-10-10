# 0009: Project relicensed to Apache-2.0; GPL binaries remain aggregate works

Date: 2026-10-11. Status: accepted. Supersedes ADR 0005's license choice
(JNA analysis retained).

## Decision

License all palmcode source (Rust workspace + Android app) **Apache-2.0**.
Workplace crates carry `license = "Apache-2.0"`; `deny.toml`'s
`private = { ignore = true }` carve-out (which existed to let the then-GPL
workspace crates pass) is dropped — our crates now pass the allowlist on
their own.

## Why the change

Spec amendment 2026-10-11: the project must be Apache-2.0. The move is safe
because ADR 0005's reason for GPL (bundling busybox/git) does not actually
require the app license to be GPL: busybox and git in the APK are
*independent and separate works* — distinct executables, invoked via
`execve`, not linked, modified, or derived from. GPL-2.0's scope extends to
works "based on the Program", and explicitly excuses "mere aggregation of
another work not based on the Program with the Program on a volume of a
storage or distribution medium". This is the same arrangement every Linux
distribution (and Termux) relies on.

## Continuing obligations (how we keep them)

- **busybox (GPL-2.0)** and **git (GPL-2.0)**: license text +
  source-code offer must accompany the binaries. We satisfy this with:
  - `THIRD_PARTY_LICENSES` (repo root, shipped as an app asset, viewable
    in-app from M1): full license texts + per-component source offer.
  - `userland/versions.env`: pinned upstream tarball URLs + sha256 that the
    build downloads and verifies — no binaries committed to the repo.
  - F-Droid builds everything from source by policy; distribution-side
    source availability is therefore inherent.
- We do not modify busybox/git/curl/mbedTLS upstream source. Config and
  *patches* (if ever needed) are GPL-compatible contributions and live in
  `userland/patches/`.

## Alternatives rejected (again)

- Staying GPL-3.0: contradicts the spec; no requirement forces it (above).
- Dual-licensing: pointless complexity; we accept external contributions
  under the single inbound license via the Apache-2.0 inbound=outbound
  default.

## Consequences

- More permissive for potential downstream reusers/SDKs; no patent-
  retaliation clause change for us (Apache-2.0 has one; GPLv3 had a stronger
  one — accepted trade).
- `deny.toml` allow-list is unchanged: bundled *Rust dependencies* must
  still be permissive (MPL-2.0 UniFFI is file-level copyleft, allowed).
- ADR 0005 kept on record with status "superseded".

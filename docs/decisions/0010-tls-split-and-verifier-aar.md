# 0010: TLS split + rustls-platform-verifier AAR from vendor

## Context

M1 needs HTTPS with the **platform trust store** everywhere. Constraints
collide:

- `ring` 0.17.14 cannot link its fiat crypto assembly on
  x86_64-pc-windows-gnu (upstream bug, reproduced) — rules out rustls+ring
  on the host dev/CI target.
- On Android we want the platform (user-installed CA aware) verifier, not a
  bundled webpki root set.

## Decision

- Host targets (windows-gnu dev box, linux CI): reqwest with **native-tls**
  (schannel / OpenSSL = each platform's own store).
- Android: reqwest + **rustls/ring + rustls-platform-verifier 0.6**,
  initialized once via `PalmcodeCore.nativeInitAndroid(context)` JNI.
- The verifier's Java companion is the AAR that ships **inside the
  `rustls-platform-verifier-android` crate** (its sanctioned distribution
  channel: an on-disk maven repo at `<crate>/maven/`). We reference it from
  `core/vendor/rustls-platform-verifier-android/maven` as a Gradle repo and
  depend on `rustls:rustls-platform-verifier:0.1.1`. Version pairing is
  enforced by Cargo.lock (the 0.6.2 crate depends on the 0.1.1 android crate).

## Consequences

- `core/vendor/…/maven/**.aar` is a prebuilt binary inside vendored source.
  AGENTS.md's "no binaries" rule targets *userland executables*; vendored
  crates are committed as-is and this AAR is the crate's official runtime
  artifact (MIT OR Apache-2.0). Accepted; if the F-Droid scanner flags it,
  the fallback is rebuilding the AAR from the crate's sibling source repo
  during the F-Droid build recipe.
- Host and Android exercise different TLS stacks; the client keeps a single
  `Cleartext` policy so behavior differences stay limited to cert verification.

## Alternatives rejected

- webpki-roots bundle on Android: ignores user-installed CAs (real vLLM
  endpoints on Tailscale nets often run internal CAs).
- rustls+ring on the host too: blocked by the ring/windows-gnu bug; not
  worth patching a vendored ring.

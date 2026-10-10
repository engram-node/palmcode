# 0006: Git 2.56.0 cross-builds with NDK r30 via upstream configure (spike A result)

Date: 2026-10-10. Status: verified (build + emulator run). Scripts:
`userland/spike/git-configure.sh`, `userland/spike/git-make.sh`.

## Verdict

**Real upstream git is feasible for Android; no libgit2 fallback needed.**

aarch64 AND x86_64 `git` 2.56.0 binaries linked clean (ELF PIE, interpreter
`/system/bin/linker64`, NDK r30, API 29). The x86_64 binary ran on the
emulator: `git --version` → "git version 2.56.0", and
init/add/commit/log/status/diff all worked. Size: ~19 MB unstripped,
~3.9 MB after `llvm-strip`.

## Recipe (full record for M3's build.sh)

- `git-2.56.0.tar.xz` (kernel.org, sha256 pinned in `userland/versions.env`).
- `./configure --host=<triple>` with
  `CC="clang --target=<triple>29" AR=llvm-ar RANLIB=llvm-ranlib STRIP=llvm-strip`
  and NDK prebuilt `bin/` on PATH.
- Options: `--without-iconv --without-curl --without-expat --without-tcltk
  --without-python --without-libpcre2`.
- Cross-compile run-test overrides:
  `ac_cv_snprintf_returns_bogus=no ac_cv_fread_reads_directories=no`.
- `config.mak` knobs (configure does not probe these):
  - `HAVE_DEV_TTY=YesPlease` — without it, `compat/terminal.c` compiles the
    no-termios fallback which calls `getpass()`, missing from bionic.
  - `NO_RUST=YesPlease` — git 2.56's opt-in-experimental Rust component is on
    by default and needs a full cargo cross setup; the C core is the product.
  - `BASIC_CFLAGS += -DPTHREAD_CANCEL_DISABLE=0 "-Dpthread_setcancelstate(a,b)=0"`
    — bionic has no pthread cancellation; the two call sites are never
    semantically needed there, stubbing to success is correct.
- Gotchas documented: `make distclean` deletes `configure` (re-extract from
  tarball instead); `--without-perl` no longer exists in configure (host perl
  is required to build, fine); upstream `getpass` absence does NOT fail
  configure (declaration missing, symbol unused when HAVE_DEV_TTY).

## Open items deferred to M3/M5

- **HTTPS remotes (clone/push over https)** need curl + OpenSSL (or rustls)
  cross-built and `--with-curl`: NOT attempted yet — the one remaining
  feasibility risk for the full userland. Local-file git ops are proven.
- Baked-in template dir is bogus for the app context
  (`C:/msys64/usr/...share/git-core/templates`); M3 must set the template
  path/`GIT_TEMPLATE_DIR` properly. Harmless (init works, just noisy).
- `cannot run maintenance` warning: `GIT_EXEC_PATH` must point at the app's
  `files/bin` on-device; by design covered in M3.
- git's test suite not run (needs a POSIX shell environment on target; M3
  evaluates how much of it is runnable via busybox ash on-device).

## Alternatives rejected

- libgit2-backed subset: unnecessary — real git builds. Would also have
  diverged CLI behavior vs. the acceptance criteria ("match CLI output").
- Vendoring git's `.git` mirror instead of tarballs: tarballs +
  sha256sums.asc are simpler to pin and verify.

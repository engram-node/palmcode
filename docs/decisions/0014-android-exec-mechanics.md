# 0014. Android nested-exec mechanics (M3-A spike)

Date: 2026-10-22. Status: accepted (PROVISIONAL evidence — emulator only at
this date; phone rerun pending in docs/VERIFY.md).

## Context

M3 runs bash on Android from the packaged userland (busybox/rg/git, no
binaries shipped beyond those built from pinned source). The M3-A spike
answers: which exec mechanics actually work under Android's kernel and
filesystem policy, before the Executor (deliverable C/D) is designed around
them. Matrix: `scripts/exec-matrix.ps1` (reusable; rerun on phone with
`-Abi arm64-v8a`).

## Facts established (emulator x86_64, Android 14 — matrix 14/14)

1. argv[0] applet dispatch works: `bin/<applet> -> ../busybox` symlink farm,
   367 applets, resolved by basename regardless of symlink target name.
2. `#!/bin/sh` shebangs work (Android ships `/bin -> /system/bin`).
3. `#!/usr/bin/env X` shebangs FAIL at execve ("No such file or directory"):
   there is no `/usr/bin/env` and `/usr` cannot be created (read-only system,
   no root). NOT fixable at the filesystem level.
4. Nested `sh -c` chains work 3+ levels deep; exit codes pass through
   (7 = 7, SIGTERM death = 143); stdin-closed gives immediate EOF (exit 0).
5. Process-group kill works: a `setsid`'d tree gets pgid = leader pid;
   `busybox kill -TERM -PGID` (negative pid — busybox kill does NOT accept
   the `--` separator) reaps leader AND grandchildren.
6. Killing only the shell pid orphans grandchildren into the dead pgid.
7. Background jobs of an adb shell die at that shell's session teardown
   (mksh job control). In-app spawns are unaffected (the app process is
   long-lived) — consistent with the PDEATHSIG/pidfile design.

## Decision

1. Scripts are invoked through `sh` explicitly by the bash tool (fresh
   `sh -c` is already the tool contract), and the command catalog (deliverable
   H) states: run scripts as `sh script`, do not rely on `#!/usr/bin/env`.
2. The executor kills the whole process group (`kill(-pgid)`, direct negative
   pid form), never a bare leader pid, and sweeps pidfile-recorded pids on
   resume (kill-mid-command must leave no orphans).
3. `setsid` at spawn: pgid = spawn root pid, so one kill reaps the tree.

## Rejected

- **Creating `/usr/bin/env`.** System image is read-only; requires root.
- **Rewriting user shebangs.** Mutates user content silently.
- **`LD_PRELOAD` execve shim.** Fragile across bionic/SELinux, per-spawn cost.
- **Relying on `kill -- -PGID` POSIX form.** busybox kill rejects `--`
  ("invalid number"); the negative-pid form is what works here.

## Consequences

- Deliverable D: spawn with setsid + PDEATHSIG + pidfile; kill via negative
  pgid; SIGTERM, 2 s grace, SIGKILL.
- Deliverable J (Linux CI + device) tests group-kill incl. grandchildren and
  orphan sweep; HostExecutor (MSYS2) must mirror the same group-kill logic
  (its kill semantics differ — covered by the same tests).

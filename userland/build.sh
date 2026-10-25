#!/usr/bin/env bash
# Build the M3 userland (busybox, ripgrep, git) for Android from
# checksum-verified upstream sources. Runs under MSYS2 (Windows host) and
# Linux (CI). Invocation: gradle `userlandBuild` task (see android/app/
# build.gradle.kts) — outputs land in <out>/jniLibs/<abi>/lib<name>.so; AGP
# (useLegacyPackaging=true → extractNativeLibs) deflates them into the APK
# and installs them on disk, so they are exec'd straight from
# ApplicationInfo.nativeLibraryDir (exec-via-symlink proven M1). argv[0]
# dispatch is ours at execve time — lib*.so disk names don't matter.
#
# Env: NDK_HOME (required) — the pinned NDK root; OUT_DIR (required).
# Consolidates userland/spike/* (device-proven 2026-10).
set -euo pipefail

: "${NDK_HOME:?gradle passes the pinned NDK dir}"
: "${OUT_DIR:?gradle passes android/app/build/userland}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=versions.env
source "$ROOT/userland/versions.env"

case "$(uname -s)" in
  # Gradle spawns bash as a NON-login shell → no /etc/profile, no /usr/bin on
  # PATH (observed: `make: command not found`). MSYS2's own mount table still
  # resolves /usr/bin, so prepend it explicitly.
  MINGW*|MSYS*|CYGWIN*)
    NDK_PREBUILT=windows-x86_64
    export PATH="/usr/bin:$PATH"
    # No symlink privilege (no Developer Mode): default winsymlinks deep-COPIES
    # dirs and FAILS on the by-design dangling links in busybox kbuild
    # (include/asm-$ARCH doesn't exist for arm64/x86_64). .lnk shell shortcuts
    # satisfy ln/rc and nothing on our arches ever dereferences them.
    export MSYS="winsymlinks:lnk"
    # rustup shims live in ~/.cargo/bin, which MSYS2 login shells don't
    # inherit (`env: 'cargo': No such file or directory` at the rg step).
    if ! command -v cargo >/dev/null 2>&1; then
      for c in "$HOME/.cargo/bin" "$([ -n "${USERPROFILE:-}" ] && cygpath -u "$USERPROFILE")/.cargo/bin"; do
        if [ -x "$c/cargo.exe" ] || [ -x "$c/cargo" ]; then
          export PATH="$c:$PATH"
          # the shim resolves toolchains via RUSTUP_HOME; under MSYS2 $HOME
          # may be /home/<user>, so point both at the Windows profile
          export CARGO_HOME="$(dirname "$c")"
          export RUSTUP_HOME="$(dirname "$(dirname "$c")")/.rustup"
          break
        fi
      done
    fi
    ;;
  Linux*)                 NDK_PREBUILT=linux-x86_64  ;;
  *) echo "unsupported host: $(uname -s)" >&2; exit 1 ;;
esac
# MSYS2 cannot exec via a C:\ path embedded in Makefiles reliably; the
# drive-letter form inside PATH entries is fine, but NDK_HOME itself may
# arrive Windows-shaped.
NDK_HOME="$(cygpath -u "$NDK_HOME" 2>/dev/null || echo "$NDK_HOME")"
# Gradle passes Windows-shaped dirs; GNU tar reads "C:\..." as a *remote host*
# ("Cannot connect to C: resolve failed") — normalize everything MSYS2-side.
OUT_DIR="$(cygpath -u "$OUT_DIR" 2>/dev/null || echo "$OUT_DIR")"
NDK_BIN="$NDK_HOME/toolchains/llvm/prebuilt/$NDK_PREBUILT/bin"
export PATH="$NDK_BIN:$PATH"

# Tarball + source cache. Default under OUT_DIR: $HOME is unreliable when
# gradle spawns us (`/home` unwritable on MSYS2; spike note), and PALMCODE_
# USERLAND_CACHE lets the host keep downloads across `clean`. CI re-fetches
# per run by design (hermetic).
CACHE="${PALMCODE_USERLAND_CACHE:-$OUT_DIR/.cache-userland}"
CACHE="$(cygpath -u "$CACHE" 2>/dev/null || echo "$CACHE")"
mkdir -p "$CACHE" "$OUT_DIR"
# wipe packaged outputs: stale spike .so files would otherwise ship in the
# APK (libexecprobe.so was left behind by the M1 spike in this dir)
rm -rf "$OUT_DIR/jniLibs"

fetch() { # <url> <sha256> <dest>
  local url=$1 sha=$2 dest="$CACHE/$3"
  if [ ! -f "$dest" ]; then curl -fSL --retry 3 -o "$dest" "$url"; fi
  echo "$sha  $dest" | sha256sum -c - >/dev/null || {
    echo "CHECKSUM MISMATCH: $dest" >&2; rm -f "$dest"; exit 1; }
  echo "$dest"
}

extract() { # <tarball> <parent-dir> <marker>  -> reuses existing tree
  local tb=$1 dir=$2 marker=$3
  if [ ! -e "$dir/$marker" ]; then
    rm -rf "$dir"; mkdir -p "$dir"
    # Some tarballs carry symlinks Windows tar can't create (HomebrewFormula,
    # scripts/config) — harmless for the build, tolerate their failure.
    tar -C "$dir" --strip-components=1 -xf "$tb" || true
  fi
  [ -e "$dir/$marker" ] || { echo "extract failed: $tb" >&2; exit 1; }
}

check_align() { # 16 KB page-alignment gate: every LOAD segment at 0x4000
  local bin=$1
  llvm-readelf -lW "$bin" | awk '/LOAD/ && $NF != "0x4000" { bad=1 } END { exit bad }' \
    || { echo "16 KB alignment MISSING in $bin" >&2; exit 1; }
}

package() { # <built-binary> <abi> <name>
  local bin=$1 abi=$2 name=$3
  # jniLibs dirs must be AGP ABI names: arm64 -> arm64-v8a ("... is not an ABI")
  local abi_dir=$abi
  [ "$abi" = arm64 ] && abi_dir=arm64-v8a
  local dest="$OUT_DIR/jniLibs/$abi_dir/lib$name.so"
  mkdir -p "$(dirname "$dest")"
  llvm-strip "$bin" -o "$dest"
  check_align "$dest"
  ls -l --block-size=1 "$dest" | awk -v n="$name" -v a="$abi" '{printf "  %-10s %-9s %8d bytes\n", n, a, $5}'
}

ABIS="arm64 x86_64"
triple_of() { case $1 in arm64) echo aarch64-linux-android ;; x86_64) echo x86_64-linux-android ;; esac; }

# ---------------------------------------------------------------- busybox
BB_TB=$(fetch "$BUSYBOX_URL" "$BUSYBOX_SHA256" "busybox-$BUSYBOX_VERSION.tar.bz2")
BB_SRC="$CACHE/src/busybox-$BUSYBOX_VERSION"
extract "$BB_TB" "$BB_SRC" "Makefile"

# BusyBox embeds AUTOCONF_TIMESTAMP in its version banner. Omit the build
# clock from both the generated config and banner, including cached builds.
export KCONFIG_NOTIMESTAMP=1

for ABI in $ABIS; do
  T=$(triple_of "$ABI")
  B="$CACHE/build/busybox-$ABI"
  mkdir -p "$B"
  # MSYS2 POSIX gcc is HOSTCC for kbuild (rustup's self-contained gcc lacks cc1).
  make -C "$BB_SRC" O="$B" allnoconfig HOSTCC=gcc >/dev/null
  # Only the reviewed applets are built, not merely hidden from PATH.
  enable_bb() {
    sed -i -e "/^CONFIG_${1}=/d" -e "/^# CONFIG_${1} is not set$/d" "$B/.config"
    echo "CONFIG_${1}=y" >> "$B/.config"
  }
  while read -r APPLET SYMBOL; do
    [[ -z "$APPLET" || "$APPLET" == \#* ]] && continue
    enable_bb "$SYMBOL"
  done < "$ROOT/userland/busybox-applets.txt"
  sed -i -e '/^CONFIG_SH_IS_NONE=/d' "$B/.config"
  for FEATURE in DESKTOP LONG_OPTS SHOW_USAGE FEATURE_VERBOSE_USAGE FEATURE_COMPRESS_USAGE \
      FEATURE_SH_MATH FEATURE_SH_MATH_64 ASH_BASH_COMPAT ASH_ECHO ASH_PRINTF ASH_TEST \
      ASH_GETOPTS ASH_CMDCMD ASH_ALIAS ASH_OPTIMIZE_FOR_SIZE \
      FEATURE_TAR_CREATE FEATURE_TAR_AUTODETECT FEATURE_TAR_GNU_EXTENSIONS \
      FEATURE_TAR_LONG_OPTIONS FEATURE_SEAMLESS_GZ FEATURE_SEAMLESS_BZ2 FEATURE_SEAMLESS_XZ \
      FEATURE_FIND_PRINT0 FEATURE_FIND_TYPE FEATURE_FIND_XDEV FEATURE_FIND_MAXDEPTH \
      FEATURE_FIND_NEWER FEATURE_FIND_INUM FEATURE_FIND_EXEC FEATURE_FIND_EXEC_PLUS \
      FEATURE_CP_LONG_OPTIONS FEATURE_MKDIR_LONG_OPTIONS FEATURE_LS_COLOR FEATURE_LS_RECURSIVE \
      FEATURE_LS_SORTFILES FEATURE_LS_TIMESTAMPS FEATURE_LS_FILETYPES FEATURE_LS_FOLLOWLINKS \
      FEATURE_STAT_FORMAT FEATURE_DU_DEFAULT_BLOCKSIZE_1K FEATURE_DD_IBS_OBS \
      FEATURE_DD_STATUS FEATURE_DD_THIRD_STATUS_LINE FEATURE_DD_SIGNAL_HANDLING; do
    enable_bb "$FEATURE"
  done
  # bionic/kernel-console hostile family (see spike notes: utmp/wtmp, sys/kd.h,
  # gethostid, tc sched, /etc/shells loginutils, SysV IPC, swapon, minix fs,
  # big nslookup needs resolver state bionic folded into libc).
  for OPT in FEATURE_UTMP FEATURE_WTMP UTMPX LAST WALL WHO USERS TC TCPSVD UDPSVD \
             LOADFONT SETFONT SHOWKEY KBD_MODE SETKEYCODES CONSPY CHVT FGCONSOLE DEALLOCVT OPENVT \
             LOADKMAP DUMPLEMAP HOSTID \
             SU LOGIN SULOGIN GETTY VLOCK PASSWD CHPASSWD CRYPT_PW MKPASSWD ADJTIMEX ETHER_WAKE \
             FEATURE_IPV6 FSCK_MINIX MKFS_MINIX IPCRM IPCS FEATURE_MTAB_SUPPORT \
             SWAPON SWAPOFF FEATURE_NSLOOKUP_BIG NSLOOKUP; do
    # shortcut: FEATURE_IPV6 off because busybox re-defines struct in6_ifreq vs
    # NDK uapi headers; upgrade: carry the Termux shared-header patch, re-enable.
    sed -i -e "s/^CONFIG_${OPT}=y$/# CONFIG_${OPT} is not set/" "$B/.config"
  done
  # `yes` dies of SIGPIPE when make stops reading; guard for `set -o pipefail`.
  { yes "" || true; } | make -C "$BB_SRC" O="$B" oldconfig HOSTCC=gcc >/dev/null
  cat > "$CACHE/build/bzero-shim.h" <<'EOF'
#ifndef __ASSEMBLER__
/* bionic shims: explicit_bzero removed (r26+, yescrypt needs (buf,0,len)
 * memset arity); addmntent has bionic readers but no writer; sigisemptyset
 * absent — an all-zero set compares equal to a zeroed sigset_t on any libc. */
#include <string.h>
#include <signal.h>
#include <mntent.h>
#include <stdio.h>
#define explicit_bzero(b, n) memset((b), 0, (n))
static int addmntent(FILE *stream, const struct mntent *mnt) {
  (void)stream; (void)mnt; return 1;
}
static int sigisemptyset(const sigset_t *set) {
  static const sigset_t zero;
  return memcmp(set, &zero, sizeof(sigset_t)) == 0;
}
#endif
EOF
  echo "== busybox $BUSYBOX_VERSION ($ABI)"
  make -C "$BB_SRC" O="$B" -j"$(nproc)" CC="$T"29-clang ARCH="$ABI" HOSTCC=gcc \
       SKIP_STRIP=y EXTRA_CFLAGS="-include $CACHE/build/bzero-shim.h"
  # Compare the compiled dispatch table, allowlist and Rust prompt catalog.
  # No Android executable is run on the build host.
  sed -n 's/^"\([^"]*\)" "\\0"$/\1/p' "$B/include/applet_tables.h" | LC_ALL=C sort > "$B/actual-applets"
  awk 'NF && $1 !~ /^#/ {print $1}' "$ROOT/userland/busybox-applets.txt" | LC_ALL=C sort > "$B/expected-applets"
  awk -F'"' '/^    "/ {print $2}' "$ROOT/core/exec/src/applets.rs" | LC_ALL=C sort > "$B/catalog-applets"
  diff -u "$B/expected-applets" "$B/actual-applets"
  diff -u "$B/expected-applets" "$B/catalog-applets"
  COUNT=$(wc -l < "$B/actual-applets")
  [ "$COUNT" -ge 100 ] && [ "$COUNT" -le 150 ] || { echo "applet count outside reviewed range" >&2; exit 1; }
  package "$B/busybox" "$ABI" busybox
done

# --------------------------------------------------------------- ripgrep
RG_TB=$(fetch "$RIPGREP_URL" "$RIPGREP_SHA256" "ripgrep-$RIPGREP_VERSION.tar.gz")
RG_SRC="$CACHE/src/ripgrep-$RIPGREP_VERSION"
# the rg binary crate is the repo ROOT (crates/ holds only libs) — Cargo.toml
# is the marker
extract "$RG_TB" "$RG_SRC" "Cargo.toml"

for ABI in $ABIS; do
  T=$(triple_of "$ABI")
  echo "== ripgrep $RIPGREP_VERSION ($ABI)"
  rustup target add "$T" >/dev/null 2>&1 || true
  # Pure-Rust closure (pcre2 off) → only the linker matters.
  env "CARGO_TARGET_$(echo "$T" | tr 'a-z-' 'A-Z_')_LINKER=$NDK_BIN/clang" \
      RUSTFLAGS="-C link-arg=--target=${T}29" \
      cargo build --release --target "$T" --manifest-path "$RG_SRC/Cargo.toml"
  package "$RG_SRC/target/$T/release/rg" "$ABI" rg
done

# ------------------------------------------------------------------- git
GIT_TB=$(fetch "$GIT_URL" "$GIT_SHA256" "git-$GIT_VERSION.tar.xz")
for ABI in $ABIS; do
  T=$(triple_of "$ABI")
  # git's `make distclean` DELETES configure — always configure a FRESH
  # extract, never reuse/clean a configured tree.
  G="$CACHE/build/git-$ABI"
  rm -rf "$G"
  extract "$GIT_TB" "$G" "git.c"
  cd "$G"
  echo "== git $GIT_VERSION ($ABI)"
  ./configure --host="$T" \
    CC="clang --target=${T}29" \
    AR=llvm-ar RANLIB=llvm-ranlib STRIP=llvm-strip \
    --without-iconv --without-curl --without-expat --without-tcltk \
    --without-python --without-libpcre2 \
    ac_cv_snprintf_returns_bogus=no \
    ac_cv_fread_reads_directories=no >/dev/null
  cat > config.mak <<'EOF'
HAVE_DEV_TTY=YesPlease
# git 2.56 builds its optional Rust component by default; the C core is the
# full product, so disable it (no target cargo needed).
NO_RUST=YesPlease
# bionic has pthreads but no cancellation API; threads are never cancelled
# anyway, so returning 0 from the stub is semantically safe.
BASIC_CFLAGS += -DPTHREAD_CANCEL_DISABLE=0 "-Dpthread_setcancelstate(a,b)=0"
EOF
  # Local ops only: single `git` binary (remote helpers, https: M5).
  make -j"$(nproc)" git >/dev/null
  package "$G/git" "$ABI" git
done

echo "== userland done -> $OUT_DIR/jniLibs"

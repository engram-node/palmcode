#!/usr/bin/env bash
# Spike: busybox 1.38.0 cross-build for Android via NDK clang (item 9 size
# measurement; final wiring lands in userland/build.sh in M3).
# Run with MSYS2:  bash userland/spike/busybox-build.sh
set -euo pipefail

VER=1.38.0
TARBALL=busybox-$VER.tar.bz2
URL=https://busybox.net/downloads/$TARBALL
NDK_BIN="/c/Users/OM/AppData/Local/Android/Sdk/ndk/30.0.16248370/toolchains/llvm/prebuilt/windows-x86_64/bin"
# NOTE: $HOME is unstable when invoked via `bash -lc` from PowerShell
# (sometimes "C:UsersOM"); use explicit absolute paths.
W="/home/OM/palmcode-spike/busybox"
mkdir -p "$W" && cd "$W"
# MSYS2 POSIX gcc supplies HOSTCC for kbuild host tools (rustup's
# self-contained gcc is a linker driver without cc1 — NOT usable).
HOSTCC=gcc
export PATH="$NDK_BIN:$PATH"

[ -f "$TARBALL" ] || curl -fSL -o "$TARBALL" "$URL"
echo "== tarball sha256 (record into versions.env):"
sha256sum "$TARBALL"
[ -d "busybox-$VER" ] || tar xf "$TARBALL"

for TARGET in arm64 x64; do
  case $TARGET in
    arm64) CC=aarch64-linux-android29-clang; ARCH=arm64 ;;
    x64)   CC=x86_64-linux-android29-clang; ARCH=x86_64 ;;
  esac
  B="$W/build-$TARGET"
  mkdir -p "$B"
  cd "$W/busybox-$VER"
  make O="$B" defconfig HOSTCC="$HOSTCC" >/dev/null
  # Android/NDK lacks the utmp/wtmp family; TC needs pkt_sched internals.
  # NB: scripts/config in the tarball is a symlink Windows tar can't create — sed .config instead.
  # Android/bionic gaps: utmp/wtmp family, kernel console ioctls (sys/kd.h),
  # gethostid, tc sched internals, /etc/shells-based loginutils (su is
  # pointless on a stock phone), crypt shells.
  # Broad bionic/kernel-console hostile family (Termux prunes the same way):
  for OPT in FEATURE_UTMP FEATURE_WTMP UTMPX LAST WALL WHO USERS TC TCPSVD UDPSVD \
             LOADFONT SETFONT SHOWKEY KBD_MODE SETKEYCODES CONSPY CHVT FGCONSOLE DEALLOCVT OPENVT \
             LOADKMAP DUMPLEMAP HOSTID \
             SU LOGIN SULOGIN GETTY VLOCK PASSWD CHPASSWD CRYPT_PW MKPASSWD ADJTIMEX ETHER_WAKE \
             FEATURE_IPV6 FSCK_MINIX MKFS_MINIX IPCRM IPCS FEATURE_MTAB_SUPPORT \
             SWAPON SWAPOFF FEATURE_NSLOOKUP_BIG NSLOOKUP; do
  # NSLOOKUP (even the small variant) touches the global _res resolver
  # state struct, which bionic removed; NSLOOKUP_BIG additionally pulls
  # -lresolv which bionic folds into libc. M3 could patch bionic's
  # res_* API in, but there is no production need for it yet.
  # NSLOOKUP_BIG pulls -lresolv (probe: dumpmachine contains "linux") but
  # bionic folds the resolver into libc — libresolv.so does not exist.
  # SWAPON/SWAPOFF: bionic declares no swapon(2)/swapoff(2)/SWAP_FLAG_*;
  # zram is system-managed on Android.
  # IPCRM/IPCS: union semun redefinition (busybox vs bionic sys/sem.h);
  # SysV IPC carries no userland value on Android. FEATURE_MTAB_SUPPORT:
  # mount.c calls addmntent (bionic lacks mntent writers).
  # shortcut: FEATURE_IPV6 off only because busybox defines struct in6_ifreq
  # in several translation units (ifconfig.c, interface.c) and NDK uapi
  # linux/in6.h already has it; M3 should source-patch one shared guarded
  # header (Termux carries this patch) and re-enable IPv6. minix fs applets
  # need BSD setbit/clrbit and are worthless on Android anyway.
    sed -i -e "s/^CONFIG_${OPT}=y$/# CONFIG_${OPT} is not set/" "$B/.config"
  done
  # `yes` dies of SIGPIPE once make stops reading; without the guard,
  # set -o pipefail kills the script right here (observed).
  { yes "" || true; } | make O="$B" oldconfig HOSTCC="$HOSTCC" >/dev/null
  echo "== building busybox $VER ($TARGET, CC=$CC)"
  # bionic dropped explicit_bzero (deliberately, NDK r26+); yescrypt calls it
  # (buf,len), memset needs (buf,0,len). A plain -Dexplicit_bzero=memset is
  # WRONG (arity). Force-include a function-macro shim instead.
  # __ASSEMBLER__ guard: the same EXTRA_CFLAGS hits *.S rules, and pulling
  # C headers into an asm compile explodes (observed at hash_sha256_hwaccel).
  # sigisemptyset: bionic has none. Do NOT poke fields — the uapi layout of
  # sigset_t differs per arch (struct vs scalar; both compile errors seen).
  # An empty set is all-zero bytes on every libc: memcmp is portable.
  # addmntent: bionic has mntent readers but no writer; mount.c calls it
  # unconditionally. Stub returns 1 ("error", harmless: no writable
  # /etc/mtab exists on stock Android anyway).
  cat > "$W/bzero-shim.h" <<'EOF'
#ifndef __ASSEMBLER__
#include <string.h>
#include <signal.h>
#include <mntent.h>
#include <stdio.h>
#define explicit_bzero(b, n) memset((b), 0, (n))
static int addmntent(FILE *stream, const struct mntent *mnt) {
  (void)stream; (void)mnt; return 1; /* bionic lacks the mtab writer */
}
static int sigisemptyset(const sigset_t *set) {
  static const sigset_t zero;
  return memcmp(set, &zero, sizeof(sigset_t)) == 0;
}
#endif
EOF
  make O="$B" -j"$(nproc)" CC="$CC" ARCH="$ARCH" HOSTCC="$HOSTCC" SKIP_STRIP=y \
       EXTRA_CFLAGS="-include $W/bzero-shim.h"
  llvm-strip "$B/busybox" -o "$B/busybox-stripped"
  echo "== $TARGET sizes:"
  ls -l --block-size=1 "$B/busybox" "$B/busybox-stripped" | awk '{print $5, $9}'
  file "$B/busybox-stripped"
done
echo "== busybox spike done"

/*
 * M0 exec probe (spike B): proves exec() through a symlink in filesDir
 * pointing into the APK's extracted native library directory.
 * Expected: prints this line with its pid, exits 42.
 */
#include <stdio.h>
#include <unistd.h>

int main(void) {
    printf("exec-probe ok pid=%d\n", getpid());
    fflush(stdout);
    return 42;
}

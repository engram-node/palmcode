package dev.pocket.app

import android.content.Context
import android.system.Os
import java.io.File

/**
 * Spike B: verify that an APK-shipped binary (libexecprobe.so, extracted to
 * the native library dir thanks to legacy JNI packaging) can be executed
 * through a symlink living in app-private storage.
 *
 * Expectation (to be verified on device via docs/VERIFY.md):
 * stdout "exec-probe ok <pid>" and exit code 42.
 */
fun runExecProbe(context: Context): String {
    val startedNanos = System.nanoTime()
    return try {
        val binDir = File(context.filesDir, "bin").apply { mkdirs() }
        val probe = File(binDir, "probe")
        val target = File(context.applicationInfo.nativeLibraryDir, "libexecprobe.so")
        if (!probe.exists() || probe.canonicalFile != target.canonicalFile) {
            probe.delete()
            Os.symlink(target.absolutePath, probe.absolutePath)
        }
        val process = ProcessBuilder(probe.absolutePath)
            .redirectErrorStream(true)
            .start()
        val out = process.inputStream.bufferedReader().readText().trim()
        val exit = process.waitFor()
        val ms = (System.nanoTime() - startedNanos) / 1_000_000
        "exit=$exit  out=\"$out\"  (${ms} ms)\n${probe.absolutePath} -> ${target.absolutePath}"
    } catch (t: Throwable) {
        "EXEC PROBE FAILED: $t"
    }
}

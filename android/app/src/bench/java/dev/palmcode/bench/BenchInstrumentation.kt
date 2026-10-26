package dev.palmcode.bench

import android.app.Instrumentation
import android.content.Intent
import android.os.Bundle
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

object BenchRunState {
    @Volatile var success = false
    val done = CountDownLatch(1)
}

/** Shell-controlled entry in the isolated bench APK, never a release component. */
class BenchInstrumentation : Instrumentation() {
    private var arguments = Bundle()
    override fun onCreate(arguments: Bundle?) {
        super.onCreate(arguments)
        this.arguments = arguments ?: Bundle()
        start()
    }

    override fun onStart() {
        val url = arguments.getString("url").orEmpty()
        val turns = arguments.getString("turns")?.toIntOrNull() ?: 50
        if (!BenchArgs.valid(url, turns)) { finish(1, Bundle()); return }
        targetContext.startActivity(Intent(targetContext, BenchActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK).putExtra("url", url).putExtra("turns", turns)
            .putExtra("ui", arguments.getString("ui") == "true"))
        val completed = BenchRunState.done.await(30, TimeUnit.MINUTES)
        if (arguments.getString("ui") == "true" && completed) Thread.sleep(15000)
        finish(if (completed && BenchRunState.success) -1 else 1,
            Bundle().also { it.putBoolean("complete", completed && BenchRunState.success) })
    }
}

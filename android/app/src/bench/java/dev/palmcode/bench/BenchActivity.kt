package dev.palmcode.bench

import android.app.Activity
import android.app.Service
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.os.Bundle
import android.os.SystemClock
import android.util.Log
import dev.palmcode.app.PalmcodeCore
import dev.palmcode.core.*
import java.io.File
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

object BenchArgs {
    fun valid(url: String, turns: Int): Boolean = runCatching {
        val uri = java.net.URI(url)
        turns in 1..1000 && uri.scheme == "http" &&
            uri.host in setOf("127.0.0.1", "[::1]", "::1") &&
            uri.port in 1..65535 && uri.userInfo == null && uri.query == null && uri.fragment == null
    }.getOrDefault(false)
}

/** Mock-only runner in an isolated application id; no Compose or provider store. */
class BenchActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val url = intent.getStringExtra("url").orEmpty()
        val turns = intent.getIntExtra("turns", 50)
        if (BenchArgs.valid(url, turns)) {
            startForegroundService(Intent(this, BenchService::class.java).putExtra("url", url).putExtra("turns", turns))
        } else Log.i("palmcode", "PHASE rejected_arguments")
        finish()
    }
}

class BenchService : Service() {
    private val events = LinkedBlockingQueue<CoreEvent>()
    private var running = false
    @Volatile private var session: String? = null
    @Volatile private var stopping = false
    private var wake: PowerManager.WakeLock? = null
    private val listener = object : CoreEventListener {
        override fun onEvents(batch: List<CoreEvent>) { batch.forEach { events.offer(it) } }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == "stop") {
            stopping = true
            session?.let { turnStop(it) }
            stopSelf()
            return START_NOT_STICKY
        }
        if (running) return START_NOT_STICKY
        val url = intent?.getStringExtra("url").orEmpty()
        val turns = intent?.getIntExtra("turns", 50) ?: 50
        if (!BenchArgs.valid(url, turns)) {
            alog("PHASE rejected_arguments")
            stopSelf()
            return START_NOT_STICKY
        }
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel("bench", "Benchmark", NotificationManager.IMPORTANCE_LOW))
        val stop = PendingIntent.getService(this, 0, Intent(this, BenchService::class.java).setAction("stop"), PendingIntent.FLAG_IMMUTABLE)
        val notification = Notification.Builder(this, "bench").setSmallIcon(android.R.drawable.ic_media_play)
            .setContentTitle("palmcode benchmark running").setOngoing(true)
            .addAction(Notification.Action.Builder(null, "Stop", stop).build()).build()
        startForeground(3, notification, if (Build.VERSION.SDK_INT >= 34) ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC else 0)
        wake = getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "palmcode:bench").also { it.acquire() }
        running = true
        check(PalmcodeCore.nativeInitAndroid(applicationContext) == 0)
        Thread({ run(url, turns) }, "palmcode-bench").start()
        return START_NOT_STICKY
    }

    override fun onTimeout(startId: Int, fgsType: Int) { stopping = true; session?.let { turnStop(it) }; stopSelf() }

    override fun onDestroy() {
        stopping = true
        session?.let { turnStop(it) }
        wake?.let { if (it.isHeld) it.release() }
        super.onDestroy()
    }

    private fun run(url: String, turns: Int) {
        try {
            coreStart(File(filesDir, "bench.db").absolutePath, listener)
            providerConfigure(ProviderConfigRecord(
                baseUrl = url.trimEnd('/'), apiKey = "",
                mainModel = System.getenv("PALMCODE_LLM_MODEL").orEmpty(), fastModel = "",
                contextWindow = 32768u, maxOutputTokens = 4096u,
                samplingJson = null, extraBodyJson = null, continuousUsage = false,
                firstTokenSecs = 30u, idleSecs = 30u,
            ))
            val workspace = File(filesDir, "bench-workspace").also { check(it.mkdirs() || it.isDirectory) }
            workspaceSet(workspace.absolutePath)
            alog("PHASE configured turns=$turns")
            repeat(turns) { index ->
                check(!stopping)
                val id = sessionsCreate("Benchmark turn").id
                session = id
                events.clear()
                val start = SystemClock.elapsedRealtime()
                messageSend(id, "Run the scripted workspace task.")
                check(!stopping) { "stopped during submission" }
                val deadline = start + 600_000
                var tools = 0
                while (true) {
                    val left = deadline - SystemClock.elapsedRealtime()
                    check(left > 0) { "turn timeout" }
                    when (val event = events.poll(left, TimeUnit.MILLISECONDS)) {
                        is CoreEvent.ApprovalRequested -> if (event.sessionId == id) approvalResolve(event.requestId, true, "once")
                        is CoreEvent.ToolFinished -> if (event.sessionId == id) tools++
                        is CoreEvent.MessageFinished -> if (event.sessionId == id) break
                        is CoreEvent.TurnError -> error("turn failed")
                        is CoreEvent.Interrupted -> error("turn interrupted")
                        null -> error("turn timeout")
                        else -> {}
                    }
                }
                check(tools > 0) { "script did not execute a tool" }
                alog("PHASE turn=${index + 1} wall_ms=${SystemClock.elapsedRealtime() - start} tools=$tools")
                session = null
            }
            // Fine-grained OVERHEAD lines are emitted by the Rust agent's alog.
            alog("PHASE complete turns=$turns")
        } catch (_: Exception) {
            session?.let { turnStop(it) }
            alog("PHASE failed")
        } finally {
            coreShutdown()
            stopSelf()
        }
    }

    private fun alog(line: String) { Log.i("palmcode", line) }
}

package dev.palmcode.app

import android.content.Context
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.Snapshot
import androidx.compose.runtime.snapshots.SnapshotStateList
import dev.palmcode.core.CoreEvent
import dev.palmcode.core.CoreEventListener
import dev.palmcode.core.SessionInfo
import dev.palmcode.core.coreShutdown
import dev.palmcode.core.coreStart
import dev.palmcode.core.messageSend
import dev.palmcode.core.providerConfigure
import dev.palmcode.core.sessionTranscript
import dev.palmcode.core.sessionsCreate
import dev.palmcode.core.sessionsList
import dev.palmcode.core.turnResume
import dev.palmcode.core.turnStop
import java.io.File

sealed interface Screen {
    data object Setup : Screen
    data object Sessions : Screen
    data class Chat(val sessionId: String) : Screen
}

/** One transcript entry. Per-field snapshot state: only a cell whose own
 *  fields changed recomposes — the streaming tail reparses alone. */
class ChatMsg(val id: Long, val role: String) {
    var text by mutableStateOf("")
    var reasoning by mutableStateOf("")
    var streaming by mutableStateOf(false)
    var interrupted by mutableStateOf(false)
    var error by mutableStateOf<String?>(null)
    var ttftMs by mutableStateOf<Long?>(null)
    var decodeTps by mutableStateOf<Double?>(null)
    var completionTokens by mutableStateOf<Long?>(null)
    var cachePct by mutableStateOf<Int?>(null)
}

object AppState {
    var screen by mutableStateOf<Screen>(Screen.Setup)
    var providerCleartext by mutableStateOf(false)
    val sessions = mutableStateListOf<SessionInfo>()
    private val transcripts = mutableStateMapOf<String, SnapshotStateList<ChatMsg>>()

    fun transcript(sessionId: String): SnapshotStateList<ChatMsg> =
        transcripts.getOrPut(sessionId) { mutableStateListOf() }

    /** Rebuild a transcript from the store (cold open / after process death). */
    fun loadTranscript(sessionId: String) {
        val items = sessionTranscript(sessionId)
        Snapshot.withMutableSnapshot {
            val list = transcript(sessionId)
            list.clear()
            for (t in items) {
                if (t.role == "system") continue
                val m = ChatMsg(t.messageId, t.role)
                m.text = t.text
                m.reasoning = t.reasoning
                m.streaming = t.streaming
                m.interrupted = t.interrupted
                m.error = t.error
                if (t.tokens > 0u) m.completionTokens = t.tokens.toLong()
                list.add(m)
            }
        }
    }

    private fun find(sessionId: String, messageId: Long): ChatMsg? =
        transcripts[sessionId]?.firstOrNull { it.id == messageId }

    private fun lastAssistant(sessionId: String): ChatMsg? =
        transcripts[sessionId]?.lastOrNull { it.role == "assistant" }

    /** Apply one coalesced engine batch atomically (one recomposition). */
    fun apply(events: List<CoreEvent>) {
        Snapshot.withMutableSnapshot {
            for (e in events) {
                when (e) {
                    is CoreEvent.SessionCreated -> {
                        // refresh via sessionsList in CoreManager
                    }
                    is CoreEvent.MessageStarted -> {
                        val list = transcript(e.sessionId)
                        if (list.none { it.id == e.messageId }) {
                            val m = ChatMsg(e.messageId, e.role)
                            m.streaming = e.role == "assistant"
                            list.add(m)
                        }
                    }
                    is CoreEvent.TextDelta -> find(e.sessionId, e.messageId)?.let { it.text = e.snapshot }
                    is CoreEvent.ReasoningDelta -> find(e.sessionId, e.messageId)?.let { it.reasoning = e.snapshot }
                    is CoreEvent.UsageTick -> {}
                    is CoreEvent.MessageFinished -> find(e.sessionId, e.messageId)?.let {
                        it.streaming = false
                        it.ttftMs = e.ttftMs?.toLong()
                        it.decodeTps = e.decodeTps
                        it.completionTokens = e.completionTokens?.toLong()
                        val cached = e.cachedTokens?.toLong()
                        val prompt = e.promptTokens?.toLong()
                        if (cached != null && prompt != null && prompt > 0) {
                            it.cachePct = ((cached * 100) / prompt).toInt()
                        }
                    }
                    is CoreEvent.Interrupted -> find(e.sessionId, e.messageId)?.let {
                        it.streaming = false
                        it.interrupted = true
                    }
                    is CoreEvent.TurnError -> {
                        val m = e.messageId?.let { id -> find(e.sessionId, id) }
                            ?: lastAssistant(e.sessionId)
                        m?.let {
                            it.streaming = false
                            it.error = "${e.code}: ${e.detail}"
                        }
                    }
                    is CoreEvent.Compacted -> {}
                }
            }
        }
    }
}

/**
 * Process-wide owner of the Rust engine: start once, route all UI calls,
 * fan events onto the main thread into the Compose snapshot system.
 */
object CoreManager {
    private val main = Handler(Looper.getMainLooper())
    // Strong ref: the UniFFI callback object must not be GC'd.
    private val listener = object : CoreEventListener {
        override fun onEvents(events: List<CoreEvent>) {
            main.post {
                AppState.apply(events)
                if (events.any { it is CoreEvent.SessionCreated }) refreshSessions()
            }
        }
    }
    private var started = false
    lateinit var store: ProviderStore
        private set

    @Synchronized
    fun ensureStarted(context: Context) {
        if (started) return
        check(PalmcodeCore.nativeInitAndroid(context) == 0) { "native TLS init failed" }
        coreStart(File(context.filesDir, "palmcode.db").absolutePath, listener)
        store = ProviderStore(context.applicationContext)
        started = true
    }

    /** Keystore-decrypt the key and hand it to Rust; zero the JVM copy. */
    fun configureFromStore(): Boolean {
        val keyBytes = store.loadApiKey() ?: return false
        return try {
            val key = String(keyBytes, Charsets.UTF_8)
            providerConfigure(store.toRecord(key))
            keyBytes.fill(0)
            AppState.providerCleartext = store.cleartext
            true
        } catch (_: Exception) {
            keyBytes.fill(0)
            false
        }
    }

    fun refreshSessions() {
        val list = sessionsList()
        Snapshot.withMutableSnapshot {
            AppState.sessions.clear()
            AppState.sessions.addAll(list)
        }
    }

    fun newChat(): String {
        val session = sessionsCreate("Chat ${System.currentTimeMillis() / 1000 % 100000}")
        refreshSessions()
        return session.id
    }

    fun openChat(sessionId: String) {
        AppState.loadTranscript(sessionId)
        AppState.screen = Screen.Chat(sessionId)
    }

    fun send(sessionId: String, text: String): Result<Unit> = runCatching {
        messageSend(sessionId, text)
        Unit
    }

    fun stop(sessionId: String) = turnStop(sessionId)

    fun resumeChat(sessionId: String) = turnResume(sessionId)

    // Process exit tears the engine down; no explicit UI-side shutdown.
    @Suppress("unused")
    fun shutdown() = coreShutdown()
}

package dev.palmcode.app

/** Main-thread run lifetime protocol, shared by UI and foreground service. */
class RunTracker {
    private val sessions = linkedSetOf<String>()
    val active: Set<String> get() = sessions.toSet()
    fun begin(session: String): Boolean = sessions.add(session)
    fun finish(session: String) { sessions.remove(session) }
}

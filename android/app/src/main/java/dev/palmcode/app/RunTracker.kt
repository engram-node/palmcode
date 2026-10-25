package dev.palmcode.app

/** Main-thread run lifetime protocol, shared by UI and foreground service. */
class RunTracker {
    private val sessions = linkedSetOf<String>()
    private val stopped = hashSetOf<String>()
    val active: Set<String> get() = sessions.toSet()
    fun begin(session: String): Boolean = sessions.add(session)
    fun requestStop(session: String) { if (session in sessions) stopped.add(session) }
    fun stopRequested(session: String): Boolean = session in stopped
    fun finish(session: String) { sessions.remove(session); stopped.remove(session) }
}

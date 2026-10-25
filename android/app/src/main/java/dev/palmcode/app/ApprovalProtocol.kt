package dev.palmcode.app

data class ApprovalDecision(val approved: Boolean, val scope: String)

object ApprovalProtocol {
    fun decide(action: String?, requestId: ULong?, session: String?, nonce: String?,
               expectedId: ULong, expectedSession: String, expectedNonce: String): ApprovalDecision? {
        if (requestId != expectedId || session != expectedSession || nonce != expectedNonce) return null
        return when (action) {
            "once" -> ApprovalDecision(true, "once")
            "session" -> ApprovalDecision(true, "session")
            "deny" -> ApprovalDecision(false, "once")
            else -> null
        }
    }
}

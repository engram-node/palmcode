package dev.palmcode.app

import org.junit.Assert.*
import org.junit.Test

class ApprovalProtocolTest {
    private fun resolve(action: String?, id: ULong? = 7u, session: String? = "s", nonce: String? = "current") =
        ApprovalProtocol.decide(action, id, session, nonce, 7u, "s", "current")

    @Test fun actionsUseTheInAppPermissionProtocol() {
        assertEquals(ApprovalDecision(true, "once"), resolve("once"))
        assertEquals(ApprovalDecision(true, "session"), resolve("session"))
        assertEquals(ApprovalDecision(false, "once"), resolve("deny"))
    }

    @Test fun malformedAndStaleActionsFailClosed() {
        assertNull(resolve("project"))
        assertNull(resolve(null))
        assertNull(resolve("once", id = null))
        assertNull(resolve("once", id = 8u))
        assertNull(resolve("once", session = "other"))
        assertNull(resolve("once", nonce = "previous-process"))
    }
}

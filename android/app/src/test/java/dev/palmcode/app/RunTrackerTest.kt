package dev.palmcode.app

import org.junit.Assert.*
import org.junit.Test

class RunTrackerTest {
    @Test fun stopDuringSubmissionIsRetainedUntilTheTurnTerminates() {
        val runs = RunTracker()
        runs.begin("one")
        runs.requestStop("one")
        assertTrue(runs.stopRequested("one"))
        assertEquals(setOf("one"), runs.active)
        runs.finish("one")
        assertFalse(runs.stopRequested("one"))
        runs.begin("one")
        assertFalse(runs.stopRequested("one"))
    }

    @Test fun overlappingRunsKeepTheServiceActiveUntilTheLastTerminalEvent() {
        val runs = RunTracker()
        assertTrue(runs.begin("one"))
        assertFalse(runs.begin("one"))
        assertTrue(runs.begin("two"))
        runs.finish("one")
        runs.finish("one")
        assertEquals(setOf("two"), runs.active)
        runs.finish("two")
        assertTrue(runs.active.isEmpty())
    }

    @Test fun failedStartCanBeRetriedAndSnapshotsCannotMutateTheTracker() {
        val runs = RunTracker()
        runs.begin("one")
        val snapshot = runs.active
        runs.finish("one")
        assertEquals(setOf("one"), snapshot)
        assertTrue(runs.active.isEmpty())
        assertTrue(runs.begin("one"))
    }
}

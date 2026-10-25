package dev.palmcode.bench

import org.junit.Assert.*
import org.junit.Test

class BenchArgsTest {
    @Test fun onlyExplicitNumericLoopbackMockEndpointsAreAccepted() {
        assertTrue(BenchArgs.valid("http://127.0.0.1:8787/v1", 50))
        assertFalse(BenchArgs.valid("https://127.0.0.1:8787/v1", 50))
        assertFalse(BenchArgs.valid("http://127.0.0.1/v1", 50))
        assertFalse(BenchArgs.valid("http://127.0.0.1:8787/v1", 0))
        assertFalse(BenchArgs.valid("http://127.0.0.1:8787/v1", 1001))
        assertFalse(BenchArgs.valid("http://127.0.0.1:8787/v1?q=1", 50))
        assertFalse(BenchArgs.valid("invalid", 50))
    }
}

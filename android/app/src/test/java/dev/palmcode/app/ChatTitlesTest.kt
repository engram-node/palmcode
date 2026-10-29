package dev.palmcode.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ChatTitlesTest {
    @Test fun automaticTitlesHideLegacyNumbers() {
        for (title in listOf("Chat 12345", "New session", "New chat")) {
            assertTrue(ChatTitles.isPlaceholder(title))
            assertEquals("New chat", ChatTitles.display(title))
        }
    }

    @Test fun summariesAndCustomTitlesArePreserved() {
        for (title in listOf("Fix shell tests", "Chat about Rust", "Chat 12x")) {
            assertFalse(ChatTitles.isPlaceholder(title))
            assertEquals(title, ChatTitles.display(title))
        }
    }
}

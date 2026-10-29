package dev.palmcode.app

internal object ChatTitles {
    fun isPlaceholder(title: String): Boolean =
        title == "New chat" || title == "New session" || Regex("Chat [0-9]+").matches(title)

    fun display(title: String): String = if (isPlaceholder(title)) "New chat" else title
}

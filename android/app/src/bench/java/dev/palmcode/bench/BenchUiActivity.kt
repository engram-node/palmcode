package dev.palmcode.bench

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Surface
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import dev.palmcode.app.AppState
import dev.palmcode.app.ui.MessageBubble
import dev.palmcode.app.ui.theme.PalmcodeTheme

object BenchUiState { var session by mutableStateOf("") }

/** Renders the app's actual transcript/tool cards without taps or provider setup. */
class BenchUiActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setShowWhenLocked(true)
        setTurnScreenOn(true)
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        setContent {
            PalmcodeTheme(dynamicColor = false) {
                val session = BenchUiState.session
                Surface(Modifier.fillMaxSize()) {
                    LazyColumn {
                        items(AppState.transcript(session), key = { it.id }) { MessageBubble(session, it) }
                    }
                }
            }
        }
    }
}

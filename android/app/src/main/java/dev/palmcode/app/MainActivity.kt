package dev.palmcode.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.ui.Modifier
import dev.palmcode.app.ui.ChatScreen
import dev.palmcode.app.ui.SessionListScreen
import dev.palmcode.app.ui.SetupScreen
import dev.palmcode.app.ui.WorkspaceScreen
import dev.palmcode.app.ui.theme.PalmcodeTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        CoreManager.ensureStarted(applicationContext)
        // Returning to a configured app: decrypt the Keystore-wrapped key,
        // hand it to Rust, land on the session list.
        if (CoreManager.store.configured && CoreManager.configureFromStore()) {
            AppState.screen = Screen.Sessions
            CoreManager.refreshSessions()
        }
        setContent {
            PalmcodeTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    when (val s = AppState.screen) {
                        Screen.Setup -> SetupScreen(onSaved = {
                            AppState.screen = Screen.Sessions
                        })
                        Screen.Sessions -> SessionListScreen()
                        Screen.Workspaces -> WorkspaceScreen()
                        is Screen.Chat -> ChatScreen(s.sessionId)
                    }
                }
            }
        }
    }
}

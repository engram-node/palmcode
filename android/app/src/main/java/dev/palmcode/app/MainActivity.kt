package dev.palmcode.app

import android.os.Bundle
import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
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
    private val notifications = registerForActivityResult(ActivityResultContracts.RequestPermission()) { }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        CoreManager.ensureStarted(applicationContext)
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            val prefs = getSharedPreferences("run-prompts", MODE_PRIVATE)
            if (!prefs.getBoolean("notifications-shown", false)) {
                prefs.edit().putBoolean("notifications-shown", true).apply()
                notifications.launch(Manifest.permission.POST_NOTIFICATIONS)
            }
        }
        val prompts = getSharedPreferences("run-prompts", MODE_PRIVATE)
        if (!prompts.getBoolean("battery-shown", false)) {
            prompts.edit().putBoolean("battery-shown", true).apply()
            if (!getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(packageName)) {
                android.app.AlertDialog.Builder(this)
                    .setTitle("Allow longer agent runs")
                    .setMessage("Battery optimization can pause network requests while the screen is off. You can allow palmcode in battery settings. Runs may still be stopped by Android; interrupted commands are never replayed automatically.")
                    .setPositiveButton("Battery settings") { _, _ ->
                        startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
                    }.setNegativeButton("Later", null).show()
            }
        }
        // Returning to a configured app: decrypt the Keystore-wrapped key,
        // hand it to Rust, land on the session list.
        if (CoreManager.store.configured && CoreManager.configureFromStore()) {
            AppState.screen = Screen.Sessions
            CoreManager.refreshSessions()
            openNotificationSession(intent)
        }
        setContent {
            PalmcodeTheme(
                mode = CoreManager.store.themeMode,
                dynamicColor = CoreManager.store.dynamicColor,
                onDarkThemeChanged = { dark ->
                    val bars = SystemBarStyle.auto(
                        android.graphics.Color.TRANSPARENT,
                        android.graphics.Color.TRANSPARENT,
                    ) { dark }
                    enableEdgeToEdge(statusBarStyle = bars, navigationBarStyle = bars)
                },
            ) {
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

    override fun onStart() {
        super.onStart()
        CoreManager.setAppVisible(true)
    }

    override fun onStop() {
        CoreManager.setAppVisible(false)
        super.onStop()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        openNotificationSession(intent)
    }

    private fun openNotificationSession(intent: Intent) {
        val session = intent.getStringExtra(ApprovalNotifications.SESSION) ?: return
        if (AppState.sessions.any { it.id == session }) CoreManager.openChat(session)
    }
}

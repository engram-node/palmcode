package dev.palmcode.app

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build

object ApprovalNotifications {
    val nonce: String = java.util.UUID.randomUUID().toString()
    private const val CHANNEL = "agent-approvals"
    const val SESSION = "session"

    fun clearStale(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.activeNotifications.filter { it.notification.channelId == CHANNEL }
            .forEach { manager.cancel(it.tag, it.id) }
    }

    fun cancel(context: Context, id: ULong) {
        context.getSystemService(NotificationManager::class.java).cancel("$nonce:$id", 2)
    }

    fun post(context: Context, approval: Approval) {
        if (Build.VERSION.SDK_INT >= 33 && context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) return
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL, "Command approvals", NotificationManager.IMPORTANCE_HIGH))
        val open = PendingIntent.getActivity(context, 0,
            Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
                .setData(Uri.parse("palmcode://approval/$nonce/${approval.requestId}"))
                .putExtra(SESSION, approval.sessionId),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        fun action(label: String, scope: String): Notification.Action {
            val intent = Intent(context, ApprovalReceiver::class.java)
                .setData(Uri.parse("palmcode://approval/$nonce/${approval.requestId}/$scope"))
                .setAction(scope).putExtra("request", approval.requestId.toString())
                .putExtra(SESSION, approval.sessionId).putExtra("nonce", nonce)
            return Notification.Action.Builder(null, label,
                PendingIntent.getBroadcast(context, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)).build()
        }
        manager.notify("$nonce:${approval.requestId}", 2,
            Notification.Builder(context, CHANNEL).setSmallIcon(android.R.drawable.ic_dialog_alert)
                .setContentTitle("palmcode needs approval")
                .setContentText("Open the session to review the command and its access.")
                .setContentIntent(open).setCategory(Notification.CATEGORY_RECOMMENDATION)
                .setVisibility(Notification.VISIBILITY_PRIVATE).setOngoing(true)
                .addAction(action("Approve once", "once"))
                .addAction(action("Always for session", "session"))
                .addAction(action("Deny", "deny")).build())
    }
}

class ApprovalReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getStringExtra("request")?.toULongOrNull() ?: return
        val pending = AppState.approvals.firstOrNull { it.requestId == id } ?: return
        val decision = ApprovalProtocol.decide(intent.action, id,
            intent.getStringExtra(ApprovalNotifications.SESSION), intent.getStringExtra("nonce"),
            pending.requestId, pending.sessionId, ApprovalNotifications.nonce) ?: return
        CoreManager.resolveApproval(id, decision.approved, decision.scope)
    }
}

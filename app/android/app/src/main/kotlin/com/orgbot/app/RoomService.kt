package com.orgbot.app

import android.app.*
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.*
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

/** User-started rooms own this service; pausing the last room releases its locks. */
class RoomService : Service() {
    companion object {
        private const val CHANNEL = "active_rooms"
        private const val NOTIFICATION = 41
        private val activeRooms = mutableSetOf<String>()
        private var instance: RoomService? = null
        private var bridge: MethodChannel? = null

        fun configure(engine: FlutterEngine, context: Context) {
            bridge = MethodChannel(engine.dartExecutor.binaryMessenger, "orgbot/background")
            bridge!!.setMethodCallHandler { call, result ->
                if (call.method != "room") { result.notImplemented(); return@setMethodCallHandler }
                val id = call.argument<String>("id")
                if (id.isNullOrBlank()) { result.error("room", "Room id is required", null); return@setMethodCallHandler }
                try {
                    if (call.argument<Boolean>("active") == true) activeRooms.add(id) else activeRooms.remove(id)
                    if (activeRooms.isEmpty()) context.stopService(Intent(context, RoomService::class.java))
                    else if (instance != null) instance!!.showStatus()
                    else {
                        MainActivity.current.get()?.requestNotifications()
                        val intent = Intent(context, RoomService::class.java)
                        if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(intent) else context.startService(intent)
                    }
                    result.success(null)
                } catch (e: Exception) {
                    activeRooms.remove(id)
                    result.error("background", "Android could not keep this room active. Open the app and resume.", null)
                }
            }
        }
    }

    private var cpu: PowerManager.WakeLock? = null
    private var wifi: WifiManager.WifiLock? = null

    override fun onCreate() {
        super.onCreate()
        instance = this
        if (Build.VERSION.SDK_INT >= 26) {
            getSystemService(NotificationManager::class.java).createNotificationChannel(
                NotificationChannel(CHANNEL, "Active rooms", NotificationManager.IMPORTANCE_LOW)
            )
        }
    }

    @Suppress("DEPRECATION")
    private fun showStatus() {
        val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pause = Intent(this, RoomService::class.java).setAction("pause")
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)
        val notification = builder.setSmallIcon(R.drawable.ic_room_notification)
            .setContentTitle(if (activeRooms.size == 1) "1 room is active" else "${activeRooms.size} rooms are active")
            .setContentText("Bots and workspace connections are running")
            .setContentIntent(PendingIntent.getActivity(this, 0, open, flags))
            .addAction(Notification.Action.Builder(null, "Pause all", PendingIntent.getService(this, 1, pause, flags)).build())
            .setOngoing(true).setOnlyAlertOnce(true).setVisibility(Notification.VISIBILITY_PRIVATE)
            .setCategory(Notification.CATEGORY_SERVICE).build()
        if (Build.VERSION.SDK_INT >= 34) startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(NOTIFICATION, notification)
        if (cpu == null) {
            cpu = getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "OrgBot:rooms")
            cpu!!.setReferenceCounted(false)
            cpu!!.acquire()
            wifi = (applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager)
                .createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "OrgBot:workspace")
            wifi!!.setReferenceCounted(false)
            wifi!!.acquire()
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (activeRooms.isEmpty()) { stopSelf(); return START_NOT_STICKY }
        showStatus()
        if (intent?.action == "pause") bridge?.invokeMethod("pauseAll", null)
        // Process death restores saved rooms paused; never replay external actions.
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        if (cpu?.isHeld == true) cpu?.release()
        if (wifi?.isHeld == true) wifi?.release()
        cpu = null; wifi = null; instance = null
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null
}

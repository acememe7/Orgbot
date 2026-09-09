package com.orgbot.app

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.provider.OpenableColumns
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.embedding.engine.FlutterEngineCache
import io.flutter.plugin.common.MethodChannel
import java.io.File
import java.nio.ByteBuffer
import java.nio.charset.CodingErrorAction
import java.lang.ref.WeakReference

class MainActivity : FlutterActivity() {
    companion object {
        var current = WeakReference<MainActivity>(null)
        private const val ENGINE = "orgbot_rooms"
    }
    override fun provideFlutterEngine(context: Context): FlutterEngine? = FlutterEngineCache.getInstance().get(ENGINE)
    override fun shouldDestroyEngineWithHost(): Boolean = false

    fun requestNotifications() {
        val prefs = getSharedPreferences("room_notifications", Context.MODE_PRIVATE)
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED && !prefs.getBoolean("asked", false)) {
            prefs.edit().putBoolean("asked", true).apply()
            requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 803)
        }
    }
    private var pending: MethodChannel.Result? = null
    private var exportText: String? = null
    private val limit = 1024 * 1024
    private val attachments by lazy { RoomAttachments(this) }

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        FlutterEngineCache.getInstance().put(ENGINE, flutterEngine)
        current = WeakReference(this)
        val appContext = applicationContext
        RoomService.configure(flutterEngine, appContext)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "orgbot/attachments")
            .setMethodCallHandler { call, result ->
                current.get()?.attachments?.call(call, result)
                    ?: result.error("attachment", "Open the app to attach a file.", null)
            }
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "orgbot/workspace")
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "directory" -> result.success(File(appContext.filesDir, "workspaces").absolutePath)
                    "chmod" -> {
                        try {
                            val path = requireNotNull(call.argument<String>("path"))
                            val mode = requireNotNull(call.argument<Int>("mode"))
                            require(mode in 0..511)
                            android.system.Os.chmod(path, mode)
                            result.success(null)
                        } catch (e: Exception) {
                            result.error("permissions", "Could not preserve file permissions.", null)
                        }
                    }
                    "import", "export" -> {
                        current.get()?.document(call, result) ?: result.error("picker", "Open the app to choose a document.", null)
                    }
                    else -> result.notImplemented()
                }
            }
    }

    private fun document(call: io.flutter.plugin.common.MethodCall, result: MethodChannel.Result) {
                        if (pending != null) {
                            result.error("busy", "A document picker is already open.", null)
                        } else {
                            val exporting = call.method == "export"
                            val text = call.argument<String>("content") ?: ""
                            if (text.toByteArray(Charsets.UTF_8).size > limit) {
                                result.error("size", "File exceeds 1 MiB.", null)
                            } else {
                                pending = result
                                exportText = if (exporting) text else null
                                val intent = Intent(if (exporting) Intent.ACTION_CREATE_DOCUMENT else Intent.ACTION_OPEN_DOCUMENT)
                                intent.addCategory(Intent.CATEGORY_OPENABLE)
                                intent.type = if (exporting) "text/plain" else "*/*"
                                if (exporting) intent.putExtra(Intent.EXTRA_TITLE, call.argument<String>("name") ?: "document.txt")
                                try { startActivityForResult(intent, if (exporting) 802 else 801) }
                                catch (e: Exception) { pending = null; exportText = null; result.error("picker", "Could not open document picker.", null) }
                            }
                        }
    }

    override fun onDestroy() {
        attachments.close()
        if (current.get() === this) current.clear()
        pending?.error("picker", "Document selection closed.", null)
        pending = null; exportText = null
        super.onDestroy()
    }

    @Deprecated("Activity result callback used by FlutterActivity")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (attachments.onResult(requestCode, resultCode, data)) return
        if (requestCode != 801 && requestCode != 802) return
        val result = pending ?: return
        val content = exportText
        pending = null; exportText = null
        val uri = data?.data
        if (resultCode != Activity.RESULT_OK || uri == null) { result.success(null); return }
        // Document providers can be remote: never block Flutter's UI thread on I/O.
        Thread {
            try {
                val value: Any? = if (requestCode == 802) {
                    val stream = contentResolver.openOutputStream(uri, "wt") ?: error("Cannot write document")
                    stream.use { it.write((content ?: "").toByteArray(Charsets.UTF_8)) }
                    true
                } else {
                    val stream = contentResolver.openInputStream(uri) ?: error("Cannot read document")
                    val bytes = stream.use { input ->
                        val output = java.io.ByteArrayOutputStream()
                        val buffer = ByteArray(8192)
                        while (output.size() <= limit) {
                            val count = input.read(buffer, 0, minOf(buffer.size, limit + 1 - output.size()))
                            if (count < 0) break
                            output.write(buffer, 0, count)
                        }
                        output.toByteArray()
                    }
                    require(bytes.size <= limit) { "File exceeds 1 MiB" }
                    val text = Charsets.UTF_8.newDecoder().onMalformedInput(CodingErrorAction.REPORT).decode(ByteBuffer.wrap(bytes)).toString()
                    var name = "document.txt"
                    contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                        if (it.moveToFirst()) name = it.getString(0).substringAfterLast('/').substringAfterLast('\\')
                    }
                    mapOf("name" to name, "content" to text)
                }
                runOnUiThread { result.success(value) }
            } catch (e: Exception) {
                runOnUiThread { result.error("document", "Could not read or write this UTF-8 document (maximum 1 MiB).", null) }
            }
        }.start()
    }
}

package com.orgbot.app

import android.app.Activity
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Matrix
import android.graphics.pdf.PdfRenderer
import android.media.ExifInterface
import android.os.Build
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.util.Xml
import androidx.core.content.FileProvider
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import org.json.JSONObject
import org.xmlpull.v1.XmlPullParser
import java.io.File
import java.io.InputStream
import java.nio.ByteBuffer
import java.nio.charset.CodingErrorAction
import java.util.UUID
import java.util.zip.ZipFile
import kotlin.math.roundToInt

/** User-selected files stay private and travel with the saved room. */
class RoomAttachments(private val activity: MainActivity) {
    private var pending: MethodChannel.Result? = null
    private var room: String? = null
    private val fileLimit = 20 * 1024 * 1024
    private val textLimit = 120_000

    private fun directory(roomId: String, id: String? = null): File {
        require(roomId.matches(Regex("room_[A-Za-z0-9_-]{1,96}"))) { "Invalid room" }
        val root = File(activity.filesDir, "workspaces/.history/$roomId/attachments")
        if (id == null) return root
        require(id.matches(Regex("att_[0-9a-f]{32}"))) { "Invalid attachment" }
        return File(root, id)
    }

    fun call(call: MethodCall, result: MethodChannel.Result) {
        try {
            val roomId = requireNotNull(call.argument<String>("room"))
            val root = directory(roomId)
            when (call.method) {
                "pick" -> {
                    check(pending == null) { "A file picker is already open" }
                    pending = result; room = roomId
                    val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
                    intent.type = "*/*"
                    intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
                    try { activity.startActivityForResult(intent, 804) }
                    catch (e: Exception) { pending = null; room = null; throw e }
                }
                "prepare" -> background(result) {
                    prepare(directory(roomId, requireNotNull(call.argument<String>("id"))))
                }
                "open" -> {
                    val folder = directory(roomId, requireNotNull(call.argument<String>("id")))
                    val manifest = JSONObject(File(folder, "manifest.json").readText())
                    val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.attachments", File(folder, "original"))
                    activity.startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(uri, manifest.getString("mime_type"))
                        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
                    result.success(null)
                }
                "directory" -> result.success(root.absolutePath)
                else -> result.notImplemented()
            }
        } catch (e: Exception) { result.error("attachment", e.message ?: "Could not open attachment", null) }
    }

    fun close() {
        pending?.error("attachment", "File selection closed. Try again.", null)
        pending = null; room = null
    }

    fun onResult(request: Int, status: Int, data: Intent?): Boolean {
        if (request != 804) return false
        val result = pending ?: return true
        val roomId = room ?: return true
        pending = null; room = null
        if (status != Activity.RESULT_OK) { result.success(emptyList<Any>()); return true }
        val uris = data?.clipData?.let { clip -> (0 until clip.itemCount).map { clip.getItemAt(it).uri } }
            ?: listOfNotNull(data?.data)
        background(result) {
            require(uris.size <= 5) { "Choose up to five files at a time" }
            val created = mutableListOf<File>()
            try {
                uris.map { uri ->
                    var name = "attachment"
                    activity.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                        if (it.moveToFirst()) name = it.getString(0) ?: name
                    }
                    name = name.substringAfterLast('/').substringAfterLast('\\').filter { !it.isISOControl() }.take(120).ifBlank { "attachment" }
                    val id = "att_" + UUID.randomUUID().toString().replace("-", "")
                    val folder = directory(roomId, id)
                    check(folder.mkdirs()) { "Cannot create attachment storage" }
                    created.add(folder)
                    val original = File(folder, "original")
                    (activity.contentResolver.openInputStream(uri) ?: error("Cannot read $name")).use { input ->
                        original.outputStream().use { output ->
                            val buffer = ByteArray(16384); var total = 0
                            while (true) {
                                val n = input.read(buffer)
                                if (n < 0) break
                                total += n
                                require(total <= fileLimit) { "$name exceeds 20 MiB" }
                                output.write(buffer, 0, n)
                            }
                        }
                    }
                    val manifest = JSONObject().put("id", id).put("name", name)
                        .put("mime_type", activity.contentResolver.getType(uri) ?: "application/octet-stream")
                        .put("bytes", original.length()).put("has_text", false).put("images", 0).put("note", "")
                    save(folder, manifest)
                    manifestMap(manifest)
                }
            } catch (e: Exception) { created.forEach { it.deleteRecursively() }; throw e }
        }
        return true
    }

    private fun background(result: MethodChannel.Result, work: () -> Any?) {
        Thread {
            try { val value = work(); activity.runOnUiThread { result.success(value) } }
            catch (e: Exception) { activity.runOnUiThread { result.error("attachment", e.message ?: "Could not prepare attachment", null) } }
        }.start()
    }

    private fun save(folder: File, value: JSONObject) {
        val temp = File(folder, "manifest.tmp")
        temp.writeText(value.toString())
        check(temp.renameTo(File(folder, "manifest.json"))) { "Could not save attachment" }
    }
    private fun manifestMap(value: JSONObject): Map<String, Any> = value.keys().asSequence().associateWith { value.get(it) }

    private fun prepare(folder: File): Map<String, Any> {
        val original = File(folder, "original")
        require(original.length() in 1..fileLimit.toLong()) { "File is empty or exceeds 20 MiB" }
        val manifest = JSONObject(File(folder, "manifest.json").readText())
        val ext = manifest.getString("name").substringAfterLast('.', "").lowercase()
        val mime = manifest.getString("mime_type")
        var text = ""; var images = 0; var note = ""
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFile(original.path, bounds)
        when {
            bounds.outWidth > 0 && bounds.outHeight > 0 -> {
                var sample = 1
                while (maxOf(bounds.outWidth, bounds.outHeight) / sample > 2048) sample *= 2
                val bitmap = BitmapFactory.decodeFile(original.path, BitmapFactory.Options().apply { inSampleSize = sample })
                    ?: error("Cannot decode this image")
                val orientation = try { ExifInterface(original.path).getAttributeInt(ExifInterface.TAG_ORIENTATION, 1) } catch (_: Exception) { 1 }
                val matrix = Matrix().apply {
                    when (orientation) {
                        2 -> setScale(-1f, 1f)
                        3 -> setRotate(180f)
                        4 -> setScale(1f, -1f)
                        5 -> { setRotate(90f); postScale(-1f, 1f) }
                        6 -> setRotate(90f)
                        7 -> { setRotate(270f); postScale(-1f, 1f) }
                        8 -> setRotate(270f)
                    }
                }
                val oriented = Bitmap.createBitmap(bitmap, 0, 0, bitmap.width, bitmap.height, matrix, true)
                try { saveImage(oriented, File(folder, "image-0.jpg")) }
                finally { if (oriented !== bitmap) oriented.recycle(); bitmap.recycle() }
                images = 1; note = "Image included for vision models."
                manifest.put("mime_type", bounds.outMimeType ?: mime)
            }
            ext == "pdf" || mime == "application/pdf" -> {
                val content = StringBuilder()
                PdfRenderer(ParcelFileDescriptor.open(original, ParcelFileDescriptor.MODE_READ_ONLY)).use { pdf ->
                    val pages = minOf(pdf.pageCount, 100)
                    for (index in 0 until pages) {
                        if (content.length >= textLimit) break
                        pdf.openPage(index).use { page ->
                            val pageText = if (Build.VERSION.SDK_INT >= 35) page.textContents.joinToString("\n") { it.text } else ""
                            content.append("\n[Page ${index + 1}]\n")
                            if (pageText.isNotBlank()) content.append(pageText.take((textLimit - content.length).coerceAtLeast(0)))
                            else if (images < 8) {
                                val scale = 1600.0 / maxOf(page.width, page.height).coerceAtLeast(1)
                                val bitmap = Bitmap.createBitmap((page.width * scale).roundToInt().coerceAtLeast(1), (page.height * scale).roundToInt().coerceAtLeast(1), Bitmap.Config.ARGB_8888)
                                try { bitmap.eraseColor(Color.WHITE); page.render(bitmap, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                                    saveImage(bitmap, File(folder, "image-${images++}.jpg"))
                                } finally { bitmap.recycle() }
                                content.append("Page included as image $images; a vision model is required.\n")
                            } else content.append("Page image omitted; send the relevant pages separately.\n")
                        }
                    }
                    note = "${pdf.pageCount} PDF pages. Extracted text and up to eight scanned-page images; complex layouts may need the original."
                    if (pdf.pageCount > pages || content.length >= textLimit) note += " Preview limited to 100 pages / 120,000 characters."
                }
                text = content.toString()
            }
            ext in setOf("docx", "pptx", "xlsx", "odt", "ods", "odp") -> {
                text = officeText(original, ext)
                note = "Extracted document text; layout, charts, embedded images and formulas may need the original."
                if (text.length >= textLimit) note += " Text preview limited to 120,000 characters."
            }
            else -> {
                try {
                    original.inputStream().use { input ->
                        text = Charsets.UTF_8.newDecoder().onMalformedInput(CodingErrorAction.REPORT)
                            .decode(ByteBuffer.wrap(bounded(input, 512 * 1024))).toString()
                        require(text.none { it == '\u0000' })
                    }
                    if (text.length > textLimit) { text = text.take(textLimit); note = "Text preview limited to 120,000 characters." }
                } catch (_: Exception) {
                    text = ""; note = "Original file saved. No automatic text extraction for this format; bots can copy it to the working computer for inspection."
                }
            }
        }
        if (text.isNotEmpty()) File(folder, "context.txt").writeText(text)
        manifest.put("has_text", text.isNotEmpty()).put("images", images).put("note", note)
        save(folder, manifest)
        return manifestMap(manifest)
    }

    private fun saveImage(bitmap: Bitmap, target: File) {
        val flat = Bitmap.createBitmap(bitmap.width, bitmap.height, Bitmap.Config.RGB_565)
        try { val canvas = Canvas(flat); canvas.drawColor(Color.WHITE); canvas.drawBitmap(bitmap, 0f, 0f, null)
            target.outputStream().use { check(flat.compress(Bitmap.CompressFormat.JPEG, 85, it)) }
        } finally { flat.recycle() }
        require(target.length() <= 2 * 1024 * 1024) { "Image preview exceeds 2 MiB" }
    }
    private fun bounded(input: InputStream, limit: Int): ByteArray {
        val out = java.io.ByteArrayOutputStream(); val buffer = ByteArray(8192)
        while (true) {
            val n = input.read(buffer)
            if (n < 0) break
            require(out.size() + n <= limit) { "Document content exceeds extraction limit" }
            out.write(buffer, 0, n)
        }
        return out.toByteArray()
    }
    private fun parser(bytes: ByteArray): XmlPullParser {
        val raw = bytes.toString(Charsets.UTF_8)
        require(!raw.contains("<!DOCTYPE", true) && !raw.contains("<!ENTITY", true)) { "Document contains unsupported XML declarations" }
        return Xml.newPullParser().apply { setFeature(XmlPullParser.FEATURE_PROCESS_NAMESPACES, true); setInput(bytes.inputStream(), "UTF-8") }
    }
    private fun xmlText(bytes: ByteArray): String {
        val parser = parser(bytes); val out = StringBuilder()
        while (parser.eventType != XmlPullParser.END_DOCUMENT && out.length < textLimit) {
            if (parser.eventType == XmlPullParser.TEXT) out.append(parser.text)
            if (parser.eventType == XmlPullParser.END_TAG && parser.name in setOf("p", "row", "si", "tr", "table-row")) out.append('\n')
            if (parser.eventType == XmlPullParser.END_TAG && parser.name in setOf("tc", "c", "table-cell")) out.append('\t')
            parser.next()
        }
        return out.toString().take(textLimit)
    }
    private fun officeText(original: File, ext: String): String = ZipFile(original).use { zip ->
        val entries = zip.entries().asSequence().filter { entry ->
            when (ext) {
                "docx" -> entry.name in setOf("word/document.xml", "word/footnotes.xml", "word/endnotes.xml")
                "pptx" -> entry.name.matches(Regex("ppt/(slides/slide|notesSlides/notesSlide)[0-9]+\\.xml"))
                "xlsx" -> entry.name == "xl/sharedStrings.xml" || entry.name.matches(Regex("xl/worksheets/sheet[0-9]+\\.xml"))
                else -> entry.name == "content.xml"
            }
        }.sortedBy { it.name.replace(Regex("[0-9]+")) { n -> n.value.padStart(6, '0') } }.take(100).toList()
        require(entries.isNotEmpty()) { "No readable document content found" }
        val out = StringBuilder(); var total = 0; val shared = mutableListOf<String>()
        for (entry in entries) {
            if (out.length >= textLimit) break
            val bytes = zip.getInputStream(entry).use { bounded(it, 4 * 1024 * 1024 - total) }; total += bytes.size
            if (ext != "xlsx") { out.append("\n[${entry.name}]\n").append(xmlText(bytes)); continue }
            val p = parser(bytes)
            var cellType = ""; var cell = ""; var valueTag = ""; val value = StringBuilder()
            if (entry.name.contains("worksheets/")) out.append("\n[${entry.name}]\n")
            while (p.eventType != XmlPullParser.END_DOCUMENT && out.length < textLimit) {
                when (p.eventType) {
                    XmlPullParser.START_TAG -> when (p.name) {
                        "si" -> value.setLength(0)
                        "c" -> { cellType = p.getAttributeValue(null, "t") ?: ""; cell = p.getAttributeValue(null, "r") ?: ""; value.setLength(0) }
                        "v", "t" -> valueTag = p.name
                    }
                    XmlPullParser.TEXT -> if (valueTag.isNotEmpty()) value.append(p.text)
                    XmlPullParser.END_TAG -> when (p.name) {
                        "v", "t" -> valueTag = ""
                        "si" -> shared.add(value.toString())
                        "c" -> { val raw = value.toString(); val text = if (cellType == "s") shared.getOrNull(raw.toIntOrNull() ?: -1) ?: "[missing shared string]" else raw
                            out.append(cell).append('=').append(text).append('\t') }
                        "row" -> out.append('\n')
                    }
                }
                p.next()
            }
        }
        out.toString().take(textLimit)
    }
}

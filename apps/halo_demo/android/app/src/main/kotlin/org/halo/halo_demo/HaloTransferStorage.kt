package org.halo.halo_demo

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import java.io.File
import java.io.FileOutputStream
import java.util.UUID

/**
 * Adapts Android document URIs to app-private paths consumed by Rust. File
 * bytes are copied natively and never cross a Flutter method channel.
 */
class HaloTransferStorage(private val context: Context) {
    private val root = File(context.noBackupFilesDir, ROOT_DIRECTORY)
    private val staging = File(root, STAGING_DIRECTORY)
    private val received = File(root, RECEIVED_DIRECTORY)
    private val outgoing = File(root, OUTGOING_DIRECTORY)

    fun directories(): Map<String, String> {
        ensureDirectories()
        return mapOf(
            "staging" to staging.absolutePath,
            "destination" to received.absolutePath,
        )
    }

    fun copySelectedFile(uri: Uri): Map<String, String> {
        ensureDirectories()
        val displayName = displayName(uri) ?: "selected-file"
        val identifier = UUID.randomUUID().toString()
        val partial = File(outgoing, ".$identifier.part")
        val finalFile = File(outgoing, "$identifier.upload")
        try {
            val input = context.contentResolver.openInputStream(uri)
                ?: throw TransferStorageException("document stream is unavailable")
            input.use { source ->
                FileOutputStream(partial).use { destination ->
                    val buffer = ByteArray(BUFFER_SIZE)
                    var total = 0L
                    while (true) {
                        val count = source.read(buffer)
                        if (count < 0) break
                        total += count
                        if (total > MAX_FILE_SIZE) {
                            throw TransferStorageException("selected file exceeds the transfer limit")
                        }
                        destination.write(buffer, 0, count)
                    }
                    destination.fd.sync()
                }
            }
            if (!partial.renameTo(finalFile)) {
                throw TransferStorageException("private file finalization failed")
            }
            return mapOf("path" to finalFile.absolutePath, "name" to displayName)
        } catch (error: Exception) {
            partial.delete()
            finalFile.delete()
            throw error
        }
    }

    fun discardOutgoing(path: String) {
        ensureDirectories()
        val candidate = File(path).canonicalFile
        if (candidate.parentFile != outgoing.canonicalFile || !candidate.name.endsWith(".upload")) {
            throw TransferStorageException("transfer source is outside private outgoing storage")
        }
        if (candidate.exists() && !candidate.delete()) {
            throw TransferStorageException("private transfer source could not be removed")
        }
    }

    private fun ensureDirectories() {
        for (directory in listOf(staging, received, outgoing)) {
            if ((!directory.exists() && !directory.mkdirs()) || !directory.isDirectory) {
                throw TransferStorageException("private transfer directory is unavailable")
            }
        }
    }

    private fun displayName(uri: Uri): String? {
        return context.contentResolver.query(
            uri,
            arrayOf(OpenableColumns.DISPLAY_NAME),
            null,
            null,
            null,
        )?.use { cursor ->
            if (!cursor.moveToFirst()) return@use null
            val column = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
            if (column < 0) null else cursor.getString(column)
        }
    }

    companion object {
        private const val ROOT_DIRECTORY = "halo-transfer-v1"
        private const val STAGING_DIRECTORY = "staging"
        private const val RECEIVED_DIRECTORY = "received"
        private const val OUTGOING_DIRECTORY = "outgoing"
        private const val BUFFER_SIZE = 64 * 1024
        private const val MAX_FILE_SIZE = 10L * 1024 * 1024 * 1024 * 1024
    }
}

class TransferStorageException(message: String) : Exception(message)

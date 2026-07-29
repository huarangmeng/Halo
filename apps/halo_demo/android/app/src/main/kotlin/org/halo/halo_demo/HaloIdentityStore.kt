package org.halo.halo_demo

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Protects Rust's opaque identity blob with a non-exportable Android Keystore
 * AES key. This class never parses or creates the device identity itself.
 */
class HaloIdentityStore(context: Context) {
    private val identityFile = AtomicFile(File(context.noBackupFilesDir, IDENTITY_FILE))
    val trustStoreDirectory: String =
        File(context.noBackupFilesDir, TRUST_DIRECTORY).absolutePath

    @Synchronized
    fun load(): ByteArray? {
        if (!identityFile.baseFile.exists()) return null
        val key = keyStore().getKey(KEY_ALIAS, null) as? SecretKey
            ?: throw IdentityStorageException("protected key is unavailable")
        val encoded = identityFile.openRead().use { it.readBytes() }
        if (encoded.size < MAGIC.size + 1 + NONCE_LENGTH + TAG_LENGTH_BYTES ||
            !encoded.copyOfRange(0, MAGIC.size).contentEquals(MAGIC)
        ) {
            throw IdentityStorageException("protected identity is corrupt")
        }
        val nonceLength = encoded[MAGIC.size].toInt() and 0xff
        if (nonceLength != NONCE_LENGTH) {
            throw IdentityStorageException("protected identity is corrupt")
        }
        val nonceStart = MAGIC.size + 1
        val ciphertextStart = nonceStart + nonceLength
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(
            Cipher.DECRYPT_MODE,
            key,
            GCMParameterSpec(TAG_LENGTH_BITS, encoded.copyOfRange(nonceStart, ciphertextStart)),
        )
        cipher.updateAAD(AAD)
        return try {
            cipher.doFinal(encoded.copyOfRange(ciphertextStart, encoded.size)).also {
                if (it.isEmpty() || it.size > MAX_BLOB_LENGTH) {
                    throw IdentityStorageException("protected identity is corrupt")
                }
            }
        } catch (error: IdentityStorageException) {
            throw error
        } catch (_: Exception) {
            throw IdentityStorageException("protected identity authentication failed")
        }
    }

    @Synchronized
    fun save(blob: ByteArray) {
        if (blob.isEmpty() || blob.size > MAX_BLOB_LENGTH) {
            throw IdentityStorageException("identity blob length is invalid")
        }
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, loadOrCreateKey())
        cipher.updateAAD(AAD)
        val ciphertext = cipher.doFinal(blob)
        val encoded = MAGIC + byteArrayOf(cipher.iv.size.toByte()) + cipher.iv + ciphertext
        val stream = identityFile.startWrite()
        try {
            stream.write(encoded)
            stream.fd.sync()
            identityFile.finishWrite(stream)
        } catch (error: Exception) {
            identityFile.failWrite(stream)
            throw IdentityStorageException("protected identity write failed", error)
        }
    }

    @Synchronized
    fun delete() {
        identityFile.delete()
        val store = keyStore()
        if (store.containsAlias(KEY_ALIAS)) store.deleteEntry(KEY_ALIAS)
    }

    private fun loadOrCreateKey(): SecretKey {
        val store = keyStore()
        (store.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(
            KeyProperties.KEY_ALGORITHM_AES,
            "AndroidKeyStore",
        )
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setRandomizedEncryptionRequired(true)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }

    private fun keyStore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply {
        load(null)
    }

    companion object {
        private const val KEY_ALIAS = "org.halo.identity.wrap.v1"
        private const val IDENTITY_FILE = "halo-identity-v1.bin"
        private const val TRUST_DIRECTORY = "halo-trust-v1"
        private const val TRANSFORMATION = "AES/GCM/NoPadding"
        private const val NONCE_LENGTH = 12
        private const val TAG_LENGTH_BITS = 128
        private const val TAG_LENGTH_BYTES = TAG_LENGTH_BITS / 8
        private const val MAX_BLOB_LENGTH = 256
        private val MAGIC = byteArrayOf(0x48, 0x49, 0x42, 0x31)
        private val AAD = "Halo Identity Blob v1".toByteArray(Charsets.UTF_8)
    }
}

class IdentityStorageException(message: String, cause: Throwable? = null) :
    Exception(message, cause)

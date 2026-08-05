package org.halo.halo_demo

import android.os.ParcelFileDescriptor
import java.net.DatagramSocket

/** Transfers an already-bound UDP socket directly from Android to Rust. */
internal object HaloNativeSocketBridge {
    fun registerBoundSocket(socket: DatagramSocket): Boolean {
        val descriptor = try {
            ParcelFileDescriptor.fromDatagramSocket(socket).detachFd()
        } catch (_: RuntimeException) {
            return false
        }
        return try {
            nativeRegisterBoundSocket(descriptor) == STATUS_OK
        } catch (_: Throwable) {
            // The JNI function did not resolve, so native code could not have
            // accepted ownership of the detached descriptor.
            runCatching { ParcelFileDescriptor.adoptFd(descriptor).close() }
            false
        }
    }

    fun disableLan(): Boolean = try {
        nativeDisableLan() == STATUS_OK
    } catch (_: Throwable) {
        false
    }

    private external fun nativeRegisterBoundSocket(fileDescriptor: Int): Int

    private external fun nativeDisableLan(): Int

    private const val STATUS_OK = 0
}

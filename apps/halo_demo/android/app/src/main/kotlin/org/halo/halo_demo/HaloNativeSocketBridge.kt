package org.halo.halo_demo

import android.os.ParcelFileDescriptor
import java.net.DatagramSocket

/** Transfers an already-bound UDP socket directly from Android to Rust. */
internal object HaloNativeSocketBridge {
    fun registerBoundSockets(
        socket: DatagramSocket,
        discoverySocket: DatagramSocket,
    ): Boolean {
        return transferSockets(socket, discoverySocket, ::nativeRegisterBoundSocket)
    }

    fun registerUserApprovedHotspotSockets(
        socket: DatagramSocket,
        discoverySocket: DatagramSocket,
    ): Boolean {
        return transferSockets(
            socket,
            discoverySocket,
            ::nativeRegisterUserApprovedHotspotSocket,
        )
    }

    private fun transferSockets(
        socket: DatagramSocket,
        discoverySocket: DatagramSocket,
        register: (Int, Int) -> Int,
    ): Boolean {
        var descriptor = -1
        var discoveryDescriptor = -1
        try {
            descriptor = ParcelFileDescriptor.fromDatagramSocket(socket).detachFd()
            discoveryDescriptor =
                ParcelFileDescriptor.fromDatagramSocket(discoverySocket).detachFd()
        } catch (_: RuntimeException) {
            closeDetached(descriptor)
            closeDetached(discoveryDescriptor)
            return false
        }
        return try {
            register(descriptor, discoveryDescriptor) == STATUS_OK
        } catch (_: Throwable) {
            // The JNI function did not resolve, so native code could not have
            // accepted ownership of the detached descriptors.
            closeDetached(descriptor)
            closeDetached(discoveryDescriptor)
            false
        }
    }

    private fun closeDetached(descriptor: Int) {
        if (descriptor >= 0) {
            runCatching { ParcelFileDescriptor.adoptFd(descriptor).close() }
        }
    }

    fun disableLan(): Boolean = try {
        nativeDisableLan() == STATUS_OK
    } catch (_: Throwable) {
        false
    }

    private external fun nativeRegisterBoundSocket(
        fileDescriptor: Int,
        discoveryFileDescriptor: Int,
    ): Int

    private external fun nativeRegisterUserApprovedHotspotSocket(
        fileDescriptor: Int,
        discoveryFileDescriptor: Int,
    ): Int

    private external fun nativeDisableLan(): Int

    private const val STATUS_OK = 0
}

package org.halo.discovery.android

import android.Manifest
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattConnectionSettings
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothGattServer
import android.bluetooth.BluetoothGattServerCallback
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.ParcelUuid
import android.os.SystemClock
import java.io.Closeable
import java.util.ArrayDeque
import java.util.UUID
import java.util.concurrent.Executor

private const val HALO_BLE_PRESENCE_LENGTH = 58
private const val HALO_BLE_LOCAL_NAME = "Halo"

public enum class HaloBleState {
    STARTING,
    READY,
    DEGRADED,
    BLUETOOTH_OFF,
    PERMISSION_REQUIRED,
    UNSUPPORTED,
    STOPPED,
}

public enum class HaloWakeLanStatus(public val wireValue: Byte) {
    SCHEDULED(0),
    NO_LAN_PROVIDER(1),
    RATE_LIMITED(2),
    MALFORMED(3),
}

public sealed interface HaloBleEvent {
    public data class StateChanged(val state: HaloBleState) : HaloBleEvent

    /** `peerHandle` is random for this provider run and must not be persisted as identity. */
    public data class Presence(
        val peerHandle: UUID,
        val descriptor: ByteArray,
        val rssi: Int,
    ) : HaloBleEvent {
        override fun equals(other: Any?): Boolean =
            other is Presence && peerHandle == other.peerHandle &&
                descriptor.contentEquals(other.descriptor) && rssi == other.rssi

        override fun hashCode(): Int = 31 * peerHandle.hashCode() + descriptor.contentHashCode()
    }

    public data class Diagnostic(val operation: String, val detail: String) : HaloBleEvent
}

public fun interface HaloBleEventListener {
    public fun onEvent(event: HaloBleEvent)
}

public fun interface HaloWakeLanHandler {
    public fun onWakeLanRequested(nonce: ByteArray): HaloWakeLanStatus
}

public data class HaloBleConfiguration(
    val presence: ByteArray,
    val maximumConcurrentGattConnections: Int = 2,
    val refreshIntervalMillis: Long = 10_000,
    val connectionTimeoutMillis: Long = 8_000,
    val hardwareFilterFallbackMillis: Long = 3_000,
    val scanRetryBaseMillis: Long = 2_000,
    val scanRetryMaximumMillis: Long = 30_000,
    val peerRetryBaseMillis: Long = 2_000,
    val peerRetryMaximumMillis: Long = 60_000,
    val advertisingRetryBaseMillis: Long = 2_000,
    val advertisingRetryMaximumMillis: Long = 30_000,
) {
    init {
        require(presence.size == HALO_BLE_PRESENCE_LENGTH) {
            "presence must be exactly 58 opaque bytes"
        }
        require(maximumConcurrentGattConnections in 1..8)
        require(refreshIntervalMillis >= 1_000)
        require(connectionTimeoutMillis in 1_000..60_000)
        require(hardwareFilterFallbackMillis in 1_000..30_000)
        require(scanRetryBaseMillis in 1_000..30_000)
        require(scanRetryMaximumMillis in scanRetryBaseMillis..120_000)
        require(peerRetryBaseMillis in 1_000..30_000)
        require(peerRetryMaximumMillis in peerRetryBaseMillis..300_000)
        require(advertisingRetryBaseMillis in 1_000..30_000)
        require(advertisingRetryMaximumMillis in advertisingRetryBaseMillis..120_000)
    }
}

/**
 * Foreground Android BLE rendezvous provider.
 *
 * It concurrently scans, advertises, serves Presence over GATT, and reads the
 * Presence characteristic from nearby peers. BLE output is an untrusted hint;
 * callers must submit it to the Rust discovery core before using any endpoint.
 * Listener callbacks are serialized on the provider's private HandlerThread.
 */
public class HaloBleProvider(
    context: Context,
    private val configuration: HaloBleConfiguration,
    private val listener: HaloBleEventListener,
    private val wakeLanHandler: HaloWakeLanHandler,
) : Closeable {
    private val applicationContext = context.applicationContext
    private val workerThread = HandlerThread("halo-ble-discovery").apply { start() }
    private val handler = Handler(workerThread.looper)
    private val bluetoothManager =
        applicationContext.getSystemService(BluetoothManager::class.java)

    private var started = false
    private var receiverRegistered = false
    private var scanning = false
    private var scanHealthy = false
    private var scanRetryAttempt = 0
    private var usingHardwareScanFilter = false
    private var advertising = false
    private var advertisingRetryAttempt = 0
    private var restartGattServerOnRetry = false
    private var gattServer: BluetoothGattServer? = null
    private var presenceCharacteristic: BluetoothGattCharacteristic? = null
    private var wakeCharacteristic: BluetoothGattCharacteristic? = null
    private var currentPresence = configuration.presence.copyOf()

    private val devices = mutableMapOf<String, BluetoothDevice>()
    private val handles = mutableMapOf<String, UUID>()
    private val latestRssi = mutableMapOf<String, Int>()
    private val pending = ArrayDeque<String>()
    private val activeGatts = mutableMapOf<String, BluetoothGatt>()
    private val connectionTimeouts = mutableMapOf<String, Runnable>()
    private val lastReadAt = mutableMapOf<String, Long>()
    private val peerRetryAttempts = mutableMapOf<String, Int>()
    private val peerRetryNotBefore = mutableMapOf<String, Long>()
    private val serverConnections = mutableSetOf<String>()
    private val subscribedDevices = mutableMapOf<String, BluetoothDevice>()
    private val lastWakeAt = mutableMapOf<String, Long>()
    private var lastGlobalWakeAt = 0L
    private val scanFilterFallback = Runnable(::switchToSoftwareFilteredScan)
    private val scanAfterFilterFallback = Runnable {
        val adapter = bluetoothManager?.adapter
        if (started && adapter?.isEnabled == true) {
            startScan(adapter, useHardwareFilter = false)
            emitDiagnostic(
                "scan-fallback",
                "hardware UUID filter found no Halo peer; software filtering enabled",
            )
        }
    }
    private val scanRegistrationConfirmation = Runnable {
        if (started && scanning) markScanHealthy()
    }
    private val scanRetry = Runnable(::retryScan)
    private val advertisingRetry = Runnable(::retryAdvertising)

    public fun start() {
        handler.post(::startInternal)
    }

    public fun stop() {
        handler.post(::stopInternal)
    }

    /**
     * Releases every Android Bluetooth registration before invoking [onComplete].
     * Callers replacing a provider must wait for this callback before starting
     * another instance, otherwise vendor stacks may retain duplicate scanners.
     */
    public fun shutdown(onComplete: () -> Unit) {
        val accepted = handler.post {
            stopInternal()
            onComplete()
            workerThread.quitSafely()
        }
        if (!accepted) onComplete()
    }

    public fun updatePresence(presence: ByteArray) {
        require(presence.size == HALO_BLE_PRESENCE_LENGTH) {
            "presence must be exactly 58 opaque bytes"
        }
        handler.post {
            currentPresence = presence.copyOf()
            notifySubscribedPeers(currentPresence, presenceCharacteristic)
        }
    }

    override fun close() {
        shutdown {}
    }

    /** Re-checks permissions and radio state after returning from Settings. */
    public fun refreshSystemState() {
        handler.post {
            val adapter = bluetoothManager?.adapter
            if (!hasRequiredPermissions()) {
                if (started) stopInternal()
                emitState(HaloBleState.PERMISSION_REQUIRED)
            } else if (adapter?.isEnabled != true) {
                if (started) stopInternal()
                registerBluetoothReceiver()
                emitState(HaloBleState.BLUETOOTH_OFF)
            } else if (!started) {
                startInternal()
            }
        }
    }

    private fun startInternal() {
        if (started) return
        emitState(HaloBleState.STARTING)

        if (!hasRequiredPermissions()) {
            emitState(HaloBleState.PERMISSION_REQUIRED)
            return
        }
        val adapter = bluetoothManager?.adapter
        if (adapter == null || !applicationContext.packageManager.hasSystemFeature(
                PackageManager.FEATURE_BLUETOOTH_LE,
            )
        ) {
            emitState(HaloBleState.UNSUPPORTED)
            return
        }
        if (!adapter.isEnabled) {
            registerBluetoothReceiver()
            emitState(HaloBleState.BLUETOOTH_OFF)
            return
        }

        started = true
        registerBluetoothReceiver()
        startGattServer(adapter)
        startScan(adapter)
    }

    private fun stopInternal() {
        val adapter = bluetoothManager?.adapter
        try {
            // Always ask the stack to unregister this callback. `scanning` may
            // already be false after an asynchronous registration failure.
            adapter?.bluetoothLeScanner?.stopScan(scanCallback)
        } catch (error: SecurityException) {
            emitDiagnostic("scan-stop", error.javaClass.simpleName)
        } catch (error: RuntimeException) {
            emitDiagnostic("scan-stop", error.javaClass.simpleName)
        }
        try {
            adapter?.bluetoothLeAdvertiser?.stopAdvertising(advertiseCallback)
        } catch (error: SecurityException) {
            emitDiagnostic("advertise-stop", error.javaClass.simpleName)
        } catch (error: RuntimeException) {
            emitDiagnostic("advertise-stop", error.javaClass.simpleName)
        }
        activeGatts.values.forEach { gatt ->
            try {
                gatt.disconnect()
            } catch (_: SecurityException) {
                // `close` below still releases the client registration.
            }
            gatt.close()
        }
        serverConnections.forEach { key ->
            try {
                devices[key]?.let { gattServer?.cancelConnection(it) }
            } catch (_: SecurityException) {
                // Closing the server below remains the final cleanup path.
            }
        }
        try {
            gattServer?.clearServices()
        } catch (_: SecurityException) {
            // Permission can be revoked while discovery is active.
        }
        gattServer?.close()
        connectionTimeouts.values.forEach(handler::removeCallbacks)
        handler.removeCallbacks(scanFilterFallback)
        handler.removeCallbacks(scanAfterFilterFallback)
        handler.removeCallbacks(scanRegistrationConfirmation)
        handler.removeCallbacks(scanRetry)
        handler.removeCallbacks(advertisingRetry)
        gattServer = null
        presenceCharacteristic = null
        wakeCharacteristic = null
        scanning = false
        scanHealthy = false
        scanRetryAttempt = 0
        usingHardwareScanFilter = false
        advertising = false
        advertisingRetryAttempt = 0
        restartGattServerOnRetry = false
        started = false
        devices.clear()
        handles.clear()
        latestRssi.clear()
        pending.clear()
        activeGatts.clear()
        connectionTimeouts.clear()
        lastReadAt.clear()
        peerRetryAttempts.clear()
        peerRetryNotBefore.clear()
        serverConnections.clear()
        subscribedDevices.clear()
        lastWakeAt.clear()
        lastGlobalWakeAt = 0L
        unregisterBluetoothReceiver()
        emitState(HaloBleState.STOPPED)
    }

    private fun startScan(adapter: BluetoothAdapter, useHardwareFilter: Boolean = true) {
        val scanner = adapter.bluetoothLeScanner
        if (scanner == null) {
            emitDiagnostic("scan", "BLE scanner unavailable")
            emitState(HaloBleState.DEGRADED)
            return
        }
        val filters = if (useHardwareFilter) {
            listOf(
                ScanFilter.Builder()
                    .setServiceUuid(ParcelUuid(HaloBleUuids.SERVICE))
                    .build(),
                ScanFilter.Builder()
                    .setDeviceName(HALO_BLE_LOCAL_NAME)
                    .build(),
            )
        } else {
            emptyList()
        }
        val settings = ScanSettings.Builder()
            .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
            .setMatchMode(ScanSettings.MATCH_MODE_AGGRESSIVE)
            .setCallbackType(ScanSettings.CALLBACK_TYPE_ALL_MATCHES)
            .build()
        try {
            handler.removeCallbacks(scanRegistrationConfirmation)
            scanner.startScan(filters, settings, scanCallback)
            scanning = true
            scanHealthy = false
            usingHardwareScanFilter = useHardwareFilter
            handler.postDelayed(scanRegistrationConfirmation, SCAN_REGISTRATION_CONFIRM_MILLIS)
            if (useHardwareFilter) {
                handler.postDelayed(
                    scanFilterFallback,
                    configuration.hardwareFilterFallbackMillis,
                )
            }
        } catch (error: SecurityException) {
            emitDiagnostic("scan", error.javaClass.simpleName)
            emitState(HaloBleState.PERMISSION_REQUIRED)
        } catch (error: RuntimeException) {
            emitDiagnostic("scan", error.javaClass.simpleName)
            emitState(HaloBleState.DEGRADED)
            scheduleScanRetry(-1)
        }
    }

    private fun retryScan() {
        if (!started || !hasRequiredPermissions()) return
        val adapter = bluetoothManager?.adapter ?: return
        if (!adapter.isEnabled) return
        emitDiagnostic("scan-retry", "retrying Android BLE scanner registration")
        startScan(adapter, useHardwareFilter = false)
    }

    private fun scheduleScanRetry(errorCode: Int) {
        if (!started) return
        scanRetryAttempt = (scanRetryAttempt + 1).coerceAtMost(MAX_BACKOFF_ATTEMPT)
        val delay = boundedBackoff(
            configuration.scanRetryBaseMillis,
            configuration.scanRetryMaximumMillis,
            scanRetryAttempt,
        )
        handler.removeCallbacks(scanRetry)
        handler.postDelayed(scanRetry, delay)
        emitDiagnostic(
            "scan-retry",
            "Android scan error $errorCode; retrying in ${delay}ms",
        )
    }

    private fun markScanHealthy() {
        if (scanHealthy) return
        scanHealthy = true
        scanRetryAttempt = 0
        handler.removeCallbacks(scanRetry)
        if (advertising) emitState(HaloBleState.READY)
    }

    /**
     * Some Android vendor controllers silently drop valid 128-bit UUID results
     * when a hardware ScanFilter is active. Fall back to an unfiltered radio
     * scan, but accept only advertisements whose parsed record contains Halo's
     * service UUID or the fixed public product marker. A name-only candidate
     * still has to expose the Halo GATT service and a Rust-valid Presence packet
     * before it is emitted upstream.
     */
    private fun switchToSoftwareFilteredScan() {
        if (!started || !scanning || !usingHardwareScanFilter || devices.isNotEmpty()) return
        val adapter = bluetoothManager?.adapter ?: return
        val scanner = adapter.bluetoothLeScanner ?: return
        try {
            scanner.stopScan(scanCallback)
            scanning = false
            scanHealthy = false
            usingHardwareScanFilter = false
            handler.postDelayed(scanAfterFilterFallback, SCAN_RESTART_SETTLE_MILLIS)
        } catch (error: SecurityException) {
            emitDiagnostic("scan-fallback", error.javaClass.simpleName)
            emitState(HaloBleState.PERMISSION_REQUIRED)
        } catch (error: RuntimeException) {
            emitDiagnostic("scan-fallback", error.javaClass.simpleName)
            emitState(HaloBleState.DEGRADED)
        }
    }

    private fun startGattServer(adapter: BluetoothAdapter) {
        if (adapter.bluetoothLeAdvertiser == null) {
            emitDiagnostic("advertise", "BLE advertising unavailable; scanning remains active")
            emitState(HaloBleState.DEGRADED)
            return
        }
        try {
            val server = bluetoothManager?.openGattServer(applicationContext, serverCallback)
            if (server == null) {
                emitDiagnostic("gatt-server", "failed to open GATT server")
                emitState(HaloBleState.DEGRADED)
                scheduleAdvertisingRetry(restartGattServer = true)
                return
            }
            gattServer = server
            val presence = BluetoothGattCharacteristic(
                HaloBleUuids.PRESENCE,
                BluetoothGattCharacteristic.PROPERTY_READ or
                    BluetoothGattCharacteristic.PROPERTY_NOTIFY,
                BluetoothGattCharacteristic.PERMISSION_READ,
            ).also(::addNotificationDescriptor)
            val wake = BluetoothGattCharacteristic(
                HaloBleUuids.WAKE_LAN,
                BluetoothGattCharacteristic.PROPERTY_WRITE or
                    BluetoothGattCharacteristic.PROPERTY_NOTIFY,
                BluetoothGattCharacteristic.PERMISSION_WRITE,
            ).also(::addNotificationDescriptor)
            val service = BluetoothGattService(
                HaloBleUuids.SERVICE,
                BluetoothGattService.SERVICE_TYPE_PRIMARY,
            ).apply {
                addCharacteristic(presence)
                addCharacteristic(wake)
            }
            presenceCharacteristic = presence
            wakeCharacteristic = wake
            if (!server.addService(service)) {
                emitDiagnostic("gatt-server", "failed to enqueue Halo service")
                emitState(HaloBleState.DEGRADED)
                scheduleAdvertisingRetry(restartGattServer = true)
            }
        } catch (error: SecurityException) {
            emitDiagnostic("gatt-server", error.javaClass.simpleName)
            emitState(HaloBleState.PERMISSION_REQUIRED)
        } catch (error: RuntimeException) {
            emitDiagnostic("gatt-server", error.javaClass.simpleName)
            emitState(HaloBleState.DEGRADED)
            scheduleAdvertisingRetry(restartGattServer = true)
        }
    }

    private fun addNotificationDescriptor(characteristic: BluetoothGattCharacteristic) {
        characteristic.addDescriptor(
            BluetoothGattDescriptor(
                HaloBleUuids.CLIENT_CHARACTERISTIC_CONFIGURATION,
                BluetoothGattDescriptor.PERMISSION_READ or BluetoothGattDescriptor.PERMISSION_WRITE,
            ),
        )
    }

    private fun startAdvertising() {
        val adapter = bluetoothManager?.adapter ?: return
        val advertiser = adapter.bluetoothLeAdvertiser ?: return
        val settings = AdvertiseSettings.Builder()
            .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
            .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_MEDIUM)
            .setConnectable(true)
            .setTimeout(0)
            .build()
        val data = AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .setIncludeTxPowerLevel(false)
            .addServiceUuid(ParcelUuid(HaloBleUuids.SERVICE))
            .build()
        try {
            advertiser.startAdvertising(settings, data, advertiseCallback)
        } catch (error: SecurityException) {
            emitDiagnostic("advertise", error.javaClass.simpleName)
            emitState(HaloBleState.PERMISSION_REQUIRED)
        } catch (error: RuntimeException) {
            emitDiagnostic("advertise", error.javaClass.simpleName)
            emitState(HaloBleState.DEGRADED)
            scheduleAdvertisingRetry(restartGattServer = false)
        }
    }

    private fun scheduleAdvertisingRetry(restartGattServer: Boolean) {
        if (!started) return
        restartGattServerOnRetry = restartGattServerOnRetry || restartGattServer
        advertisingRetryAttempt = (advertisingRetryAttempt + 1).coerceAtMost(MAX_BACKOFF_ATTEMPT)
        val delay = boundedBackoff(
            configuration.advertisingRetryBaseMillis,
            configuration.advertisingRetryMaximumMillis,
            advertisingRetryAttempt,
        )
        handler.removeCallbacks(advertisingRetry)
        handler.postDelayed(advertisingRetry, delay)
        emitDiagnostic("advertise-retry", "retrying BLE advertising in ${delay}ms")
    }

    private fun retryAdvertising() {
        if (!started || !hasRequiredPermissions()) return
        val adapter = bluetoothManager?.adapter ?: return
        if (!adapter.isEnabled) return
        if (restartGattServerOnRetry) {
            restartGattServerOnRetry = false
            try {
                gattServer?.clearServices()
            } catch (_: SecurityException) {
                // Reopening below remains the recovery path.
            }
            gattServer?.close()
            gattServer = null
            presenceCharacteristic = null
            wakeCharacteristic = null
            startGattServer(adapter)
        } else {
            startAdvertising()
        }
    }

    private fun onScanResult(result: ScanResult) {
        if (!started) return
        markScanHealthy()
        if (!isHaloAdvertisement(result)) return
        val key = addressOf(result.device) ?: return
        devices[key] = result.device
        handles.getOrPut(key, UUID::randomUUID)
        latestRssi[key] = result.rssi

        val now = SystemClock.elapsedRealtime()
        val recentlyRead = lastReadAt[key]?.let { now - it < configuration.refreshIntervalMillis } == true
        val retryDeferred = now < (peerRetryNotBefore[key] ?: 0L)
        if (recentlyRead || retryDeferred || activeGatts.containsKey(key) || pending.contains(key)) return
        pending.addLast(key)
        drainConnectionQueue()
    }

    private fun isHaloAdvertisement(result: ScanResult): Boolean {
        val record = result.scanRecord ?: return false
        val haloService = ParcelUuid(HaloBleUuids.SERVICE)
        return record.serviceUuids.orEmpty().contains(haloService) ||
            record.serviceSolicitationUuids.orEmpty().contains(haloService) ||
            record.deviceName == HALO_BLE_LOCAL_NAME
    }

    private fun drainConnectionQueue() {
        if (!started || !hasRequiredPermissions()) return
        while (
            activeGatts.size < configuration.maximumConcurrentGattConnections &&
            pending.isNotEmpty()
        ) {
            val key = pending.removeFirst()
            val device = devices[key] ?: continue
            try {
                val gatt = if (Build.VERSION.SDK_INT >= 37) {
                    val settings = BluetoothGattConnectionSettings.Builder()
                        .setAutoConnectEnabled(false)
                        .setAutomaticMtuEnabled(true)
                        .setTransport(BluetoothDevice.TRANSPORT_LE)
                        .build()
                    device.connectGatt(
                        settings,
                        Executor { command -> handler.post(command) },
                        clientCallback,
                    )
                } else {
                    @Suppress("DEPRECATION")
                    device.connectGatt(
                        applicationContext,
                        false,
                        clientCallback,
                        BluetoothDevice.TRANSPORT_LE,
                        BluetoothDevice.PHY_LE_1M_MASK,
                        handler,
                    )
                }
                if (gatt == null) {
                    emitDiagnostic("connect", "Android rejected the GATT connection request")
                    deferPeerRetry(key)
                    continue
                }
                activeGatts[key] = gatt
                val timeout = Runnable {
                    if (activeGatts[key] === gatt) {
                        emitDiagnostic("connect", "peer did not respond before timeout")
                        finishClient(key, gatt, disconnectFirst = true)
                    }
                }
                connectionTimeouts[key] = timeout
                handler.postDelayed(timeout, configuration.connectionTimeoutMillis)
            } catch (error: SecurityException) {
                emitDiagnostic("connect", error.javaClass.simpleName)
                emitState(HaloBleState.PERMISSION_REQUIRED)
                deferPeerRetry(key)
            } catch (error: RuntimeException) {
                emitDiagnostic("connect", error.javaClass.simpleName)
                deferPeerRetry(key)
            }
        }
    }

    private fun handlePresenceRead(
        gatt: BluetoothGatt,
        value: ByteArray,
        status: Int,
    ) {
        val key = addressOf(gatt.device) ?: return
        if (status != BluetoothGatt.GATT_SUCCESS) {
            emitDiagnostic("presence-read", "GATT status $status")
            finishClient(key, gatt, disconnectFirst = true)
            return
        }
        if (value.size == HALO_BLE_PRESENCE_LENGTH) {
            lastReadAt[key] = SystemClock.elapsedRealtime()
            listener.onEvent(
                HaloBleEvent.Presence(
                    peerHandle = handles.getOrPut(key, UUID::randomUUID),
                    descriptor = value.copyOf(),
                    rssi = latestRssi[key] ?: Int.MIN_VALUE,
                ),
            )
            finishClient(key, gatt, disconnectFirst = true, succeeded = true)
            return
        } else {
            emitDiagnostic("presence-read", "unexpected opaque value length ${value.size}")
        }
        finishClient(key, gatt, disconnectFirst = true)
    }

    private fun finishClient(
        key: String,
        gatt: BluetoothGatt,
        disconnectFirst: Boolean,
        succeeded: Boolean = false,
    ) {
        connectionTimeouts.remove(key)?.let(handler::removeCallbacks)
        activeGatts.remove(key)
        if (succeeded) {
            peerRetryAttempts.remove(key)
            peerRetryNotBefore.remove(key)
        } else {
            deferPeerRetry(key)
        }
        try {
            if (disconnectFirst) gatt.disconnect()
            gatt.close()
        } catch (error: SecurityException) {
            emitDiagnostic("disconnect", error.javaClass.simpleName)
        }
        drainConnectionQueue()
    }

    private fun deferPeerRetry(key: String) {
        val attempt = ((peerRetryAttempts[key] ?: 0) + 1).coerceAtMost(MAX_BACKOFF_ATTEMPT)
        peerRetryAttempts[key] = attempt
        peerRetryNotBefore[key] = SystemClock.elapsedRealtime() + boundedBackoff(
            configuration.peerRetryBaseMillis,
            configuration.peerRetryMaximumMillis,
            attempt,
        )
    }

    private fun notifySubscribedPeers(
        value: ByteArray,
        characteristic: BluetoothGattCharacteristic?,
    ) {
        val server = gattServer ?: return
        val target = characteristic ?: return
        subscribedDevices.values.forEach { device ->
            try {
                if (Build.VERSION.SDK_INT >= 33) {
                    server.notifyCharacteristicChanged(device, target, false, value)
                } else {
                    @Suppress("DEPRECATION")
                    target.value = value
                    @Suppress("DEPRECATION")
                    server.notifyCharacteristicChanged(device, target, false)
                }
            } catch (error: SecurityException) {
                emitDiagnostic("notify", error.javaClass.simpleName)
            }
        }
    }

    private fun wakeStatusFor(key: String, value: ByteArray): HaloWakeLanStatus {
        if (value.size != 8) return HaloWakeLanStatus.MALFORMED
        val now = SystemClock.elapsedRealtime()
        if (now - lastGlobalWakeAt < GLOBAL_WAKE_INTERVAL_MILLIS ||
            now - (lastWakeAt[key] ?: 0L) < PEER_WAKE_INTERVAL_MILLIS
        ) {
            return HaloWakeLanStatus.RATE_LIMITED
        }
        lastGlobalWakeAt = now
        lastWakeAt[key] = now
        return wakeLanHandler.onWakeLanRequested(value.copyOf())
    }

    private fun sendWakeNotification(device: BluetoothDevice, request: ByteArray, status: HaloWakeLanStatus) {
        if (request.size != 8) return
        val response = request + status.wireValue
        val server = gattServer ?: return
        val characteristic = wakeCharacteristic ?: return
        try {
            if (Build.VERSION.SDK_INT >= 33) {
                server.notifyCharacteristicChanged(device, characteristic, false, response)
            } else {
                @Suppress("DEPRECATION")
                characteristic.value = response
                @Suppress("DEPRECATION")
                server.notifyCharacteristicChanged(device, characteristic, false)
            }
        } catch (error: SecurityException) {
            emitDiagnostic("wake-notify", error.javaClass.simpleName)
        }
    }

    private fun registerBluetoothReceiver() {
        if (receiverRegistered) return
        val filter = IntentFilter(BluetoothAdapter.ACTION_STATE_CHANGED)
        if (Build.VERSION.SDK_INT >= 33) {
            applicationContext.registerReceiver(
                bluetoothStateReceiver,
                filter,
                Context.RECEIVER_NOT_EXPORTED,
            )
        } else {
            @Suppress("DEPRECATION")
            applicationContext.registerReceiver(bluetoothStateReceiver, filter)
        }
        receiverRegistered = true
    }

    private fun unregisterBluetoothReceiver() {
        if (!receiverRegistered) return
        try {
            applicationContext.unregisterReceiver(bluetoothStateReceiver)
        } catch (_: IllegalArgumentException) {
            // Already unregistered by the platform during teardown.
        }
        receiverRegistered = false
    }

    private fun hasRequiredPermissions(): Boolean = REQUIRED_PERMISSIONS.all {
        applicationContext.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED
    }

    private fun addressOf(device: BluetoothDevice): String? = try {
        device.address
    } catch (_: SecurityException) {
        null
    }

    private fun emitState(state: HaloBleState) {
        listener.onEvent(HaloBleEvent.StateChanged(state))
    }

    private fun emitDiagnostic(operation: String, detail: String) {
        listener.onEvent(HaloBleEvent.Diagnostic(operation, detail))
    }

    private val bluetoothStateReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            if (intent?.action != BluetoothAdapter.ACTION_STATE_CHANGED) return
            val state = intent.getIntExtra(BluetoothAdapter.EXTRA_STATE, BluetoothAdapter.ERROR)
            handler.post {
                when (state) {
                    BluetoothAdapter.STATE_OFF -> {
                        if (started) stopInternal()
                        registerBluetoothReceiver()
                        emitState(HaloBleState.BLUETOOTH_OFF)
                    }
                    BluetoothAdapter.STATE_ON -> if (!started) startInternal()
                }
            }
        }
    }

    private val scanCallback = object : ScanCallback() {
        override fun onScanResult(callbackType: Int, result: ScanResult) {
            handler.post { onScanResult(result) }
        }

        override fun onBatchScanResults(results: MutableList<ScanResult>) {
            handler.post { results.forEach(::onScanResult) }
        }

        override fun onScanFailed(errorCode: Int) {
            handler.post {
                scanning = false
                scanHealthy = false
                handler.removeCallbacks(scanFilterFallback)
                handler.removeCallbacks(scanRegistrationConfirmation)
                emitDiagnostic("scan", "Android scan error $errorCode")
                emitState(HaloBleState.DEGRADED)
                scheduleScanRetry(errorCode)
            }
        }
    }

    private val advertiseCallback = object : AdvertiseCallback() {
        override fun onStartSuccess(settingsInEffect: AdvertiseSettings?) {
            handler.post {
                advertising = true
                advertisingRetryAttempt = 0
                restartGattServerOnRetry = false
                handler.removeCallbacks(advertisingRetry)
                emitState(if (scanHealthy) HaloBleState.READY else HaloBleState.DEGRADED)
            }
        }

        override fun onStartFailure(errorCode: Int) {
            handler.post {
                advertising = false
                emitDiagnostic("advertise", "Android advertise error $errorCode")
                emitState(HaloBleState.DEGRADED)
                scheduleAdvertisingRetry(restartGattServer = false)
            }
        }
    }

    private val clientCallback = object : BluetoothGattCallback() {
        override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
            handler.post {
                val key = addressOf(gatt.device) ?: return@post
                if (status == BluetoothGatt.GATT_SUCCESS && newState == BluetoothProfile.STATE_CONNECTED) {
                    try {
                        if (!gatt.discoverServices()) {
                            emitDiagnostic("service-discovery", "request was rejected")
                            finishClient(key, gatt, disconnectFirst = true)
                        }
                    } catch (error: SecurityException) {
                        emitDiagnostic("service-discovery", error.javaClass.simpleName)
                        finishClient(key, gatt, disconnectFirst = false)
                    }
                } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                    if (activeGatts[key] === gatt) {
                        if (status != BluetoothGatt.GATT_SUCCESS) {
                            emitDiagnostic("connect", "GATT status $status")
                        }
                        finishClient(key, gatt, disconnectFirst = false)
                    } else {
                        gatt.close()
                    }
                }
            }
        }

        override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
            handler.post {
                val key = addressOf(gatt.device) ?: return@post
                if (status != BluetoothGatt.GATT_SUCCESS) {
                    emitDiagnostic("service-discovery", "GATT status $status")
                    finishClient(key, gatt, disconnectFirst = true)
                    return@post
                }
                val characteristic = gatt.getService(HaloBleUuids.SERVICE)
                    ?.getCharacteristic(HaloBleUuids.PRESENCE)
                if (characteristic == null || !gatt.readCharacteristic(characteristic)) {
                    emitDiagnostic("presence-read", "Presence characteristic unavailable")
                    finishClient(key, gatt, disconnectFirst = true)
                }
            }
        }

        override fun onCharacteristicRead(
            gatt: BluetoothGatt,
            characteristic: BluetoothGattCharacteristic,
            value: ByteArray,
            status: Int,
        ) {
            if (characteristic.uuid == HaloBleUuids.PRESENCE) {
                handler.post { handlePresenceRead(gatt, value.copyOf(), status) }
            }
        }

        @Deprecated("Used by Android 12; Android 13+ invokes the value overload")
        override fun onCharacteristicRead(
            gatt: BluetoothGatt,
            characteristic: BluetoothGattCharacteristic,
            status: Int,
        ) {
            if (Build.VERSION.SDK_INT >= 33 || characteristic.uuid != HaloBleUuids.PRESENCE) return
            @Suppress("DEPRECATION")
            val value = characteristic.value?.copyOf() ?: ByteArray(0)
            handler.post { handlePresenceRead(gatt, value, status) }
        }
    }

    private val serverCallback = object : BluetoothGattServerCallback() {
        override fun onServiceAdded(status: Int, service: BluetoothGattService) {
            handler.post {
                if (service.uuid != HaloBleUuids.SERVICE) return@post
                if (status == BluetoothGatt.GATT_SUCCESS) {
                    startAdvertising()
                } else {
                    emitDiagnostic("gatt-server", "service add status $status")
                    emitState(HaloBleState.DEGRADED)
                    scheduleAdvertisingRetry(restartGattServer = true)
                }
            }
        }

        override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
            handler.post {
                val key = addressOf(device) ?: return@post
                devices[key] = device
                if (status == BluetoothGatt.GATT_SUCCESS && newState == BluetoothProfile.STATE_CONNECTED) {
                    if (serverConnections.size >= configuration.maximumConcurrentGattConnections) {
                        try {
                            gattServer?.cancelConnection(device)
                        } catch (_: SecurityException) {
                            // Permission loss is surfaced by subsequent state checks.
                        }
                    } else {
                        serverConnections.add(key)
                    }
                } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                    serverConnections.remove(key)
                    subscribedDevices.remove(key)
                }
            }
        }

        override fun onCharacteristicReadRequest(
            device: BluetoothDevice,
            requestId: Int,
            offset: Int,
            characteristic: BluetoothGattCharacteristic,
        ) {
            handler.post {
                if (characteristic.uuid != HaloBleUuids.PRESENCE) {
                    gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_FAILURE, offset, null)
                    return@post
                }
                if (offset !in 0..currentPresence.size) {
                    gattServer?.sendResponse(
                        device,
                        requestId,
                        BluetoothGatt.GATT_INVALID_OFFSET,
                        offset,
                        null,
                    )
                    return@post
                }
                gattServer?.sendResponse(
                    device,
                    requestId,
                    BluetoothGatt.GATT_SUCCESS,
                    offset,
                    currentPresence.copyOfRange(offset, currentPresence.size),
                )
            }
        }

        override fun onCharacteristicWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            characteristic: BluetoothGattCharacteristic,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray,
        ) {
            handler.post {
                val key = addressOf(device) ?: return@post
                val validEnvelope =
                    characteristic.uuid == HaloBleUuids.WAKE_LAN && !preparedWrite && offset == 0
                val status = if (validEnvelope) {
                    wakeStatusFor(key, value)
                } else {
                    HaloWakeLanStatus.MALFORMED
                }
                if (responseNeeded) {
                    val gattStatus = if (status == HaloWakeLanStatus.MALFORMED) {
                        BluetoothGatt.GATT_INVALID_ATTRIBUTE_LENGTH
                    } else {
                        BluetoothGatt.GATT_SUCCESS
                    }
                    gattServer?.sendResponse(device, requestId, gattStatus, offset, null)
                }
                sendWakeNotification(device, value, status)
            }
        }

        override fun onDescriptorReadRequest(
            device: BluetoothDevice,
            requestId: Int,
            offset: Int,
            descriptor: BluetoothGattDescriptor,
        ) {
            handler.post {
                val key = addressOf(device) ?: return@post
                val enabled = subscribedDevices.containsKey(key)
                val value = if (enabled) {
                    BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
                } else {
                    BluetoothGattDescriptor.DISABLE_NOTIFICATION_VALUE
                }
                gattServer?.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, value)
            }
        }

        override fun onDescriptorWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            descriptor: BluetoothGattDescriptor,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray,
        ) {
            handler.post {
                val key = addressOf(device) ?: return@post
                val valid = descriptor.uuid == HaloBleUuids.CLIENT_CHARACTERISTIC_CONFIGURATION &&
                    !preparedWrite && offset == 0
                val enabled = value.contentEquals(BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)
                val disabled = value.contentEquals(BluetoothGattDescriptor.DISABLE_NOTIFICATION_VALUE)
                if (valid && enabled) subscribedDevices[key] = device
                if (valid && disabled) subscribedDevices.remove(key)
                if (responseNeeded) {
                    gattServer?.sendResponse(
                        device,
                        requestId,
                        if (valid && (enabled || disabled)) {
                            BluetoothGatt.GATT_SUCCESS
                        } else {
                            BluetoothGatt.GATT_FAILURE
                        },
                        offset,
                        null,
                    )
                }
            }
        }
    }

    public companion object {
        public val REQUIRED_PERMISSIONS: Array<String> = arrayOf(
            Manifest.permission.BLUETOOTH_SCAN,
            Manifest.permission.BLUETOOTH_ADVERTISE,
            Manifest.permission.BLUETOOTH_CONNECT,
            // Some vendor Android builds return no BLE scan results without
            // location permission, including on Android 12 and newer.
            Manifest.permission.ACCESS_FINE_LOCATION,
        )

        private const val GLOBAL_WAKE_INTERVAL_MILLIS = 500L
        private const val PEER_WAKE_INTERVAL_MILLIS = 2_000L
        private const val SCAN_REGISTRATION_CONFIRM_MILLIS = 1_000L
        private const val SCAN_RESTART_SETTLE_MILLIS = 300L
        private const val MAX_BACKOFF_ATTEMPT = 8
    }
}

private fun boundedBackoff(baseMillis: Long, maximumMillis: Long, attempt: Int): Long {
    val shift = (attempt - 1).coerceIn(0, 30)
    return baseMillis.times(1L shl shift).coerceAtMost(maximumMillis)
}

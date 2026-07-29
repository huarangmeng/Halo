package org.halo.halo_demo

import android.Manifest
import android.bluetooth.BluetoothManager
import android.content.Context
import android.content.pm.PackageManager
import android.location.LocationManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.wifi.WifiManager
import android.os.Build
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.embedding.android.FlutterActivity
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import org.halo.discovery.android.HaloBleConfiguration
import org.halo.discovery.android.HaloBleEvent
import org.halo.discovery.android.HaloBleProvider
import org.halo.discovery.android.HaloWakeLanStatus

class MainActivity : FlutterActivity(), EventChannel.StreamHandler {
    private var eventSink: EventChannel.EventSink? = null
    private var provider: HaloBleProvider? = null
    private var permissionResult: MethodChannel.Result? = null
    private var providerTransition = false
    private var providerGeneration = 0L
    private lateinit var identityStore: HaloIdentityStore

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        identityStore = HaloIdentityStore(this)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, METHOD_CHANNEL)
            .setMethodCallHandler(::handleMethodCall)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, IDENTITY_CHANNEL)
            .setMethodCallHandler(::handleIdentityCall)
        EventChannel(flutterEngine.dartExecutor.binaryMessenger, EVENT_CHANNEL)
            .setStreamHandler(this)
    }

    override fun onListen(arguments: Any?, events: EventChannel.EventSink?) {
        eventSink = events
    }

    override fun onCancel(arguments: Any?) {
        eventSink = null
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != PERMISSION_REQUEST) return
        val granted = grantResults.isNotEmpty() &&
            grantResults.all { it == PackageManager.PERMISSION_GRANTED }
        permissionResult?.success(
            if (granted) preparationPayload() else preparationPayload("permission_denied"),
        )
        permissionResult = null
    }

    override fun onResume() {
        super.onResume()
        provider?.refreshSystemState()
    }

    override fun onDestroy() {
        providerGeneration += 1
        provider?.close()
        provider = null
        super.onDestroy()
    }

    private fun handleMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "prepare" -> preparePermissions(result)
            "capabilities" -> result.success(capabilityPayload())
            "start" -> startBle(call, result)
            "updatePresence" -> updatePresence(call, result)
            "stop" -> stopBle(result)
            else -> result.notImplemented()
        }
    }

    private fun handleIdentityCall(call: MethodCall, result: MethodChannel.Result) {
        try {
            when (call.method) {
                "load" -> result.success(identityStore.load())
                "save" -> {
                    val blob = call.argument<ByteArray>("blob")
                    if (blob == null) {
                        result.error("invalid-identity", "Rust identity blob is missing", null)
                    } else {
                        identityStore.save(blob)
                        result.success(null)
                    }
                }
                "delete" -> {
                    identityStore.delete()
                    result.success(null)
                }
                "trustStoreDirectory" -> result.success(identityStore.trustStoreDirectory)
                else -> result.notImplemented()
            }
        } catch (error: IdentityStorageException) {
            result.error("identity-storage", error.message, null)
        } catch (_: Exception) {
            result.error("identity-storage", "Protected identity storage failed", null)
        }
    }

    private fun preparePermissions(result: MethodChannel.Result) {
        val missing = requiredPermissions().filter {
            checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED
        }
        if (missing.isEmpty()) {
            result.success(preparationPayload())
            return
        }
        if (permissionResult != null) {
            result.error("permission-in-progress", "A permission request is already active", null)
            return
        }
        permissionResult = result
        requestPermissions(missing.toTypedArray(), PERMISSION_REQUEST)
    }

    private fun startBle(call: MethodCall, result: MethodChannel.Result) {
        val presence = call.argument<ByteArray>("presence")
        if (presence == null || presence.size != 58) {
            result.error("invalid-presence", "Rust must supply exactly 58 bytes", null)
            return
        }
        if (providerTransition) {
            result.error("provider-transition", "BLE provider is still stopping", null)
            return
        }
        providerTransition = true
        providerGeneration += 1
        val generation = providerGeneration
        val oldProvider = provider
        provider = null
        val startReplacement = {
            runOnUiThread {
                if (isDestroyed) {
                    providerTransition = false
                    result.error("activity-destroyed", "Activity closed during BLE startup", null)
                } else {
                    provider = HaloBleProvider(
                        context = this,
                        configuration = HaloBleConfiguration(presence = presence),
                        listener = { event ->
                            runOnUiThread {
                                if (generation == providerGeneration) emit(event)
                            }
                        },
                        wakeLanHandler = {
                            // Rust does not expose immediate LAN re-announcement yet.
                            HaloWakeLanStatus.NO_LAN_PROVIDER
                        },
                    ).also { it.start() }
                    HaloDiscoveryForegroundService.start(applicationContext)
                    providerTransition = false
                    result.success(null)
                }
            }
        }
        if (oldProvider == null) startReplacement() else oldProvider.shutdown(startReplacement)
    }

    private fun stopBle(result: MethodChannel.Result) {
        if (providerTransition) {
            result.error("provider-transition", "BLE provider is still transitioning", null)
            return
        }
        providerTransition = true
        providerGeneration += 1
        val oldProvider = provider
        provider = null
        if (oldProvider == null) {
            HaloDiscoveryForegroundService.stop(applicationContext)
            providerTransition = false
            result.success(null)
            return
        }
        oldProvider.shutdown {
            runOnUiThread {
                HaloDiscoveryForegroundService.stop(applicationContext)
                providerTransition = false
                result.success(null)
            }
        }
    }

    private fun updatePresence(call: MethodCall, result: MethodChannel.Result) {
        val presence = call.argument<ByteArray>("presence")
        if (presence == null || presence.size != 58) {
            result.error("invalid-presence", "Rust must supply exactly 58 bytes", null)
            return
        }
        provider?.updatePresence(presence)
        result.success(null)
    }

    private fun emit(event: HaloBleEvent) {
        val payload = when (event) {
            is HaloBleEvent.StateChanged -> mapOf(
                "type" to "state",
                "state" to providerStateName(event.state),
                "detail" to providerStateDetail(event.state),
            )
            is HaloBleEvent.Presence -> mapOf(
                "type" to "presence",
                "descriptor" to event.descriptor,
                "rssi" to event.rssi,
            )
            is HaloBleEvent.Diagnostic -> mapOf(
                "type" to "diagnostic",
                "operation" to event.operation,
                "detail" to event.detail,
            )
        }
        eventSink?.success(payload)
    }

    private fun requiredPermissions(): List<String> = buildList {
        addAll(HaloBleProvider.REQUIRED_PERMISSIONS)
        if (Build.VERSION.SDK_INT >= 33) add(Manifest.permission.POST_NOTIFICATIONS)
        if (Build.VERSION.SDK_INT >= 37) add(ACCESS_LOCAL_NETWORK_PERMISSION)
    }

    private fun preparationPayload(forcedReason: String? = null): Map<String, Any> {
        val reason = forcedReason ?: if (!isLocationServiceEnabled()) {
            "location_services_disabled"
        } else {
            "ready"
        }
        return mapOf(
            "ready" to (reason == "ready"),
            "reason" to reason,
            "capabilities" to capabilityPayload(),
        )
    }

    private fun capabilityPayload(): List<Map<String, String>> = listOf(
        bluetoothCapability(),
        wifiCapability(),
        localNetworkCapability(),
        capability(
            "background",
            if (HaloDiscoveryForegroundService.running) "ready" else "stopped",
            if (HaloDiscoveryForegroundService.running) {
                "foreground_service_running"
            } else {
                "foreground_service_stopped"
            },
        ),
    )

    private fun bluetoothCapability(): Map<String, String> {
        val missing = HaloBleProvider.REQUIRED_PERMISSIONS.filter {
            checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED
        }
        if (missing.isNotEmpty()) {
            return capability(
                "bluetooth",
                "permission_required",
                "bluetooth_permission_missing",
            )
        }
        val manager = getSystemService(BluetoothManager::class.java)
        val adapter = manager?.adapter
        if (adapter == null || !packageManager.hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE)) {
            return capability("bluetooth", "unsupported", "ble_unsupported")
        }
        if (!adapter.isEnabled) {
            return capability("bluetooth", "hardware_off", "bluetooth_powered_off")
        }
        if (adapter.bluetoothLeAdvertiser == null) {
            return capability("bluetooth", "degraded", "ble_advertising_unavailable")
        }
        return capability("bluetooth", "ready", "bluetooth_ready")
    }

    private fun wifiCapability(): Map<String, String> {
        return try {
            val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
                ?: return capability("wifi", "unsupported", "wifi_unsupported")
            if (!wifi.isWifiEnabled) {
                return capability("wifi", "hardware_off", "wifi_powered_off")
            }
            val connectivity = getSystemService(ConnectivityManager::class.java)
            val capabilities = connectivity?.getNetworkCapabilities(connectivity.activeNetwork)
            if (capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) == true) {
                capability("wifi", "ready", "wifi_connected")
            } else {
                capability("wifi", "temporarily_unavailable", "wifi_not_connected")
            }
        } catch (_: SecurityException) {
            capability("wifi", "permission_required", "wifi_state_permission_missing")
        } catch (_: RuntimeException) {
            capability("wifi", "temporarily_unavailable", "wifi_state_unavailable")
        }
    }

    private fun localNetworkCapability(): Map<String, String> {
        if (Build.VERSION.SDK_INT >= 37 &&
            checkSelfPermission(ACCESS_LOCAL_NETWORK_PERMISSION) != PackageManager.PERMISSION_GRANTED
        ) {
            return capability("local_network", "permission_required", "local_network_permission_missing")
        }
        return try {
            val connectivity = getSystemService(ConnectivityManager::class.java)
            val networkCapabilities = connectivity?.getNetworkCapabilities(connectivity.activeNetwork)
            val hasLanTransport = networkCapabilities?.let {
                it.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
                    it.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)
            } == true
            if (hasLanTransport) {
                capability("local_network", "ready", "local_network_connected")
            } else {
                capability("local_network", "temporarily_unavailable", "no_local_network_route")
            }
        } catch (_: SecurityException) {
            capability("local_network", "permission_required", "network_state_permission_missing")
        } catch (_: RuntimeException) {
            capability("local_network", "temporarily_unavailable", "network_state_unavailable")
        }
    }

    private fun capability(name: String, state: String, detail: String): Map<String, String> =
        mapOf("name" to name, "state" to state, "detail" to detail)

    private fun providerStateName(state: org.halo.discovery.android.HaloBleState): String =
        when (state) {
            org.halo.discovery.android.HaloBleState.BLUETOOTH_OFF -> "hardware_off"
            org.halo.discovery.android.HaloBleState.PERMISSION_REQUIRED -> "permission_required"
            org.halo.discovery.android.HaloBleState.UNSUPPORTED -> "unsupported"
            else -> state.name.lowercase()
        }

    private fun providerStateDetail(state: org.halo.discovery.android.HaloBleState): String =
        when (state) {
            org.halo.discovery.android.HaloBleState.BLUETOOTH_OFF -> "bluetooth_powered_off"
            org.halo.discovery.android.HaloBleState.PERMISSION_REQUIRED ->
                "bluetooth_permission_missing"
            org.halo.discovery.android.HaloBleState.UNSUPPORTED -> "ble_unsupported"
            org.halo.discovery.android.HaloBleState.DEGRADED -> "ble_operation_degraded"
            org.halo.discovery.android.HaloBleState.STARTING -> "ble_starting"
            org.halo.discovery.android.HaloBleState.READY -> "ble_ready"
            org.halo.discovery.android.HaloBleState.STOPPED -> "ble_stopped"
        }

    private fun isLocationServiceEnabled(): Boolean =
        getSystemService(LocationManager::class.java)?.isLocationEnabled == true

    companion object {
        private const val METHOD_CHANNEL = "org.halo.discovery/ble"
        private const val EVENT_CHANNEL = "org.halo.discovery/ble-events"
        private const val IDENTITY_CHANNEL = "org.halo.identity/storage"
        private const val PERMISSION_REQUEST = 7101
        private const val ACCESS_LOCAL_NETWORK_PERMISSION =
            "android.permission.ACCESS_LOCAL_NETWORK"
    }
}

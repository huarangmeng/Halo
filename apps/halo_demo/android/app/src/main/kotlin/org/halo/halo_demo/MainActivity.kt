package org.halo.halo_demo

import android.content.pm.PackageManager
import android.location.LocationManager
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

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, METHOD_CHANNEL)
            .setMethodCallHandler(::handleMethodCall)
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
            "start" -> startBle(call, result)
            "updatePresence" -> updatePresence(call, result)
            "stop" -> stopBle(result)
            else -> result.notImplemented()
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
            providerTransition = false
            result.success(null)
            return
        }
        oldProvider.shutdown {
            runOnUiThread {
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
                "state" to event.state.name.lowercase(),
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
        if (Build.VERSION.SDK_INT >= 37) add(ACCESS_LOCAL_NETWORK_PERMISSION)
    }

    private fun preparationPayload(forcedReason: String? = null): Map<String, Any> {
        val reason = forcedReason ?: if (!isLocationServiceEnabled()) {
            "location_services_disabled"
        } else {
            "ready"
        }
        return mapOf("ready" to (reason == "ready"), "reason" to reason)
    }

    private fun isLocationServiceEnabled(): Boolean =
        getSystemService(LocationManager::class.java)?.isLocationEnabled == true

    companion object {
        private const val METHOD_CHANNEL = "org.halo.discovery/ble"
        private const val EVENT_CHANNEL = "org.halo.discovery/ble-events"
        private const val PERMISSION_REQUEST = 7101
        private const val ACCESS_LOCAL_NETWORK_PERMISSION =
            "android.permission.ACCESS_LOCAL_NETWORK"
    }
}

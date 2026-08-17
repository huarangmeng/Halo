package org.halo.halo_demo

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.WifiNetworkSpecifier

internal class HaloLocalHotspotJoiner(context: Context) : AutoCloseable {
    sealed interface Result {
        data class Ready(val network: Network) : Result
        data object Unavailable : Result
        data object Lost : Result
        data object Cancelled : Result
        data object InvalidCredentials : Result
        data object Failed : Result
    }

    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private var callback: ConnectivityManager.NetworkCallback? = null
    private var completion: ((Result) -> Unit)? = null

    var network: Network? = null
        private set

    fun join(ssid: String, passphrase: String, completion: (Result) -> Unit) {
        if (callback != null) {
            completion(Result.Failed)
            return
        }
        if (!validCredentials(ssid, passphrase)) {
            completion(Result.InvalidCredentials)
            return
        }
        val manager = connectivity
        if (manager == null) {
            completion(Result.Failed)
            return
        }
        val specifier = try {
            WifiNetworkSpecifier.Builder()
                .setSsid(ssid)
                .setWpa2Passphrase(passphrase)
                .build()
        } catch (_: IllegalArgumentException) {
            completion(Result.InvalidCredentials)
            return
        }
        val request = NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .setNetworkSpecifier(specifier)
            .build()
        this.completion = completion
        val networkCallback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(available: Network) {
                val capabilities = manager.getNetworkCapabilities(available) ?: return
                acceptIfLocalOnly(available, capabilities)
            }

            override fun onCapabilitiesChanged(
                available: Network,
                capabilities: NetworkCapabilities,
            ) {
                acceptIfLocalOnly(available, capabilities)
            }

            override fun onUnavailable() {
                finish(Result.Unavailable, releaseRequest = true)
            }

            override fun onLost(lost: Network) {
                if (network == lost) {
                    network = null
                    finish(Result.Lost, releaseRequest = true)
                }
            }
        }
        callback = networkCallback
        try {
            manager.requestNetwork(request, networkCallback)
        } catch (_: SecurityException) {
            finish(Result.Failed, releaseRequest = true)
        } catch (_: RuntimeException) {
            finish(Result.Failed, releaseRequest = true)
        }
    }

    private fun acceptIfLocalOnly(
        available: Network,
        capabilities: NetworkCapabilities,
    ) {
        if (network != null ||
            !capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN) ||
            capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
        ) {
            return
        }
        network = available
        completion?.invoke(Result.Ready(available))
    }

    private fun finish(result: Result, releaseRequest: Boolean) {
        val callback = completion
        completion = null
        if (releaseRequest) releaseRequest()
        callback?.invoke(result)
    }

    private fun releaseRequest() {
        val active = callback ?: return
        callback = null
        runCatching { connectivity?.unregisterNetworkCallback(active) }
    }

    override fun close() {
        val hadRequest = callback != null
        network = null
        releaseRequest()
        val callback = completion
        completion = null
        if (hadRequest) callback?.invoke(Result.Cancelled)
    }

    private fun validCredentials(ssid: String, passphrase: String): Boolean =
        ssid.isNotBlank() &&
            ssid.toByteArray(Charsets.UTF_8).size <= 32 &&
            passphrase.length in 8..63
}

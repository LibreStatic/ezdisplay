package com.librestatic.ezdisplay.wifi

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress
import java.util.concurrent.Executor

/**
 * Finds ezdisplay hosts (`_ezdisplay._udp`) on the network with [NsdManager]. Call [start] while a screen that
 * needs the list is visible and [stop] when it is not. [onChange] runs on the main thread with the current list.
 *
 * API 34+ resolves with `registerServiceInfoCallback` (which also follows address changes); older versions resolve
 * one service at a time with `resolveService`.
 */
class HostDiscovery(context: Context, private val onChange: (List<DiscoveredHost>) -> Unit) {
    private val app = context.applicationContext
    private val nsd = app.getSystemService(Context.NSD_SERVICE) as NsdManager
    private val main = Handler(Looper.getMainLooper())
    private val mainExecutor = Executor { main.post(it) }

    private var running = false
    private var lock: WifiManager.MulticastLock? = null
    private val found = LinkedHashMap<String, DiscoveredHost>()
    private val infoCallbacks = HashMap<String, NsdManager.ServiceInfoCallback>()
    private val pendingResolve = ArrayDeque<NsdServiceInfo>()
    private var resolving = false

    /** The hosts found so far. */
    val hosts: List<DiscoveredHost> get() = found.values.toList()

    private val discoveryListener = object : NsdManager.DiscoveryListener {
        override fun onDiscoveryStarted(serviceType: String) {}
        override fun onDiscoveryStopped(serviceType: String) {}
        override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
            Log.w(TAG, "discovery failed: $errorCode")
        }

        override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}
        override fun onServiceFound(info: NsdServiceInfo) {
            main.post { serviceFound(info) }
        }

        override fun onServiceLost(info: NsdServiceInfo) {
            main.post { serviceLost(info.serviceName) }
        }
    }

    /** Starts discovery; a no-op while running. */
    fun start() {
        if (running) return
        running = true
        acquireMulticast()
        try {
            nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, discoveryListener)
        } catch (e: RuntimeException) {
            Log.w(TAG, "cannot start discovery", e)
            running = false
            releaseMulticast()
        }
    }

    /** Stops discovery and forgets what was found. */
    fun stop() {
        if (!running) return
        running = false
        try {
            nsd.stopServiceDiscovery(discoveryListener)
        } catch (e: RuntimeException) {
            Log.w(TAG, "cannot stop discovery", e)
        }
        if (Build.VERSION.SDK_INT >= 34) {
            infoCallbacks.values.forEach { runCatching { nsd.unregisterServiceInfoCallback(it) } }
        }
        infoCallbacks.clear()
        pendingResolve.clear()
        resolving = false
        releaseMulticast()
        if (found.isNotEmpty()) {
            found.clear()
            onChange(emptyList())
        }
    }

    private fun serviceFound(info: NsdServiceInfo) {
        if (!running) return
        if (Build.VERSION.SDK_INT >= 34) {
            if (infoCallbacks.containsKey(info.serviceName)) return
            val cb = object : NsdManager.ServiceInfoCallback {
                override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {
                    Log.w(TAG, "resolve ${info.serviceName} failed: $errorCode")
                    infoCallbacks.remove(info.serviceName)
                }

                override fun onServiceUpdated(updated: NsdServiceInfo) = resolved(updated)
                override fun onServiceLost() = serviceLost(info.serviceName)
                override fun onServiceInfoCallbackUnregistered() {}
            }
            infoCallbacks[info.serviceName] = cb
            nsd.registerServiceInfoCallback(info, mainExecutor, cb)
        } else {
            pendingResolve.add(info)
            resolveNext()
        }
    }

    @Suppress("DEPRECATION")
    private fun resolveNext() {
        if (resolving) return
        val info = pendingResolve.removeFirstOrNull() ?: return
        resolving = true
        nsd.resolveService(
            info,
            object : NsdManager.ResolveListener {
                override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
                    Log.w(TAG, "resolve ${serviceInfo.serviceName} failed: $errorCode")
                    main.post { resolving = false; resolveNext() }
                }

                override fun onServiceResolved(serviceInfo: NsdServiceInfo) {
                    main.post { resolving = false; resolved(serviceInfo); resolveNext() }
                }
            },
        )
    }

    private fun resolved(info: NsdServiceInfo) {
        if (!running) return
        val addresses = if (Build.VERSION.SDK_INT >= 34) info.hostAddresses else listOfNotNull(legacyHost(info))
        val ip = pickAddress(addresses) ?: return
        val txt = info.attributes.mapValues { (_, v) -> v?.toString(Charsets.UTF_8).orEmpty() }
        val host = DiscoveredHost(
            serviceName = info.serviceName,
            name = txt["name"]?.takeIf { it.isNotBlank() } ?: info.serviceName,
            shortFp = txt["fp"].orEmpty(),
            addr = formatAddr(ip, info.port),
        )
        if (found[info.serviceName] != host) {
            found[info.serviceName] = host
            onChange(hosts)
        }
    }

    @Suppress("DEPRECATION")
    private fun legacyHost(info: NsdServiceInfo): InetAddress? = info.host

    private fun serviceLost(serviceName: String) {
        if (Build.VERSION.SDK_INT >= 34) {
            infoCallbacks.remove(serviceName)?.let { runCatching { nsd.unregisterServiceInfoCallback(it) } }
        }
        if (found.remove(serviceName) != null) onChange(hosts)
    }

    private fun acquireMulticast() {
        val wifi = app.getSystemService(Context.WIFI_SERVICE) as? WifiManager ?: return
        lock = runCatching {
            wifi.createMulticastLock("ezdisplay-discovery").apply {
                setReferenceCounted(false)
                acquire()
            }
        }.getOrNull()
    }

    private fun releaseMulticast() {
        runCatching { lock?.takeIf { it.isHeld }?.release() }
        lock = null
    }

    companion object {
        private const val TAG = "ezdisplay"

        /** NsdManager wants the service type without the `.local.` suffix. */
        const val SERVICE_TYPE = "_ezdisplay._udp"

        /** IPv4 first; otherwise a global IPv6 address (link-local ones need a scope id and are skipped). */
        internal fun pickAddress(addresses: List<InetAddress>): InetAddress? =
            addresses.firstOrNull { it is Inet4Address }
                ?: addresses.firstOrNull { it is Inet6Address && !it.isLinkLocalAddress && !it.isLoopbackAddress }

        internal fun formatAddr(ip: InetAddress, port: Int): String {
            val text = ip.hostAddress.orEmpty().substringBefore('%')
            return if (ip is Inet6Address) "[$text]:$port" else "$text:$port"
        }
    }
}

package com.librestatic.ezdisplay.wifi

import android.content.Context
import com.librestatic.ezdisplay.NativeBridge
import java.io.File

/** The native Wi-Fi operations the settings need. Pairing blocks: call it off the main thread. */
interface WifiBackend {
    fun hosts(): List<WifiHost>
    fun identity(): String
    fun forget(fingerprint: String): Boolean

    /** [pinOrUri] is the typed PIN (with [addr]) or a full `ezd://pair?...` link (which carries its own address). */
    fun pair(addr: String, pinOrUri: String, deviceName: String): PairResult

    /** Nothing is paired and nothing can be paired. */
    object None : WifiBackend {
        override fun hosts() = emptyList<WifiHost>()
        override fun identity() = ""
        override fun forget(fingerprint: String) = false
        override fun pair(addr: String, pinOrUri: String, deviceName: String) =
            PairResult(null, "other", "Wi-Fi is not available")
    }
}

/** [WifiBackend] over the JNI bridge; identity and paired hosts live in `filesDir/wifi`. */
class NativeWifiBackend(context: Context) : WifiBackend {
    private val dir: String = stateDir(context).absolutePath

    override fun hosts() = parseHostsJson(NativeBridge.wifiHosts(dir))
    override fun identity(): String = NativeBridge.wifiIdentity(dir)
    override fun forget(fingerprint: String) = NativeBridge.wifiForget(dir, fingerprint)
    override fun pair(addr: String, pinOrUri: String, deviceName: String) =
        parsePairResultJson(NativeBridge.wifiPair(addr, pinOrUri, deviceName, dir))

    companion object {
        /** The private directory with this device's identity and the paired hosts. */
        fun stateDir(context: Context): File = File(context.filesDir, "wifi")
    }
}

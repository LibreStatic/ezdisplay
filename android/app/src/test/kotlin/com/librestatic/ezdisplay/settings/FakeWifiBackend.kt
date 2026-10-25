package com.librestatic.ezdisplay.settings

import com.librestatic.ezdisplay.wifi.PairResult
import com.librestatic.ezdisplay.wifi.WifiBackend
import com.librestatic.ezdisplay.wifi.WifiHost
import com.librestatic.ezdisplay.wifi.shortFingerprint

/** In-memory [WifiBackend]: pairing succeeds with [pin] and answers [error] otherwise. */
class FakeWifiBackend(
    initial: List<WifiHost> = emptyList(),
    private val pin: String = "123456",
    private var error: String = "wrong_pin",
) : WifiBackend {
    val paired = initial.toMutableList()
    val calls = mutableListOf<Triple<String, String, String>>()

    override fun hosts(): List<WifiHost> = paired.toList()
    override fun identity() = "abcd-0123-4567-89ef"
    override fun forget(fingerprint: String) = paired.removeAll { it.fingerprint == fingerprint }

    override fun pair(addr: String, pinOrUri: String, deviceName: String): PairResult {
        calls += Triple(addr, pinOrUri, deviceName)
        if (!pinOrUri.startsWith("ezd://") && pinOrUri != pin) return PairResult(null, error, "refused")
        val fp = "ab".repeat(32)
        val host = WifiHost("Studio PC", fp, shortFingerprint(fp), addr)
        paired.removeAll { it.fingerprint == fp }
        paired += host
        return PairResult(host)
    }
}

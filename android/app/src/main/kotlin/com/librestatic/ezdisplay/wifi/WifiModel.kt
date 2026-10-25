package com.librestatic.ezdisplay.wifi

import org.json.JSONArray
import org.json.JSONObject
import java.net.URLDecoder

/** Port hosts listen on for Wi-Fi when an address is typed without one. */
const val DEFAULT_PORT = 27183

/** Length of the pairing PIN the host shows. */
const val PIN_LENGTH = 6

/** A host this device paired with (`wifiHosts` JSON). [addr] is the address it was last reached at. */
data class WifiHost(val name: String, val fingerprint: String, val shortFp: String, val addr: String? = null)

/** A host found on the network by mDNS: [addr] is `ip:port`, [shortFp] the `fp` TXT record. */
data class DiscoveredHost(val serviceName: String, val name: String, val shortFp: String, val addr: String)

/** Contents of the QR code the host shows: `ezd://pair?a=<ip:port>&fp=<64 hex>&pin=<PIN>&n=<name>`. */
data class PairLink(val addr: String, val fingerprint: String, val pin: String, val name: String) {
    val shortFp: String get() = shortFingerprint(fingerprint)

    /** The link again, as the native pairing takes it. */
    fun toUri(): String = "ezd://pair?a=$addr&fp=$fingerprint&pin=$pin&n=${percentEncode(name)}"
}

/** Percent-encodes everything but unreserved characters (the host decodes `%XX` only, not `+`). */
internal fun percentEncode(s: String): String = buildString {
    for (b in s.toByteArray(Charsets.UTF_8)) {
        val c = b.toInt().toChar()
        if (c.isLetterOrDigit() && b >= 0 || c in "-_.~") append(c) else append("%%%02X".format(b.toInt() and 0xFF))
    }
}

/** Result of `wifiPair`: the paired [host], or an [error] code (see `wifiErrorText`) with a technical [message]. */
data class PairResult(val host: WifiHost?, val error: String? = null, val message: String = "") {
    val ok: Boolean get() = host != null
}

/** True for exactly [PIN_LENGTH] ASCII digits. */
fun isValidPin(pin: String): Boolean = pin.length == PIN_LENGTH && pin.all { it in '0'..'9' }

/** `3f2a-91c0-77de-0b14`: the first 16 hex digits of [fingerprint] in four groups (what the host shows). */
fun shortFingerprint(fingerprint: String): String =
    fingerprint.lowercase().take(16).chunked(4).joinToString("-")

/** True when the full [fingerprint] starts with the dash-separated [shortFp] (from mDNS or the host's screen). */
fun fingerprintMatches(fingerprint: String, shortFp: String): Boolean {
    val short = shortFp.replace("-", "").lowercase()
    return short.length >= 8 && fingerprint.lowercase().startsWith(short)
}

private const val PAIR_URI_PREFIX = "ezd://pair?"
private val HEX64 = Regex("[0-9a-fA-F]{64}")

/** Parses a pairing link; null unless address, a 64-digit fingerprint and a valid PIN are present. */
fun parsePairUri(uri: String?): PairLink? {
    val trimmed = uri?.trim() ?: return null
    if (!trimmed.startsWith(PAIR_URI_PREFIX)) return null
    val query = trimmed.removePrefix(PAIR_URI_PREFIX)
    var addr: String? = null
    var fp: String? = null
    var pin: String? = null
    var name = ""
    for (kv in query.split('&')) {
        val (k, v) = kv.split('=', limit = 2).takeIf { it.size == 2 } ?: return null
        val value = try {
            URLDecoder.decode(v.replace("+", "%2B"), "UTF-8")
        } catch (e: IllegalArgumentException) {
            return null
        }
        when (k) {
            "a" -> addr = value
            "fp" -> fp = value
            "pin" -> pin = value
            "n" -> name = value.filter { !it.isISOControl() }.trim().take(64)
        }
    }
    if (addr.isNullOrBlank() || parseManualAddress(addr) == null) return null
    if (fp == null || !HEX64.matches(fp) || pin == null || !isValidPin(pin)) return null
    return PairLink(parseManualAddress(addr)!!, fp.lowercase(), pin, name)
}

private val IPV4 = Regex("""(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})""")

/**
 * Normalizes a typed host address to `ip:port`: an IPv4 address or an IPv6 address (in brackets when a port
 * follows), with [DEFAULT_PORT] when there is none. Null when it is not an address.
 */
fun parseManualAddress(text: String): String? {
    val s = text.trim()
    if (s.isEmpty()) return null
    if (s.startsWith("[")) {
        val end = s.indexOf(']')
        if (end < 0) return null
        val ip = s.substring(1, end)
        val port = when {
            end == s.lastIndex -> DEFAULT_PORT
            s[end + 1] == ':' -> parsePort(s.substring(end + 2)) ?: return null
            else -> return null
        }
        return if (isIpv6(ip)) "[$ip]:$port" else null
    }
    if (s.count { it == ':' } > 1) return if (isIpv6(s)) "[$s]:$DEFAULT_PORT" else null
    val host = s.substringBefore(':')
    val port = if (':' in s) parsePort(s.substringAfter(':')) ?: return null else DEFAULT_PORT
    val m = IPV4.matchEntire(host) ?: return null
    if (m.groupValues.drop(1).any { it.toInt() > 255 }) return null
    return "$host:$port"
}

private fun parsePort(s: String): Int? = s.toIntOrNull()?.takeIf { s.all(Char::isDigit) && it in 1..65535 }

private fun isIpv6(s: String): Boolean =
    s.contains(':') && s.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' || it == ':' || it == '.' } &&
        s.count { it == ':' } in 2..7

/** Parses the `wifiHosts` JSON array; bad input gives an empty list. */
fun parseHostsJson(json: String): List<WifiHost> = try {
    val a = JSONArray(json)
    (0 until a.length()).map { parseHost(a.getJSONObject(it)) }
} catch (e: Exception) {
    emptyList()
}

private fun parseHost(o: JSONObject) = WifiHost(
    name = o.optString("name"),
    fingerprint = o.getString("fingerprint"),
    shortFp = o.optString("shortFp"),
    addr = o.optString("addr").takeIf { it.isNotEmpty() },
)

/** Parses the `wifiPair` JSON. Anything unreadable is an `other` error. */
fun parsePairResultJson(json: String): PairResult = try {
    val o = JSONObject(json)
    if (o.optBoolean("ok")) PairResult(parseHost(o.getJSONObject("host")))
    else PairResult(null, o.optString("error", "other"), o.optString("message"))
} catch (e: Exception) {
    PairResult(null, "other", json)
}

/** Where to connect over Wi-Fi: the paired [host] at [addr] (`ip:port`). */
data class WifiChoice(val host: WifiHost, val addr: String)

/**
 * The paired host to try over Wi-Fi and its address. Prefers a host that is on the network now (address from
 * discovery), then the one used last ([lastFingerprint]) and its stored address, then any host with a stored
 * address. Null when no paired host has an address.
 */
fun chooseWifiTarget(hosts: List<WifiHost>, discovered: List<DiscoveredHost>, lastFingerprint: String?): WifiChoice? {
    fun live(h: WifiHost) = discovered.firstOrNull { fingerprintMatches(h.fingerprint, it.shortFp) }?.addr
    val ordered = hosts.sortedBy { if (it.fingerprint == lastFingerprint) 0 else 1 }
    ordered.firstNotNullOfOrNull { h -> live(h)?.let { WifiChoice(h, it) } }?.let { return it }
    return ordered.firstNotNullOfOrNull { h -> h.addr?.let { WifiChoice(h, it) } }
}

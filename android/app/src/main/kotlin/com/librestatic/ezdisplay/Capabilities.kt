package com.librestatic.ezdisplay

import android.app.Activity
import android.content.SharedPreferences
import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.os.Build
import android.view.Display
import android.view.InputDevice
import com.librestatic.ezdisplay.settings.SettingsStore
import com.librestatic.ezdisplay.settings.StreamQualities
import com.librestatic.ezdisplay.settings.VideoCodecs
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/** Gathers what the host needs for negotiation and serializes it for native (`ClientCaps`). */
object Capabilities {
    private val CODECS = listOf(
        "h264" to "video/avc",
        "hevc" to "video/hevc",
        "av1" to "video/av01",
    )

    /** Display modes at the panel's native resolution. */
    data class NativeModes(val width: Int, val height: Int, val modes: List<Display.Mode>) {
        /** The mode with the highest refresh rate. */
        val best: Display.Mode get() = modes.maxBy { it.refreshRate }
    }

    @Suppress("DEPRECATION")
    fun display(activity: Activity): Display =
        if (Build.VERSION.SDK_INT >= 30) activity.display!! else activity.windowManager.defaultDisplay

    fun nativeModes(display: Display): NativeModes {
        val all = display.supportedModes.toList().ifEmpty { listOf(display.mode) }
        // Landscape-normalized physical size; the largest one is the native resolution.
        fun Display.Mode.size() = max(physicalWidth, physicalHeight) to min(physicalWidth, physicalHeight)
        val native = all.maxBy { it.size().first.toLong() * it.size().second }.size()
        val modes = all.filter { it.size() == native }
        return NativeModes(native.first, native.second, modes)
    }

    /** True when the device lists a decoder for [codec] (`h264`, `hevc`, `av1`). */
    fun hasDecoder(codec: String): Boolean {
        val mime = CODECS.firstOrNull { it.first == codec }?.second ?: return false
        return MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.any { info ->
            !info.isEncoder && info.supportedTypes.any { it.equals(mime, ignoreCase = true) }
        }
    }

    /**
     * `stream_prefs` from the settings. HEVC without an HEVC decoder is sent as `auto`; unknown or missing values
     * are `auto` / `normal`.
     */
    fun streamPrefs(prefs: SharedPreferences, hevcAvailable: Boolean = hasDecoder("hevc")): JSONObject {
        val codec = prefs.getString(SettingsStore.VIDEO_CODEC, null).takeIf { it in VideoCodecs } ?: "auto"
        val quality = prefs.getString(SettingsStore.STREAM_QUALITY, null).takeIf { it in StreamQualities } ?: "normal"
        return JSONObject()
            .put("codec", if (codec == "hevc" && !hevcAvailable) "auto" else codec)
            .put("quality", quality)
    }

    fun toJson(activity: Activity, chosen: Display.Mode?): String {
        val display = display(activity)
        val native = nativeModes(display)
        val rates = native.modes.map { (it.refreshRate * 1000).roundToInt() }.distinct().sorted()
        val current = ((chosen ?: native.best).refreshRate * 1000).roundToInt()
        val json = JSONObject()
            .put("device_name", "${Build.MANUFACTURER} ${Build.MODEL}".trim())
            .put(
                "display",
                JSONObject()
                    .put("width_px", native.width)
                    .put("height_px", native.height)
                    .put("density_dpi", activity.resources.displayMetrics.densityDpi)
                    .put("refresh_rates_mhz", JSONArray(rates))
                    .put("current_refresh_mhz", current),
            )
            .put("hdr", hdr(display))
            .put("decoders", decoders(native.width, native.height))
            .put("stylus", hasStylus())
            .put("max_touch_points", 10)
            .put("stream_prefs", streamPrefs(InputPrefs.prefs(activity)))
        return json.toString()
    }

    /**
     * [caps] (from [toJson]) with the `audio` switch set. The switch is a user setting that can change between
     * sessions, so it is applied on every connect instead of being cached with the rest.
     */
    fun withAudio(caps: String, enabled: Boolean): String =
        JSONObject(caps).put("audio", enabled).toString()

    /** HDR10 support and luminance of the panel; luminance members are 0 when the platform does not report them. */
    @Suppress("DEPRECATION")
    private fun hdr(display: Display): JSONObject {
        val caps = display.hdrCapabilities
        val types = if (Build.VERSION.SDK_INT >= 34) {
            display.mode.supportedHdrTypes
        } else {
            caps?.supportedHdrTypes ?: IntArray(0)
        }
        fun nits(v: Float, scale: Float = 1f) =
            if (v.isNaN() || v <= 0f) 0 else min(65535f, v * scale).roundToInt()
        return JSONObject()
            .put("display_hdr10", types.contains(Display.HdrCapabilities.HDR_TYPE_HDR10))
            .put("max_nits", nits(caps?.desiredMaxLuminance ?: 0f))
            .put("max_avg_nits", nits(caps?.desiredMaxAverageLuminance ?: 0f))
            .put("min_millinits", nits(caps?.desiredMinLuminance ?: 0f, 1000f))
    }

    /** True when [caps] lists a Main10 HDR10 profile for [mime]. */
    private fun hasHdr10Profile(caps: MediaCodecInfo.CodecCapabilities, mime: String): Boolean {
        val profile = when (mime.lowercase()) {
            "video/hevc" -> MediaCodecInfo.CodecProfileLevel.HEVCProfileMain10HDR10
            "video/av01" -> MediaCodecInfo.CodecProfileLevel.AV1ProfileMain10HDR10
            else -> return false
        }
        return caps.profileLevels.any { it.profile == profile }
    }

    private fun isHardware(info: MediaCodecInfo): Boolean {
        if (Build.VERSION.SDK_INT >= 29) return info.isHardwareAccelerated
        val n = info.name.lowercase()
        return !(n.startsWith("omx.google.") || n.startsWith("c2.android.") || n.contains(".sw."))
    }

    private fun decoders(width: Int, height: Int): JSONArray {
        val out = JSONArray()
        val infos = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
        for (info in infos) {
            if (info.isEncoder) continue
            if (Build.VERSION.SDK_INT >= 29 && info.isAlias) continue
            if (info.name.endsWith(".secure")) continue
            for ((codec, mime) in CODECS) {
                if (info.supportedTypes.none { it.equals(mime, ignoreCase = true) }) continue
                val caps = try {
                    info.getCapabilitiesForType(mime)
                } catch (_: IllegalArgumentException) {
                    continue
                }
                val video = caps.videoCapabilities ?: continue
                val lowLatency = Build.VERSION.SDK_INT >= 30 &&
                    caps.isFeatureSupported(MediaCodecInfo.CodecCapabilities.FEATURE_LowLatency)
                val maxW = video.supportedWidths.upper
                val maxH = video.supportedHeights.upper
                val fps = try {
                    video.getSupportedFrameRatesFor(min(width, maxW), min(height, maxH)).upper.toInt()
                } catch (_: IllegalArgumentException) {
                    video.supportedFrameRates.upper.toInt()
                }
                out.put(
                    JSONObject()
                        .put("codec", codec)
                        .put("name", info.name)
                        .put("hardware", isHardware(info))
                        .put("low_latency", lowLatency)
                        .put("max_width", maxW)
                        .put("max_height", maxH)
                        .put("max_fps", fps)
                        .put("hdr10", hasHdr10Profile(caps, mime)),
                )
            }
        }
        return out
    }

    private fun hasStylus(): Boolean =
        InputDevice.getDeviceIds().any { id ->
            val d = InputDevice.getDevice(id) ?: return@any false
            d.supportsSource(InputDevice.SOURCE_STYLUS)
        }
}

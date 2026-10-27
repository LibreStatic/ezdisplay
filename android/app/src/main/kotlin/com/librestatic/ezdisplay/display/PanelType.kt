package com.librestatic.ezdisplay.display

import android.content.Context
import android.os.Build
import android.view.Display
import androidx.core.content.edit
import com.librestatic.ezdisplay.InputPrefs
import java.io.File

/** What the screen is made of; OLED screens are the ones that can burn in. */
enum class PanelType { Oled, Lcd, Unknown }

/** Device models known to have an OLED screen, and known exceptions. Extend the lists, not the code. */
object KnownPanels {
    /** `Build.MODEL` prefixes of OLED devices (Samsung tablets and phones, Pixel phones). */
    val OLED_MODEL_PREFIXES: List<String> = listOf(
        // Galaxy Tab S (AMOLED generations).
        "SM-T86", "SM-T97", "SM-X71", "SM-X72", "SM-X73", "SM-X80", "SM-X81", "SM-X82", "SM-X90", "SM-X91", "SM-X92", "SM-X93",
        // Galaxy S, Note, Z Fold and Flip.
        "SM-S9", "SM-G9", "SM-N9", "SM-F9", "SM-F7",
        // Pixel phones; the exceptions below are LCD.
        "Pixel ",
    )

    /** Models that start with an OLED prefix but have an LCD. */
    val LCD_MODEL_PREFIXES: List<String> = listOf("Pixel Tablet", "Pixel C")
}

private val OledName = Regex("amoled|oled|s6e\\w+|ea8\\w+|ams\\d+|rm69\\w+|nt37\\w+", RegexOption.IGNORE_CASE)
private val LcdName = Regex("lcd|tft|ips|nt36\\w+|ili\\w+", RegexOption.IGNORE_CASE)

/** Classifies a panel or driver name such as `ss_fhd_s6e3hc3_amoled`; OLED is checked first ("amoled" is not LCD). */
fun classifyPanelName(name: String): PanelType = when {
    OledName.containsMatchIn(name) -> PanelType.Oled
    LcdName.containsMatchIn(name) -> PanelType.Lcd
    else -> PanelType.Unknown
}

/** Classifies a `Build.MODEL` against [KnownPanels]; LCD exceptions win over OLED prefixes. */
fun classifyModel(model: String): PanelType {
    val m = model.trim()
    return when {
        KnownPanels.LCD_MODEL_PREFIXES.any { m.startsWith(it, ignoreCase = true) } -> PanelType.Lcd
        KnownPanels.OLED_MODEL_PREFIXES.any { m.startsWith(it, ignoreCase = true) } -> PanelType.Oled
        else -> PanelType.Unknown
    }
}

/**
 * Best-effort screen type detection, first definitive answer wins: sysfs panel names, vendor system
 * properties, the device model, then the minimum luminance the display reports (OLED goes near zero).
 * The sources are injected so the layers can be tested without Android.
 */
class PanelDetector(
    private val readFile: (String) -> String? = ::readTextOrNull,
    private val listDir: (String) -> List<String> = ::listDirOrEmpty,
    private val readProp: (String) -> String? = ::readSystemProperty,
    private val model: String = Build.MODEL,
) {
    /** [minLuminance] is `Display.HdrCapabilities.desiredMinLuminance`, or null when unavailable. */
    fun detect(minLuminance: Float? = null): PanelType {
        fromSysfs().takeIf { it != PanelType.Unknown }?.let { return it }
        fromProps().takeIf { it != PanelType.Unknown }?.let { return it }
        classifyModel(model).takeIf { it != PanelType.Unknown }?.let { return it }
        if (minLuminance != null && minLuminance > 0f && minLuminance <= OLED_MAX_MIN_LUMINANCE) return PanelType.Oled
        return PanelType.Unknown
    }

    private fun fromSysfs(): PanelType {
        val names = mutableListOf<String>()
        for (path in SYSFS_FILES) runCatching { readFile(path) }.getOrNull()?.let { names += it }
        for (dir in runCatching { listDir("/sys/class/drm") }.getOrDefault(emptyList())) {
            runCatching { readFile("/sys/class/drm/$dir/panel_name") }.getOrNull()?.let { names += it }
        }
        return names.firstNotNullOfOrNull { classifyPanelName(it).takeIf { t -> t != PanelType.Unknown } }
            ?: PanelType.Unknown
    }

    private fun fromProps(): PanelType =
        PROPS.firstNotNullOfOrNull { p ->
            runCatching { readProp(p) }.getOrNull()?.takeIf { it.isNotBlank() }
                ?.let { classifyPanelName(it) }?.takeIf { it != PanelType.Unknown }
        } ?: PanelType.Unknown

    companion object {
        const val OLED_MAX_MIN_LUMINANCE = 0.005f

        val SYSFS_FILES = listOf(
            "/sys/class/graphics/fb0/msm_fb_panel_info",
            "/sys/devices/virtual/graphics/fb0/panel_info",
            "/sys/class/lcd/panel/lcd_type",
        )
        val PROPS = listOf(
            "ro.vendor.display.panel_name",
            "vendor.display.panel_type",
            "persist.vendor.display.panel_name",
        )

        /** The detected type kept in prefs; detection runs once. */
        fun cached(context: Context, display: Display? = null): PanelType {
            val prefs = InputPrefs.prefs(context)
            prefs.getString(InputPrefs.PANEL_TYPE_DETECTED, null)
                ?.let { s -> PanelType.entries.firstOrNull { it.name == s } }
                ?.takeIf { it != PanelType.Unknown }
                ?.let { return it }
            val type = PanelDetector().detect(minLuminance(context, display))
            prefs.edit { putString(InputPrefs.PANEL_TYPE_DETECTED, type.name) }
            return type
        }

        @Suppress("DEPRECATION")
        private fun minLuminance(context: Context, display: Display?): Float? = runCatching {
            val d = display ?: context.getSystemService(android.hardware.display.DisplayManager::class.java)
                .getDisplay(Display.DEFAULT_DISPLAY)
            d?.hdrCapabilities?.desiredMinLuminance
        }.getOrNull()
    }
}

private fun readTextOrNull(path: String): String? = runCatching { File(path).readText().trim() }.getOrNull()

private fun listDirOrEmpty(path: String): List<String> =
    runCatching { File(path).list()?.toList() }.getOrNull() ?: emptyList()

private fun readSystemProperty(key: String): String? = runCatching {
    Class.forName("android.os.SystemProperties").getMethod("get", String::class.java).invoke(null, key) as? String
}.getOrNull()

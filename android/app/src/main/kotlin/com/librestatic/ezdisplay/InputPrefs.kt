package com.librestatic.ezdisplay

import android.content.Context
import android.content.SharedPreferences
import org.json.JSONObject

/** Input settings (SharedPreferences file `input`) and the JSON the native input engine takes. */
object InputPrefs {
    const val FILE = "input"
    const val PROFILE = "profile"
    const val CUSTOM_CLICK = "custom_click"
    const val CUSTOM_DOUBLE_CLICK = "custom_double_click"
    const val CUSTOM_HOLD = "custom_hold"
    const val CUSTOM_HOLD_DRAG_CONTACT = "custom_hold_drag_contact"
    const val CUSTOM_HOLD_DRAG_HOVER = "custom_hold_drag_hover"
    const val ZOOM_FACTOR = "zoom_factor"
    const val DOUBLE_CLICK_MS = "double_click_ms"
    const val PALM_REJECTION = "palm_rejection"
    const val KEYS_CLICK = "keys_click"
    const val KEYS_DOUBLE_CLICK = "keys_double_click"
    const val KEYS_HOLD = "keys_hold"

    const val MOD_CTRL = 1
    const val MOD_SHIFT = 2
    const val MOD_ALT = 4
    const val MOD_META = 8

    const val DOUBLE_CLICK_MIN = 150
    const val DOUBLE_CLICK_MAX = 800
    const val DOUBLE_CLICK_STEP = 10

    val PROFILES = listOf("raw", "desktop", "drawing", "graphics_tablet", "custom")
    /** The stored profile, or "raw" when missing or not one of [PROFILES] (no migration of retired values). */
    fun profile(p: SharedPreferences): String =
        p.getString(PROFILE, "raw")?.takeIf { it in PROFILES } ?: "raw"

    val ZOOM_FACTORS = listOf(1.5f, 2f, 3f)

    /** A key combination: [mods] bit set (CTRL=1, SHIFT=2, ALT=4, META=8) and a USB HID keyboard [usage] (0 = none). */
    data class KeyCombo(val mods: Int, val usage: Int) {
        fun format() = "$mods:$usage"

        /** Human label such as "Ctrl + Shift + Z"; "-" when empty. */
        fun label(context: Context): String {
            val parts = mutableListOf<String>()
            if (mods and MOD_CTRL != 0) parts += "Ctrl"
            if (mods and MOD_SHIFT != 0) parts += "Shift"
            if (mods and MOD_ALT != 0) parts += "Alt"
            if (mods and MOD_META != 0) parts += context.getString(R.string.key_meta)
            if (usage != 0) parts += keyLabel(context, usage)
            return if (parts.isEmpty()) "-" else parts.joinToString(" + ")
        }

        companion object {
            fun parse(s: String?, default: KeyCombo): KeyCombo {
                val parts = s?.split(":") ?: return default
                if (parts.size != 2) return default
                val m = parts[0].toIntOrNull() ?: return default
                val u = parts[1].toIntOrNull() ?: return default
                if (m !in 0..15 || u !in 0..255) return default
                return KeyCombo(m, u)
            }
        }
    }

    val DEFAULT_KEYS_CLICK = KeyCombo(MOD_CTRL, 0x1D)
    val DEFAULT_KEYS_DOUBLE_CLICK = KeyCombo(MOD_CTRL or MOD_SHIFT, 0x1D)
    val DEFAULT_KEYS_HOLD = KeyCombo(MOD_CTRL, 0x2C)

    /** Selectable keys in spinner order (HID usage ids). */
    val KEY_USAGES: List<Int> = buildList {
        add(0)
        add(0x2C)
        addAll(0x04..0x1D) // A-Z
        addAll(0x1E..0x27) // 1-9, 0
        addAll(0x3A..0x45) // F1-F12
        addAll(listOf(0x2B, 0x29, 0x28, 0x2A, 0x4C, 0x50, 0x4F, 0x52, 0x51, 0x2F, 0x30, 0x2D, 0x2E, 0x36, 0x37))
    }

    private val KEY_NAME_RES = mapOf(
        0 to R.string.key_none, 0x2C to R.string.key_space, 0x2B to R.string.key_tab,
        0x29 to R.string.key_esc, 0x28 to R.string.key_enter, 0x2A to R.string.key_backspace,
        0x4C to R.string.key_delete,
    )
    private val KEY_SYMBOLS = mapOf(
        0x50 to "←", 0x4F to "→", 0x52 to "↑", 0x51 to "↓", 0x2F to "[", 0x30 to "]",
        0x2D to "-", 0x2E to "=", 0x36 to ",", 0x37 to ".",
    )

    fun keyLabel(context: Context, usage: Int): String = when (usage) {
        in KEY_NAME_RES -> context.getString(KEY_NAME_RES.getValue(usage))
        in KEY_SYMBOLS -> KEY_SYMBOLS.getValue(usage)
        in 0x04..0x1D -> ('A' + (usage - 0x04)).toString()
        in 0x1E..0x26 -> (usage - 0x1D).toString()
        0x27 -> "0"
        in 0x3A..0x45 -> "F${usage - 0x39}"
        else -> "0x" + usage.toString(16)
    }

    fun keyCombo(context: Context, key: String): KeyCombo = KeyCombo.parse(
        prefs(context).getString(key, null),
        when (key) {
            KEYS_CLICK -> DEFAULT_KEYS_CLICK
            KEYS_DOUBLE_CLICK -> DEFAULT_KEYS_DOUBLE_CLICK
            else -> DEFAULT_KEYS_HOLD
        },
    )

    /** True when the active profile can inject keyboard shortcuts (graphics-tablet profile, or Custom with a "keys" action). */
    fun usesKeys(context: Context): Boolean {
        val p = prefs(context)
        return when (profile(p)) {
            "graphics_tablet" -> true
            "custom" -> p.getString(CUSTOM_CLICK, "raw") == "keys" ||
                p.getString(CUSTOM_DOUBLE_CLICK, "local_zoom") == "keys" ||
                p.getString(CUSTOM_HOLD, "raw") == "keys"
            else -> false
        }
    }

    fun prefs(context: Context): SharedPreferences =
        context.getSharedPreferences(FILE, Context.MODE_PRIVATE)

    /** Engine configuration JSON built from the stored preferences (defaults when unset). */
    fun toJson(context: Context): String {
        val p = prefs(context)
        val custom = JSONObject()
            .put("click", p.getString(CUSTOM_CLICK, "raw"))
            .put("double_click", p.getString(CUSTOM_DOUBLE_CLICK, "local_zoom"))
            .put("hold", p.getString(CUSTOM_HOLD, "raw"))
            .put("hold_drag_contact", p.getString(CUSTOM_HOLD_DRAG_CONTACT, "raw"))
            .put("hold_drag_hover", p.getString(CUSTOM_HOLD_DRAG_HOVER, "raw"))
        fun combo(key: String) = keyCombo(context, key).let {
            JSONObject().put("mods", it.mods).put("key", it.usage)
        }
        val keys = JSONObject()
            .put("click", combo(KEYS_CLICK))
            .put("double_click", combo(KEYS_DOUBLE_CLICK))
            .put("hold", combo(KEYS_HOLD))
        return JSONObject()
            .put("profile", profile(p))
            .put("custom", custom)
            .put("keys", keys)
            .put("zoom_factor", p.getFloat(ZOOM_FACTOR, 2f).toDouble())
            .put("double_click_ms", p.getInt(DOUBLE_CLICK_MS, 300))
            .put("palm_rejection", p.getBoolean(PALM_REJECTION, true))
            .toString()
    }
}

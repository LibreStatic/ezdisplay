package io.github.facum.ezdisplay

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

    const val DOUBLE_CLICK_MIN = 150
    const val DOUBLE_CLICK_MAX = 800
    const val DOUBLE_CLICK_STEP = 10

    val PROFILES = listOf("raw", "desktop", "drawing", "custom")
    val ZOOM_FACTORS = listOf(1.5f, 2f, 3f)

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
        return JSONObject()
            .put("profile", p.getString(PROFILE, "raw"))
            .put("custom", custom)
            .put("zoom_factor", p.getFloat(ZOOM_FACTOR, 2f).toDouble())
            .put("double_click_ms", p.getInt(DOUBLE_CLICK_MS, 300))
            .put("palm_rejection", p.getBoolean(PALM_REJECTION, true))
            .toString()
    }
}

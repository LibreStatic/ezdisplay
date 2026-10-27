package com.librestatic.ezdisplay.screensaver

import android.content.Context
import android.content.SharedPreferences
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.display.PanelDetector
import com.librestatic.ezdisplay.display.PanelType
import kotlin.math.roundToInt

/** The idle screensaver settings. [blankMin] is 0 when the screen never goes fully black. */
data class ScreensaverConfig(
    val enabled: Boolean,
    val idleMin: Int,
    val brightnessPct: Int,
    val blankMin: Int,
    val speed: Int = InputPrefs.SCREENSAVER_SPEED_DEFAULT,
    val motion: ScreensaverMotionMode = ScreensaverMotionMode.Drift,
)

/** Clamps a stored or typed value to the allowed ranges. */
fun normalizeIdleMin(v: Int): Int =
    v.coerceIn(InputPrefs.SCREENSAVER_IDLE_MIN_VALUE, InputPrefs.SCREENSAVER_IDLE_MAX_VALUE)

fun normalizeBrightnessPct(v: Int): Int =
    v.coerceIn(InputPrefs.SCREENSAVER_BRIGHTNESS_MIN_VALUE, InputPrefs.SCREENSAVER_BRIGHTNESS_MAX_VALUE)

fun normalizeSpeed(v: Int): Int =
    v.coerceIn(InputPrefs.SCREENSAVER_SPEED_MIN_VALUE, InputPrefs.SCREENSAVER_SPEED_MAX_VALUE)

/** 0 (never) or 5..120; anything between 1 and 4 is raised to 5. */
fun normalizeBlankMin(v: Int): Int =
    if (v <= 0) 0 else v.coerceIn(InputPrefs.SCREENSAVER_BLANK_MIN_VALUE, InputPrefs.SCREENSAVER_BLANK_MAX_VALUE)

/** Rounds a slider position to a valid blank-after value; positions below the minimum mean never (0). */
fun roundBlankMin(raw: Float): Int {
    val step = InputPrefs.SCREENSAVER_BLANK_STEP
    val v = (raw / step).roundToInt() * step
    return if (v < InputPrefs.SCREENSAVER_BLANK_MIN_VALUE) 0 else v.coerceAtMost(InputPrefs.SCREENSAVER_BLANK_MAX_VALUE)
}

/** Reads and writes the screensaver prefs (file `input`, but never part of the engine JSON). */
object ScreensaverPrefs {
    /** Enabled flag; when the user never chose, on exactly for OLED screens. */
    fun enabled(prefs: SharedPreferences, panel: PanelType): Boolean =
        if (prefs.contains(InputPrefs.SCREENSAVER_ENABLED)) {
            prefs.getBoolean(InputPrefs.SCREENSAVER_ENABLED, false)
        } else {
            panel == PanelType.Oled
        }

    fun read(prefs: SharedPreferences, panel: PanelType): ScreensaverConfig = ScreensaverConfig(
        enabled = enabled(prefs, panel),
        idleMin = normalizeIdleMin(prefs.getInt(InputPrefs.SCREENSAVER_IDLE_MIN, InputPrefs.SCREENSAVER_IDLE_DEFAULT)),
        brightnessPct = normalizeBrightnessPct(
            prefs.getInt(InputPrefs.SCREENSAVER_BRIGHTNESS_PCT, InputPrefs.SCREENSAVER_BRIGHTNESS_DEFAULT),
        ),
        blankMin = normalizeBlankMin(prefs.getInt(InputPrefs.SCREENSAVER_BLANK_MIN, InputPrefs.SCREENSAVER_BLANK_DEFAULT)),
        speed = normalizeSpeed(prefs.getInt(InputPrefs.SCREENSAVER_SPEED, InputPrefs.SCREENSAVER_SPEED_DEFAULT)),
        motion = ScreensaverMotionMode.fromKey(prefs.getString(InputPrefs.SCREENSAVER_MOTION, null)),
    )

    /** Reads the config for the device, detecting (and caching) the panel type when needed. */
    fun read(context: Context): ScreensaverConfig =
        read(InputPrefs.prefs(context), PanelDetector.cached(context))
}

package com.librestatic.ezdisplay.display

import android.content.Context
import android.content.SharedPreferences
import com.librestatic.ezdisplay.InputPrefs

/** Pixel shift settings: [radiusPx] is the largest distance from the centre on each axis. */
data class PixelShiftConfig(
    val enabled: Boolean = false,
    val radiusPx: Int = InputPrefs.DEFAULT_PIXEL_SHIFT_PX,
)

/**
 * Burn-in protection: moves the picture by one pixel every [STEP_INTERVAL_MS], only while the user is not
 * touching the screen (or while the screensaver hides the picture). Pure logic; time is passed on every call.
 *
 * The offsets follow a serpentine walk over the (2r+1) x (2r+1) grid, forward and then backward, so each step
 * changes exactly one axis by exactly 1 px and an end-to-end pass visits every position.
 */
class PixelShift(config: PixelShiftConfig = PixelShiftConfig(), nowMs: Long = 0L) {
    private var config = config
    private var pathIndex = 0
    private var direction = 1
    private var lastStepMs = nowMs
    private var lastPointerMs = nowMs
    private var touchDown = false
    private var hovering = false

    var offsetX: Int = 0
        private set
    var offsetY: Int = 0
        private set

    init {
        restart(nowMs)
    }

    /** Returns true when the offset changed. */
    fun setConfig(config: PixelShiftConfig, nowMs: Long): Boolean {
        if (config == this.config) return false
        val restart = !config.enabled || config.radiusPx != this.config.radiusPx || !this.config.enabled
        this.config = config
        if (!restart) return false
        return restart(nowMs)
    }

    /** Call for every touch event (finger, pen or mouse button); [down] is true until the last pointer lifts. */
    fun onTouch(down: Boolean, nowMs: Long) {
        touchDown = down
        lastPointerMs = nowMs
    }

    /**
     * Call for every hover or other generic pointer event; [hovering] is true while a pen stays in range. A mouse
     * passes false, because it sends no exit when it stops: only its pause counts. Never changes the touch state.
     */
    fun onHover(hovering: Boolean, nowMs: Long) {
        this.hovering = hovering
        lastPointerMs = nowMs
    }

    /** Takes one pending step when due; returns true when the offset changed. */
    fun tick(nowMs: Long, covered: Boolean): Boolean {
        if (!config.enabled) return false
        if (nowMs - lastStepMs < STEP_INTERVAL_MS) return false
        val quiet = !touchDown && !hovering && nowMs - lastPointerMs >= QUIET_MS
        if (!covered && !quiet) return false
        lastStepMs = nowMs
        val last = side() * side() - 1
        if (last == 0) return false
        if (pathIndex + direction !in 0..last) direction = -direction
        pathIndex += direction
        return apply()
    }

    private fun side() = 2 * config.radiusPx + 1

    /** Back to the centre; returns true when the offset changed. */
    private fun restart(nowMs: Long): Boolean {
        direction = 1
        lastStepMs = nowMs
        pathIndex = centreIndex()
        return apply()
    }

    private fun centreIndex(): Int {
        val r = config.radiusPx
        // The middle row is r; its centre cell is at column r whichever way the row is walked.
        return r * side() + r
    }

    private fun apply(): Boolean {
        val oldX = offsetX
        val oldY = offsetY
        if (!config.enabled) {
            offsetX = 0
            offsetY = 0
        } else {
            val n = side()
            val row = pathIndex / n
            val col = pathIndex % n
            val x = if (row % 2 == 0) col else n - 1 - col
            offsetX = x - config.radiusPx
            offsetY = row - config.radiusPx
        }
        return offsetX != oldX || offsetY != oldY
    }

    companion object {
        const val STEP_INTERVAL_MS = 60_000L
        const val QUIET_MS = 3_000L
    }
}

/** Reads the pixel shift prefs (file `input`, but never part of the engine JSON). */
object PixelShiftPrefs {
    /** Clamps a stored or typed value to the allowed range. */
    fun normalizePx(v: Int): Int = v.coerceIn(InputPrefs.MIN_PIXEL_SHIFT_PX, InputPrefs.MAX_PIXEL_SHIFT_PX)

    /** Enabled flag; when the user never chose, on exactly for OLED screens. */
    fun enabled(prefs: SharedPreferences, panel: PanelType): Boolean =
        if (prefs.contains(InputPrefs.PIXEL_SHIFT_ENABLED)) {
            prefs.getBoolean(InputPrefs.PIXEL_SHIFT_ENABLED, false)
        } else {
            panel == PanelType.Oled
        }

    fun radiusPx(prefs: SharedPreferences): Int =
        normalizePx(prefs.getInt(InputPrefs.PIXEL_SHIFT_PX, InputPrefs.DEFAULT_PIXEL_SHIFT_PX))

    fun read(prefs: SharedPreferences, panel: PanelType): PixelShiftConfig =
        PixelShiftConfig(enabled = enabled(prefs, panel), radiusPx = radiusPx(prefs))

    /** Reads the config for the device, detecting (and caching) the panel type when needed. */
    fun read(context: Context): PixelShiftConfig =
        read(InputPrefs.prefs(context), PanelDetector.cached(context))
}

package com.librestatic.ezdisplay.screensaver

import android.animation.ValueAnimator
import android.content.Context
import android.provider.Settings
import android.view.Window
import android.view.WindowManager
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

/**
 * Android side of the screensaver: holds the [state] the overlay shows and animates the window brightness
 * (dim in Saver, almost off in Blank, back to the system level on wake). The caller owns the [IdleTracker].
 */
class ScreensaverController(private val context: Context, private val window: Window) {
    var state by mutableStateOf(IdleState.Active)
        private set

    private var animator: ValueAnimator? = null

    /** Moves to [new]; a no-op when unchanged. [config] gives the dim level. */
    fun apply(new: IdleState, config: ScreensaverConfig) {
        if (new == state) return
        state = new
        when (new) {
            IdleState.Active -> {
                animator?.cancel()
                setBrightness(WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_NONE)
            }
            IdleState.Saver -> animateBrightness((config.brightnessPct / 100f).coerceAtLeast(MIN_BRIGHTNESS), SAVER_DIM_MS)
            IdleState.Blank -> animateBrightness(MIN_BRIGHTNESS, BLANK_DIM_MS)
        }
    }

    /** Stops animating and gives the brightness back. Call from onPause / onDestroy. */
    fun release() {
        animator?.cancel()
        state = IdleState.Active
        setBrightness(WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_NONE)
    }

    private fun animateBrightness(to: Float, durationMs: Long) {
        animator?.cancel()
        val from = currentBrightness()
        val a = ValueAnimator.ofFloat(from, to).apply {
            duration = durationMs
            addUpdateListener { setBrightness(it.animatedValue as Float) }
        }
        animator = a
        a.start() // an animator duration scale of 0 makes this jump straight to the end
    }

    private fun currentBrightness(): Float {
        val override = window.attributes.screenBrightness
        if (override >= 0f) return override
        val system = try {
            Settings.System.getInt(context.contentResolver, Settings.System.SCREEN_BRIGHTNESS, -1)
        } catch (e: Exception) {
            -1
        }
        return if (system >= 0) (system / 255f).coerceIn(MIN_BRIGHTNESS, 1f) else DEFAULT_BRIGHTNESS
    }

    private fun setBrightness(value: Float) {
        val lp = window.attributes
        lp.screenBrightness = value
        window.attributes = lp
    }

    companion object {
        const val MIN_BRIGHTNESS = 0.01f
        const val SAVER_DIM_MS = 1500L
        const val BLANK_DIM_MS = 600L
        private const val DEFAULT_BRIGHTNESS = 0.5f
    }
}

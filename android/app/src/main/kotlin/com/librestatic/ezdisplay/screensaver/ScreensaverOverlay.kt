package com.librestatic.ezdisplay.screensaver

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.tween
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.ui.expressive.currentReducedMotion

/** Time for the black layer to fade in when the saver starts. */
const val SAVER_FADE_IN_MS = 2000

/** Time for the black layer to fade out on wake. */
const val SAVER_FADE_OUT_MS = 300

/**
 * The screensaver layer: nothing while [IdleState.Active], a black layer with moving shapes in
 * [IdleState.Saver] and the shapes fading out in [IdleState.Blank]. It takes no touches. Fades are skipped with
 * reduced motion.
 */
@Composable
fun ScreensaverOverlay(
    state: IdleState,
    modifier: Modifier = Modifier,
    speed: Int = InputPrefs.SCREENSAVER_SPEED_DEFAULT,
    motion: ScreensaverMotionMode = ScreensaverMotionMode.Drift,
) {
    val reduced = currentReducedMotion()
    val target = if (state == IdleState.Active) 0f else 1f
    val alpha by animateFloatAsState(
        targetValue = target,
        animationSpec = when {
            reduced -> snap()
            target == 1f -> tween(SAVER_FADE_IN_MS)
            else -> tween(SAVER_FADE_OUT_MS)
        },
        label = "screensaverAlpha",
    )
    if (alpha > 0f || target > 0f) {
        ScreensaverContent(
            modifier.fillMaxSize().graphicsLayer { this.alpha = alpha },
            blank = state == IdleState.Blank,
            speed = speed,
            motion = motion,
        )
    }
}

package com.librestatic.ezdisplay.ui.expressive

import androidx.compose.animation.ContentTransform
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.core.FastOutLinearInEasing
import androidx.compose.animation.core.LinearOutSlowInEasing
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.VisibilityThreshold
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.LayoutDirection

/**
 * Material shared-axis X transitions between pages: the old page slides a quarter width out and fades
 * quickly, the new one slides in after it, both on the same spring, so a step reads as one move.
 * [forward] is the direction of travel through the flow (next vs back); [rtl] mirrors it so pages still
 * advance in reading order. With [reducedMotion] pages only crossfade.
 *
 * With `AnimatedContent`: `transitionSpec = { transitions.contentTransform(targetState > initialState) }`.
 */
class PageTransitions(private val reducedMotion: Boolean, private val rtl: Boolean) {
    private val slide = spring(
        dampingRatio = 0.9f,
        stiffness = Spring.StiffnessMediumLow,
        visibilityThreshold = IntOffset.VisibilityThreshold,
    )

    /** +1 when the next page comes from the end side of the screen. */
    private fun sign(forward: Boolean) = (if (forward) 1 else -1) * (if (rtl) -1 else 1)

    fun enter(forward: Boolean): EnterTransition =
        if (reducedMotion) {
            fadeIn(tween(durationMillis = 150))
        } else {
            slideInHorizontally(slide) { width -> sign(forward) * width / 4 } +
                fadeIn(tween(durationMillis = 220, delayMillis = 90, easing = LinearOutSlowInEasing))
        }

    fun exit(forward: Boolean): ExitTransition =
        if (reducedMotion) {
            fadeOut(tween(durationMillis = 150))
        } else {
            slideOutHorizontally(slide) { width -> -sign(forward) * width / 4 } +
                fadeOut(tween(durationMillis = 90, easing = FastOutLinearInEasing))
        }

    fun contentTransform(forward: Boolean): ContentTransform = enter(forward) togetherWith exit(forward)
}

/** [PageTransitions] for the current layout direction and reduced-motion setting. */
@Composable
fun rememberPageTransitions(): PageTransitions {
    val reduced = currentReducedMotion()
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    return remember(reduced, rtl) { PageTransitions(reduced, rtl) }
}

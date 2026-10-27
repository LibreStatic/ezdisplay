package com.librestatic.ezdisplay.screensaver

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.IntSize
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.ui.expressive.currentReducedMotion
import kotlin.random.Random
import kotlinx.coroutines.delay

/** White, light grey and mid grey: the only colours on the screensaver, so no hue ages the panel unevenly. */
private val ScreensaverPalette = listOf(Color(0xFFFFFFFF), Color(0xFFC8C8C8), Color(0xFF8A8A8A))

/** Dim overall: the base opacity (0.12 to 0.24 by depth) is multiplied by this. */
private const val ScreensaverAlpha = 2.2f

/** Time the shapes take to fade out when blank. */
private const val FadeOutMs = 800

/** Seconds between moves with reduced motion: rare steps instead of per-frame movement. */
private const val ReducedStepMs = 4000L

/** Longest single frame step, so a stalled frame does not make the shapes jump. */
private const val MaxFrameSeconds = 0.1f

/** Dp per second for one speed level. */
const val SCREENSAVER_DP_PER_LEVEL = 10f

/**
 * What the screensaver shows: a black background with a few dim monochrome shapes that travel across the screen
 * ([motion]: cross it and leave, or bounce off the edges) at [speed] (level 1..10) so no pixel stays lit.
 * With [blank] the shapes fade out, leaving pure black (fully off pixels on an OLED screen) and nothing is
 * drawn or computed any more. Reduced motion keeps the movement but only in rare steps.
 */
@Composable
fun ScreensaverContent(
    modifier: Modifier = Modifier,
    blank: Boolean = false,
    count: Int = 5,
    seed: Int? = null,
    speed: Int = InputPrefs.SCREENSAVER_SPEED_DEFAULT,
    motion: ScreensaverMotionMode = ScreensaverMotionMode.Drift,
) {
    val reducedMotion = currentReducedMotion()
    val currentSeed = seed ?: rememberSaveable { Random.nextInt() }
    val density = LocalDensity.current.density
    var size by remember { mutableStateOf(IntSize.Zero) }
    val field = remember(currentSeed, count, density, motion) {
        ScreensaverMotion(motion, count, currentSeed, density)
    }
    // Sizes a field recreated by a mode change (the size callback below does not fire again for it).
    SideEffect {
        field.resize(size.width.toFloat(), size.height.toFloat())
        field.speedDp = normalizeSpeed(speed) * SCREENSAVER_DP_PER_LEVEL
    }

    val fade = remember { Animatable(1f, visibilityThreshold = 0.0005f) }
    var active by remember { mutableStateOf(true) }
    LaunchedEffect(blank, reducedMotion) {
        if (!blank) active = true
        val target = if (blank) 0f else 1f
        if (reducedMotion) fade.snapTo(target) else fade.animateTo(target, tween(FadeOutMs, easing = FastOutSlowInEasing))
        if (blank) active = false
    }

    // Read in the draw phase only, so a frame redraws the canvas without recomposing anything.
    var tick by remember { mutableLongStateOf(0L) }
    LaunchedEffect(field, active, reducedMotion) {
        if (!active) return@LaunchedEffect
        if (reducedMotion) {
            while (true) {
                delay(ReducedStepMs)
                field.step(ReducedStepMs / 1000f)
                tick++
            }
        }
        var last = withFrameNanos { it }
        while (true) {
            withFrameNanos { now ->
                field.step(((now - last) / 1_000_000_000f).coerceAtMost(MaxFrameSeconds))
                last = now
                tick++
            }
        }
    }

    // Parsing and scaling a silhouette is costly and a shape's size never changes, so build each once.
    val glyphs = remember(field) { arrayOfNulls<Path>(count) }

    Box(modifier.background(Color.Black)) {
        Canvas(
            Modifier.fillMaxSize().clearAndSetSemantics {}.onSizeChanged {
                size = it
                // Not left to the SideEffect: nothing in composition reads the size, so no recomposition follows.
                field.resize(it.width.toFloat(), it.height.toFloat())
            },
        ) {
            tick // subscribe to frames
            val visibility = fade.value
            if (!active && visibility <= 0f) return@Canvas
            field.bodies.forEachIndexed { i, b ->
                val r = b.radius
                if (b.x + r <= 0f || b.x - r >= this.size.width || b.y + r <= 0f || b.y - r >= this.size.height) return@forEachIndexed
                val glyph = glyphs[i] ?: screensaverGlyph(b.shapeIndex, r * 2f).also { glyphs[i] = it }
                val alpha = ((0.12f + 0.12f * b.depth) * ScreensaverAlpha).coerceIn(0f, 1f) * visibility
                translate(b.x - r, b.y - r) {
                    rotate(b.angle, Offset(r, r)) {
                        drawPath(glyph, color = ScreensaverPalette[b.colorIndex % ScreensaverPalette.size], alpha = alpha)
                    }
                }
            }
        }
    }
}

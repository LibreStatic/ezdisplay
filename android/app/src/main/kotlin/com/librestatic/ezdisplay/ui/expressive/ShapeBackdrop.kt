package com.librestatic.ezdisplay.ui.expressive

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Outline
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.drawOutline
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.graphics.shapes.RoundedPolygon
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.sin
import kotlin.random.Random
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.drop

/** One floating shape. Positions are fractions of the backdrop; [depth] drives parallax and size. */
private class FloatingShape(
    val shape: Shape,
    val role: Int,
    val x: Float,
    val y: Float,
    val sizeDp: Float,
    val depth: Float,
    val spin: Float,
    val phase: Float,
    val wobble: Float,
) {
    // Building a polygon outline is costly, and the side never changes, so build it once.
    private var cached: Outline? = null

    fun outline(side: Float, direction: LayoutDirection, density: Density): Outline =
        cached ?: shape.createOutline(Size(side, side), direction, density).also { cached = it }
}

/** How far, in backdrop widths, one unit of `progress` scrolls the shapes along their rail. */
private const val MoveLength = 0.55f

/** Horizontal span the rail wraps around, so shapes leaving one edge come back on the other. */
private const val WrapSpan = 1.5f

/** Degrees a shape rolls per unit of `progress`, so travel reads as rolling rather than sliding. */
private const val MoveRoll = 90f

/** Largest distance, in dp, the whole field wanders from its place when pixel shift is on. */
private const val WanderDp = 28f

/** Seconds one full slow wander cycle takes on each axis (different, so the path never repeats soon). */
private const val WanderPeriodX = 97f
private const val WanderPeriodY = 71f

/** How often, in milliseconds, the field is redrawn with reduced motion and pixel shift on. */
private const val ReducedWanderStepMs = 4000L

/** The Material expressive shapes [ShapeBackdrop] draws unless told otherwise. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
val DefaultBackdropShapes: List<RoundedPolygon> = listOf(
    MaterialShapes.Cookie9Sided, MaterialShapes.Clover4Leaf, MaterialShapes.SoftBurst,
    MaterialShapes.Flower, MaterialShapes.Pill, MaterialShapes.Arch, MaterialShapes.Sunny,
    MaterialShapes.Gem, MaterialShapes.Cookie6Sided, MaterialShapes.Puffy,
)

/**
 * Decorative field of Material expressive shapes. Shapes drift and turn slowly, roll along a rail as
 * [progress] advances (one unit per page move, with parallax) and give a light outward burst whenever
 * [burstKey] changes (not for its first value).
 *
 * - [palette]: colours the shapes cycle through (any alpha; the field applies its own).
 * - [shapes]: polygons to pick from; [count] shapes are drawn, or a number by width when null.
 * - [alpha]: multiplier of the base opacity (0.12 to 0.24 by depth), clamped to opaque.
 * - [speed]: 1 is the lively default; about 0.3 suits a screensaver.
 * - [pixelShift]: slowly moves the whole field around (Lissajous path, a few dozen dp) so that no
 *   pixel stays lit on an OLED screen. It keeps running with reduced motion, in rare small steps.
 * - [dispersed]: shapes fly out from the centre and fade, then [onDispersed] runs; clearing it
 *   brings them back.
 * - [seed]: layout seed; random per composition (kept across recreation) when null.
 *
 * Everything is drawn on one Canvas; the clock is read in the draw phase, so nothing recomposes per
 * frame. With system animations off the field is static and the exit finishes immediately.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ShapeBackdrop(
    modifier: Modifier = Modifier,
    palette: List<Color>,
    shapes: List<RoundedPolygon> = DefaultBackdropShapes,
    count: Int? = null,
    alpha: Float = 1f,
    speed: Float = 1f,
    progress: () -> Float = { 0f },
    burstKey: Any? = null,
    dispersed: Boolean = false,
    onDispersed: () -> Unit = {},
    pixelShift: Boolean = false,
    seed: Int? = null,
) {
    require(palette.isNotEmpty()) { "palette must not be empty" }
    require(shapes.isNotEmpty()) { "shapes must not be empty" }
    val reducedMotion = currentReducedMotion()
    val currentSeed = seed ?: rememberSaveable { Random.nextInt() }
    val finish by rememberUpdatedState(onDispersed)
    // Tracked as state so the long-lived collectors below always read the latest values.
    val currentProgress by rememberUpdatedState(progress)
    val currentSpeed by rememberUpdatedState(speed)
    val currentKey by rememberUpdatedState(burstKey)

    // Thresholds far below the defaults: the rail is multiplied by several widths, so the default
    // 0.01 cut-off would snap the last few dozen pixels in one frame when a move settled.
    val rail = remember { Animatable(0f, visibilityThreshold = 0.0001f) }
    LaunchedEffect(reducedMotion) {
        snapshotFlow { currentProgress() }.collectLatest { target ->
            if (reducedMotion) {
                rail.snapTo(0f)
            } else {
                // A new target mid-move keeps the current velocity, so quick moves chain smoothly.
                rail.animateTo(target, spring(dampingRatio = 0.9f, stiffness = Spring.StiffnessLow, visibilityThreshold = 0.0001f))
            }
        }
    }
    // Drift clock (scaled by speed) and wander clock (real seconds).
    var time by remember { mutableFloatStateOf(0f) }
    var wanderTime by remember { mutableFloatStateOf(0f) }
    LaunchedEffect(reducedMotion, pixelShift) {
        if (reducedMotion) {
            // Static field; with pixel shift it still creeps, redrawn rarely.
            if (!pixelShift) return@LaunchedEffect
            val start = System.nanoTime()
            while (true) {
                wanderTime = (System.nanoTime() - start) / 1_000_000_000f
                delay(ReducedWanderStepMs)
            }
        }
        var last = withFrameNanos { it }
        val start = last
        while (true) {
            withFrameNanos { now ->
                val dt = (now - last) / 1_000_000_000f
                last = now
                time += dt * currentSpeed
                wanderTime = (now - start) / 1_000_000_000f
            }
        }
    }
    val burst = remember { Animatable(0f, visibilityThreshold = 0.0005f) }
    LaunchedEffect(reducedMotion) {
        if (reducedMotion) return@LaunchedEffect
        // No burst for the key the field opens with, only for later changes.
        snapshotFlow { currentKey }.distinctUntilChanged().drop(1).collectLatest {
            // An impulse from rest, not a jump: the shapes swell out and bounce back continuously.
            burst.animateTo(
                0f,
                spring(dampingRatio = 0.45f, stiffness = Spring.StiffnessLow, visibilityThreshold = 0.0005f),
                initialVelocity = 4f,
            )
        }
    }
    val exit = remember { Animatable(0f, visibilityThreshold = 0.0005f) }
    LaunchedEffect(dispersed, reducedMotion) {
        val target = if (dispersed) 1f else 0f
        if (reducedMotion) exit.snapTo(target) else exit.animateTo(target, tween(durationMillis = 800, easing = FastOutSlowInEasing))
        if (dispersed) finish()
    }

    BoxWithConstraints(modifier.fillMaxSize().clearAndSetSemantics {}) {
        val total = count ?: when {
            maxWidth >= 840.dp -> 16
            maxWidth >= 600.dp -> 12
            else -> 6
        }
        val density = LocalDensity.current
        val outlines = shapes.map { it.toShape() }
        // Keyed on density too: each shape caches an outline built at its pixel size.
        val field = remember(currentSeed, total, outlines, density) {
            val random = Random(currentSeed)
            List(total) {
                val depth = 0.4f + random.nextFloat() * 0.6f
                FloatingShape(
                    shape = outlines[random.nextInt(outlines.size)],
                    role = random.nextInt(palette.size.coerceAtLeast(1)),
                    x = random.nextFloat() * WrapSpan,
                    y = random.nextFloat(),
                    sizeDp = 40f + 120f * depth,
                    depth = depth,
                    spin = (6f + random.nextFloat() * 14f) * if (random.nextBoolean()) 1f else -1f,
                    phase = random.nextFloat() * 2f * PI.toFloat(),
                    wobble = 8f + random.nextFloat() * 16f,
                )
            }
        }

        Canvas(Modifier.fillMaxSize()) {
            val center = Offset(size.width / 2f, size.height / 2f)
            val diagonal = hypot(size.width, size.height)
            val e = exit.value
            val b = burst.value
            val shiftX: Float
            val shiftY: Float
            if (pixelShift) {
                val amp = WanderDp.dp.toPx()
                val tau = 2f * PI.toFloat()
                shiftX = sin(wanderTime * tau / WanderPeriodX) * amp
                shiftY = sin(wanderTime * tau / WanderPeriodY + 1.3f) * amp
            } else {
                shiftX = 0f
                shiftY = 0f
            }
            field.forEach { s ->
                val side = s.sizeDp.dp.toPx()
                // Rail travel with parallax: nearer (bigger) shapes move further.
                val along = ((s.x - rail.value * MoveLength * s.depth) % WrapSpan + WrapSpan) % WrapSpan
                val wobble = s.wobble.dp.toPx()
                var cx = (along - (WrapSpan - 1f) / 2f) * size.width + sin(time * 0.35f + s.phase) * wobble + shiftX
                var cy = s.y * size.height + cos(time * 0.27f + s.phase * 1.3f) * wobble + shiftY
                val dx = cx - center.x
                val dy = cy - center.y
                val distance = hypot(dx, dy).coerceAtLeast(1f)
                // Bursts and the exit both push outwards along the line from the centre.
                val push = b * 40.dp.toPx() * s.depth + e * diagonal * (0.6f + s.depth)
                cx += dx / distance * push
                cy += dy / distance * push
                val scale = 1f + 0.15f * b + 0.4f * e
                val roll = rail.value * MoveRoll * s.depth * if (s.spin > 0f) 1f else -1f
                val angle = s.phase * 57.3f + time * s.spin + roll + e * s.spin * 12f
                val shapeAlpha = ((0.12f + 0.12f * s.depth) * alpha).coerceIn(0f, 1f) * (1f - e)
                if (shapeAlpha <= 0f) return@forEach
                translate(cx - side / 2f, cy - side / 2f) {
                    val pivot = Offset(side / 2f, side / 2f)
                    scale(scale, pivot) {
                        rotate(angle, pivot) {
                            drawOutline(
                                s.outline(side, layoutDirection, this),
                                color = palette[s.role % palette.size],
                                alpha = shapeAlpha,
                            )
                        }
                    }
                }
            }
        }
    }
}

package com.librestatic.ezdisplay.onboarding

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.graphics.shapes.Morph
import com.librestatic.ezdisplay.R
import com.librestatic.ezdisplay.settings.Symbol
import com.librestatic.ezdisplay.ui.expressive.MorphShape
import com.librestatic.ezdisplay.ui.expressive.PolygonShape
import com.librestatic.ezdisplay.ui.expressive.currentReducedMotion
import kotlin.math.PI
import kotlin.math.sin

/**
 * A value looping 0..1 every [durationMs]. With reduced motion it stays at [rest], a frame that still reads
 * as the idea of the animation.
 */
@Composable
internal fun rememberLoop(durationMs: Int, rest: Float = 0.6f, mode: RepeatMode = RepeatMode.Restart): State<Float> {
    if (currentReducedMotion()) return remember(rest) { mutableFloatStateOf(rest) }
    return rememberInfiniteTransition(label = "loop").animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(durationMs, easing = LinearEasing), mode),
        label = "loop",
    )
}

/** Stacked, slowly morphing and turning shapes behind a stylus. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun WelcomeHero(size: Dp, modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    val morph = rememberLoop(durationMs = 5000, rest = 0.4f, mode = RepeatMode.Reverse)
    val spin = rememberLoop(durationMs = 24000, rest = 0f)
    val outer = remember { Morph(MaterialShapes.Cookie9Sided, MaterialShapes.Sunny) }
    val middle = remember { Morph(MaterialShapes.Gem, MaterialShapes.Clover4Leaf) }
    val inner = remember { Morph(MaterialShapes.Circle, MaterialShapes.Flower) }
    Box(modifier.size(size).clearAndSetSemantics { }, contentAlignment = Alignment.Center) {
        Box(
            Modifier.size(size)
                .graphicsLayer { rotationZ = spin.value * 360f }
                .clip(MorphShape(outer) { morph.value })
                .background(colors.primaryContainer),
        )
        Box(
            Modifier.size(size * 0.7f)
                .graphicsLayer { rotationZ = -spin.value * 360f * 1.5f }
                .clip(MorphShape(middle) { 1f - morph.value })
                .background(colors.tertiaryContainer),
        )
        Box(
            Modifier.size(size * 0.42f)
                .graphicsLayer { rotationZ = spin.value * 360f * 2f }
                .clip(MorphShape(inner) { morph.value })
                .background(colors.secondaryContainer),
        )
        Symbol(R.drawable.ic_stylus, tint = colors.onSecondaryContainer, size = size * 0.2f)
    }
}

/** A cookie that pops in on a spring and then turns slowly. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun DoneHero(size: Dp, modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    val reduced = currentReducedMotion()
    val pop = remember { Animatable(if (reduced) 1f else 0f) }
    LaunchedEffect(reduced) {
        if (!reduced) pop.animateTo(1f, spring(dampingRatio = Spring.DampingRatioMediumBouncy, stiffness = Spring.StiffnessLow))
    }
    val spin = rememberLoop(durationMs = 18000, rest = 0f)
    val shape = remember { PolygonShape(MaterialShapes.Cookie12Sided) }
    Box(modifier.size(size).clearAndSetSemantics { }, contentAlignment = Alignment.Center) {
        Box(
            Modifier.size(size)
                .graphicsLayer {
                    scaleX = pop.value
                    scaleY = pop.value
                    rotationZ = spin.value * 360f
                }
                .clip(shape)
                .background(colors.primary),
        )
        Symbol(
            R.drawable.ic_check, tint = colors.onPrimary, size = size * 0.4f,
            modifier = Modifier.graphicsLayer {
                scaleX = pop.value
                scaleY = pop.value
            },
        )
    }
}

private fun DrawScope.wave(a: Float, b: Float, color: Color, width: Float) {
    if (b <= a) return
    val steps = 24
    val path = Path()
    var started = false
    for (i in 0..steps) {
        val f = i / steps.toFloat()
        if (f < a || f > b) continue
        val x = size.width * (0.1f + 0.8f * f)
        val y = size.height * (0.55f - 0.22f * sin(f * 2f * PI.toFloat()))
        if (started) path.lineTo(x, y) else path.moveTo(x, y)
        started = true
    }
    if (started) drawPath(path, color, style = Stroke(width, cap = StrokeCap.Round))
}

/** A small looping picture of what the pen button does in [profile]. */
@Composable
internal fun ProfileHint(profile: String, modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    val t = rememberLoop(durationMs = 2400, rest = 0.7f)
    val stroke = colors.primary
    val accent = colors.tertiary
    val muted = colors.outline
    Canvas(modifier.size(56.dp).clearAndSetSemantics { }) {
        val w = size.width
        val h = size.height
        val sw = 3.dp.toPx()
        val v = t.value
        val pulse = (0.5f - 0.5f * kotlin.math.cos(v * 2f * PI.toFloat()))
        when (profile) {
            // The pen moves and its button is passed on to the PC as it is.
            "raw" -> {
                wave(0f, v, stroke, sw)
                drawCircle(accent.copy(alpha = 0.3f + 0.7f * pulse), radius = 5.dp.toPx(), center = Offset(w * 0.85f, h * 0.15f))
            }
            // A mouse: the right half lights up on a click.
            "desktop" -> {
                val body = Size(w * 0.5f, h * 0.8f)
                val topLeft = Offset((w - body.width) / 2f, h * 0.1f)
                drawRoundRect(muted, topLeft, body, CornerRadius(body.width / 2f), style = Stroke(sw))
                drawRoundRect(
                    accent.copy(alpha = 0.2f + 0.7f * pulse),
                    Offset(w / 2f, topLeft.y), Size(body.width / 2f, body.height * 0.4f), CornerRadius(8.dp.toPx()),
                )
                drawLine(muted, Offset(w / 2f, topLeft.y), Offset(w / 2f, topLeft.y + body.height * 0.4f), sw)
            }
            // A stroke is drawn, then the eraser takes it away again.
            "drawing" -> {
                if (v < 0.5f) wave(0f, v * 2f, stroke, sw) else wave((v - 0.5f) * 2f, 1f, stroke, sw)
                drawCircle(accent, radius = 3.dp.toPx(), center = Offset(w * 0.1f + w * 0.8f * (if (v < 0.5f) v * 2f else (v - 0.5f) * 2f), h * 0.88f))
            }
            // A key cap that goes down while the button is held.
            "graphics_tablet" -> {
                val cap = Size(w * 0.36f, h * 0.36f)
                val down = 3.dp.toPx() * pulse
                drawRoundRect(muted, Offset(w * 0.1f, h * 0.32f), cap, CornerRadius(6.dp.toPx()), style = Stroke(sw))
                drawRoundRect(
                    accent.copy(alpha = 0.35f + 0.65f * pulse),
                    Offset(w * 0.54f, h * 0.32f + down), cap, CornerRadius(6.dp.toPx()),
                )
            }
        }
    }
}

/** A tablet outline and a finger dragging a panel down from its top edge. */
@Composable
internal fun SwipeDownHint(modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    val t = rememberLoop(durationMs = 2600, rest = 0.7f)
    val outline = colors.outline
    val panel = colors.primaryContainer
    val finger = colors.primary
    Canvas(modifier.size(width = 140.dp, height = 96.dp).clearAndSetSemantics { }) {
        val sw = 3.dp.toPx()
        val frame = Size(size.width - sw, size.height - sw)
        val topLeft = Offset(sw / 2f, sw / 2f)
        val corner = CornerRadius(14.dp.toPx())
        // Move for the first 70 %, then rest while the panel is out and fade.
        val move = FastOutSlowInEasing.transform((t.value / 0.7f).coerceIn(0f, 1f))
        val fade = if (t.value < 0.85f) 1f else (1f - (t.value - 0.85f) / 0.15f)
        val drop = frame.height * 0.45f * move
        drawRoundRect(panel.copy(alpha = fade), topLeft, Size(frame.width, drop.coerceAtLeast(0.1f)), corner)
        drawRoundRect(outline, topLeft, frame, corner, style = Stroke(sw))
        val fy = topLeft.y + drop
        drawCircle(finger.copy(alpha = 0.25f * fade), radius = 16.dp.toPx(), center = Offset(size.width / 2f, fy))
        drawCircle(finger.copy(alpha = fade), radius = 8.dp.toPx(), center = Offset(size.width / 2f, fy))
    }
}

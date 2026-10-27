package com.librestatic.ezdisplay.ui.expressive

import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.size
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.graphics.shapes.Morph
import androidx.graphics.shapes.RoundedPolygon

/** The shapes [StepShape] walks through by default, one per step (wrapping when there are more steps). */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
val DefaultStepShapes: List<RoundedPolygon> = listOf(
    MaterialShapes.Circle,
    MaterialShapes.Gem,
    MaterialShapes.Clover4Leaf,
    MaterialShapes.Flower,
    MaterialShapes.Cookie9Sided,
    MaterialShapes.Pill,
    MaterialShapes.Sunny,
)

/**
 * Step indicator: a Material shape that morphs from the previous step's shape to [stepIndex]'s with a
 * spatial spring while its colour moves from the primary container towards the tertiary container as
 * the flow advances ([stepIndex] of [stepCount]). With reduced motion the morph becomes a crossfade.
 * [content] (an icon or a number) is drawn centred on the shape.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun StepShape(
    stepIndex: Int,
    stepCount: Int,
    modifier: Modifier = Modifier,
    shapes: List<RoundedPolygon> = DefaultStepShapes,
    size: Dp = 44.dp,
    content: @Composable BoxScope.() -> Unit = {},
) {
    require(shapes.isNotEmpty()) { "shapes must not be empty" }
    val colors = MaterialTheme.colorScheme
    val start = colors.primaryContainer
    val end = colors.tertiaryContainer
    fun polygon(i: Int) = shapes[i.mod(shapes.size)]
    fun color(i: Int): Color =
        if (stepCount <= 1) start else lerp(start, end, (i.toFloat() / (stepCount - 1)).coerceIn(0f, 1f))

    Box(modifier.size(size).clearAndSetSemantics { }, contentAlignment = Alignment.Center) {
        if (currentReducedMotion()) {
            Crossfade(targetState = stepIndex, modifier = Modifier.size(size), label = "stepShape") { target ->
                Box(Modifier.size(size).clip(PolygonShape(polygon(target))).background(color(target)))
            }
        } else {
            var from by remember { mutableIntStateOf(stepIndex) }
            var to by remember { mutableIntStateOf(stepIndex) }
            val progress = remember { Animatable(1f) }
            val spec = MaterialTheme.motionScheme.slowSpatialSpec<Float>()
            LaunchedEffect(stepIndex) {
                if (to != stepIndex) {
                    from = to
                    to = stepIndex
                    progress.snapTo(0f)
                    progress.animateTo(1f, spec)
                }
            }
            val morph = remember(from, to, shapes) { Morph(polygon(from), polygon(to)) }
            Box(
                Modifier.size(size)
                    .clip(MorphShape(morph) { progress.value })
                    .background(lerp(color(from), color(to), progress.value.coerceIn(0f, 1f))),
            )
        }
        content()
    }
}

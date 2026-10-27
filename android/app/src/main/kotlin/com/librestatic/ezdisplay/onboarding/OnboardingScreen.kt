package com.librestatic.ezdisplay.onboarding

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.librestatic.ezdisplay.R
import com.librestatic.ezdisplay.settings.PairDialog
import com.librestatic.ezdisplay.settings.SettingsActions
import com.librestatic.ezdisplay.settings.SettingsState
import com.librestatic.ezdisplay.settings.SettingsStore
import com.librestatic.ezdisplay.settings.Symbol
import com.librestatic.ezdisplay.ui.expressive.ShapeBackdrop
import com.librestatic.ezdisplay.ui.expressive.StepShape
import com.librestatic.ezdisplay.ui.expressive.currentReducedMotion
import com.librestatic.ezdisplay.ui.expressive.rememberPageTransitions
import kotlinx.coroutines.delay

/** The pages of the getting-started guide, in order. */
enum class OnboardingStep { Welcome, HostInstall, Connection, PenProfile, Display, Done }

/** Width of the page column on phones and narrow windows. */
private val PageMaxWidth = 560.dp

/** From this width on, the hero sits beside the content instead of above it. */
private val WideBreakpoint = 840.dp

/** Below this height (landscape phones) heroes get smaller so the copy fits. */
private val ShortWindowHeight = 480.dp

/** How long a skip waits for the shapes to fly out before leaving anyway. */
private const val ExitFallbackMillis = 1400L

/** The guide bound to the settings store, so choices are saved at once. [onFinish] runs on Done and on Skip. */
@Composable
fun OnboardingScreen(store: SettingsStore, onFinish: () -> Unit, modifier: Modifier = Modifier) {
    OnboardingContent(store.state.value, store, onFinish, modifier)
}

/**
 * The getting-started guide: one page per [OnboardingStep] with shared-axis transitions, a progress bar
 * and Skip on top, Back and Next below, and a drifting shape field behind. Stateless apart from the
 * current step, so tests and previews can drive it with a fake [SettingsActions].
 */
@Composable
fun OnboardingContent(
    state: SettingsState,
    actions: SettingsActions,
    onFinish: () -> Unit,
    modifier: Modifier = Modifier,
    initialStep: OnboardingStep = OnboardingStep.Welcome,
) {
    val colors = MaterialTheme.colorScheme
    val reduced = currentReducedMotion()
    val steps = OnboardingStep.entries
    var stepIndex by rememberSaveable { mutableIntStateOf(initialStep.ordinal) }
    val step = steps[stepIndex]
    val last = step == steps.last()
    var finishing by remember { mutableStateOf(false) }
    var completed by remember { mutableStateOf(false) }
    val currentFinish by rememberUpdatedState(onFinish)

    fun complete() {
        if (completed) return
        completed = true
        currentFinish()
    }

    /** Skip scatters the shapes first (the backdrop calls back when they are gone), then leaves. */
    fun skip() {
        if (finishing) return
        if (reduced) complete() else finishing = true
    }
    LaunchedEffect(finishing) {
        if (finishing) {
            delay(ExitFallbackMillis)
            complete()
        }
    }
    fun back() {
        if (!finishing && stepIndex > 0) stepIndex--
    }
    fun next() {
        if (finishing) return
        if (last) complete() else stepIndex++
    }
    BackHandler(enabled = stepIndex > 0 && !finishing, onBack = ::back)

    val transitions = rememberPageTransitions()
    val stepLabel = stringResource(R.string.onboarding_step_of, stepIndex + 1, steps.size)
    val contentAlpha by animateFloatAsState(if (finishing) 0f else 1f, tween(300), label = "onboarding-exit")

    Surface(modifier.fillMaxSize().testTag("onboarding_root"), color = colors.surface, contentColor = colors.onSurface) {
        ShapeBackdrop(
            Modifier.fillMaxSize(),
            palette = listOf(colors.primaryContainer, colors.secondaryContainer, colors.tertiaryContainer),
            alpha = 1.6f,
            progress = { stepIndex.toFloat() },
            burstKey = step,
            dispersed = last || finishing,
            onDispersed = { if (finishing) complete() },
        )
        BoxWithConstraints(Modifier.fillMaxSize()) {
            val wide = maxWidth >= WideBreakpoint
            val short = maxHeight < ShortWindowHeight
            val barWidth = if (wide) Modifier.fillMaxWidth() else Modifier.widthIn(max = PageMaxWidth).fillMaxWidth()
            Column(
                Modifier.fillMaxSize()
                    .graphicsLayer { alpha = contentAlpha }
                    .windowInsetsPadding(WindowInsets.safeDrawing),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                TopBar(step, stepLabel, onSkip = if (last) null else ::skip, modifier = barWidth)
                AnimatedContent(
                    targetState = step,
                    modifier = Modifier.weight(1f).fillMaxWidth().semantics { paneTitle = stepLabel },
                    contentKey = { it.name },
                    transitionSpec = { transitions.contentTransform(targetState.ordinal > initialState.ordinal) },
                    label = "onboarding-step",
                ) { current ->
                    StepPage(current, state, actions, wide = wide, short = short)
                }
                BottomBar(
                    showBack = stepIndex > 0,
                    primaryLabel = stringResource(
                        when (step) {
                            OnboardingStep.Welcome -> R.string.onboarding_get_started
                            OnboardingStep.Done -> R.string.onboarding_connect
                            else -> R.string.onboarding_next
                        },
                    ),
                    onBack = ::back,
                    onPrimary = ::next,
                    modifier = barWidth,
                )
            }
        }
    }
    state.pairing?.let { PairDialog(it, actions) }
}

@Composable
private fun TopBar(step: OnboardingStep, stepLabel: String, onSkip: (() -> Unit)?, modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    Row(
        modifier.heightIn(min = 56.dp).padding(start = 24.dp, end = 8.dp, top = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Row(
            Modifier.weight(1f).semantics(mergeDescendants = true) { contentDescription = stepLabel }.testTag("onboarding_progress"),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            OnboardingStep.entries.forEach { entry ->
                val width by animateFloatAsState(if (entry == step) 28f else 12f, label = "progress-width")
                Box(
                    Modifier.height(6.dp).width(width.dp).clip(CircleShape)
                        .background(if (entry.ordinal <= step.ordinal) colors.primary else colors.surfaceContainerHighest),
                )
            }
        }
        if (onSkip != null) {
            TextButton(onClick = onSkip, modifier = Modifier.heightIn(min = 48.dp).testTag("onboarding_skip")) {
                Text(stringResource(R.string.onboarding_skip), style = MaterialTheme.typography.labelLarge)
            }
        }
    }
}

@Composable
private fun BottomBar(
    showBack: Boolean,
    primaryLabel: String,
    onBack: () -> Unit,
    onPrimary: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier.padding(start = 8.dp, end = 24.dp, top = 12.dp, bottom = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (showBack) {
            TextButton(onClick = onBack, modifier = Modifier.heightIn(min = 48.dp).testTag("onboarding_back")) {
                Text(stringResource(R.string.onboarding_back))
            }
        }
        Spacer(Modifier.weight(1f))
        Button(
            onClick = onPrimary,
            modifier = Modifier.heightIn(min = 56.dp).widthIn(min = 140.dp).testTag("onboarding_next"),
        ) { Text(primaryLabel, style = MaterialTheme.typography.titleMedium) }
    }
}

private class StepText(val title: Int, val body: Int)

private fun stepText(step: OnboardingStep) = when (step) {
    OnboardingStep.Welcome -> StepText(R.string.onboarding_welcome_title, R.string.onboarding_welcome_body)
    OnboardingStep.HostInstall -> StepText(R.string.onboarding_host_title, R.string.onboarding_host_body)
    OnboardingStep.Connection -> StepText(R.string.onboarding_connection_title, R.string.onboarding_connection_body)
    OnboardingStep.PenProfile -> StepText(R.string.onboarding_pen_title, R.string.onboarding_pen_body)
    OnboardingStep.Display -> StepText(R.string.onboarding_display_title, R.string.onboarding_display_body)
    OnboardingStep.Done -> StepText(R.string.onboarding_done_title, R.string.onboarding_done_body)
}

private fun stepIcon(step: OnboardingStep) = when (step) {
    OnboardingStep.HostInstall -> R.drawable.ic_computer
    OnboardingStep.Connection -> R.drawable.ic_link
    OnboardingStep.PenProfile -> R.drawable.ic_stylus
    OnboardingStep.Display -> R.drawable.ic_bedtime
    else -> R.drawable.ic_check
}

@Composable
private fun Hero(step: OnboardingStep, size: Dp) {
    when (step) {
        OnboardingStep.Welcome -> WelcomeHero(size)
        OnboardingStep.Done -> DoneHero(size)
        else -> StepShape(
            stepIndex = step.ordinal,
            stepCount = OnboardingStep.entries.size,
            size = size,
        ) { Symbol(stepIcon(step), tint = MaterialTheme.colorScheme.onPrimaryContainer, size = size * 0.45f) }
    }
}

/** The extra content of a step (below the copy on phones, in its own pane on wide windows), or null. */
private fun stepExtra(step: OnboardingStep, state: SettingsState, actions: SettingsActions): (@Composable ColumnScope.() -> Unit)? =
    when (step) {
        OnboardingStep.HostInstall -> { { HostInstallContent() } }
        OnboardingStep.Connection -> { { ConnectionStepContent(state, actions) } }
        OnboardingStep.PenProfile -> { { PenProfileContent(state, actions) } }
        OnboardingStep.Display -> { { DisplayStepContent(state, actions) } }
        else -> null
    }

@Composable
private fun StepPage(step: OnboardingStep, state: SettingsState, actions: SettingsActions, wide: Boolean, short: Boolean) {
    val text = stepText(step)
    val extra = stepExtra(step, state, actions)
    val big = step == OnboardingStep.Welcome || step == OnboardingStep.Done
    val tag = Modifier.testTag("onboarding_step_${step.name}")
    if (wide) {
        Row(tag.fillMaxSize().padding(horizontal = 32.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(32.dp)) {
            Column(
                Modifier.weight(1f).fillMaxSize().verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(20.dp, Alignment.CenterVertically),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Hero(step, if (big) 240.dp else 160.dp)
                Copy(text, centered = true)
            }
            if (extra != null) {
                Surface(
                    Modifier.weight(1.1f).fillMaxSize().padding(vertical = 8.dp),
                    shape = RoundedCornerShape(32.dp),
                    color = MaterialTheme.colorScheme.surfaceContainer.copy(alpha = 0.94f),
                ) {
                    Column(
                        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(24.dp),
                        verticalArrangement = Arrangement.spacedBy(16.dp),
                    ) { extra() }
                }
            }
        }
    } else {
        Box(tag.fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            Column(
                Modifier.widthIn(max = PageMaxWidth).fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp, vertical = 8.dp),
                verticalArrangement = Arrangement.spacedBy(if (short) 12.dp else 20.dp, if (extra == null) Alignment.CenterVertically else Alignment.Top),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Hero(step, if (short) 72.dp else if (big) 200.dp else 96.dp)
                Copy(text, centered = extra == null)
                if (extra != null) extra()
            }
        }
    }
}

@Composable
private fun Copy(text: StepText, centered: Boolean) {
    val align = if (centered) TextAlign.Center else TextAlign.Start
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            stringResource(text.title),
            Modifier.fillMaxWidth(),
            style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.Bold, textAlign = align,
        )
        Text(
            stringResource(text.body),
            Modifier.fillMaxWidth(),
            style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = align,
        )
    }
}

package com.librestatic.ezdisplay.session

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.ExperimentalAnimationApi
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.displayCutoutPadding
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import com.librestatic.ezdisplay.R
import com.librestatic.ezdisplay.ui.expressive.currentReducedMotion
import kotlinx.coroutines.delay

/** How long the pill stays up without being used. */
const val SESSION_PILL_TIMEOUT_MS = 4000L

/**
 * The control pill that drops from the top edge while a session streams: Input settings and Disconnect.
 * While [visible] a transparent layer behind it takes the taps outside (they dismiss the pill and are not
 * forwarded to the host); when hidden nothing here takes touches. Hides itself after [SESSION_PILL_TIMEOUT_MS].
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class, ExperimentalAnimationApi::class)
@Composable
fun SessionPill(
    visible: Boolean,
    onSettings: () -> Unit,
    onDisconnect: () -> Unit,
    onDismiss: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val reduced = currentReducedMotion()
    val dismiss by rememberUpdatedState(onDismiss)
    LaunchedEffect(visible) {
        if (visible) {
            delay(SESSION_PILL_TIMEOUT_MS)
            dismiss()
        }
    }
    val spatial = MaterialTheme.motionScheme.defaultSpatialSpec<IntOffset>()
    val effects = MaterialTheme.motionScheme.defaultEffectsSpec<Float>()
    Box(modifier.fillMaxSize()) {
        if (visible) {
            Box(Modifier.fillMaxSize().pointerInput(Unit) { detectTapGestures { dismiss() } })
        }
        AnimatedVisibility(
            visible = visible,
            modifier = Modifier.align(Alignment.TopCenter).displayCutoutPadding().padding(top = 12.dp),
            enter = if (reduced) fadeIn(snapSpecFloat) else slideInVertically(spatial) { -it * 2 } + fadeIn(effects),
            exit = if (reduced) fadeOut(snapSpecFloat) else slideOutVertically { -it * 2 } + fadeOut(effects),
        ) {
            Surface(
                shape = MaterialTheme.shapes.extraLarge,
                color = MaterialTheme.colorScheme.surfaceContainerHigh,
                contentColor = MaterialTheme.colorScheme.onSurface,
                shadowElevation = 6.dp,
            ) {
                Row(
                    Modifier.padding(horizontal = 12.dp, vertical = 10.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    FilledTonalButton(onClick = onSettings) {
                        Icon(painterResource(R.drawable.ic_settings), null, Modifier.size(ButtonDefaults.IconSize))
                        Text(stringResource(R.string.settings_button), Modifier.padding(start = ButtonDefaults.IconSpacing))
                    }
                    Button(
                        onClick = onDisconnect,
                        colors = ButtonDefaults.buttonColors(
                            containerColor = MaterialTheme.colorScheme.errorContainer,
                            contentColor = MaterialTheme.colorScheme.onErrorContainer,
                        ),
                    ) {
                        Icon(painterResource(R.drawable.ic_close), null, Modifier.size(ButtonDefaults.IconSize))
                        Text(stringResource(R.string.session_disconnect), Modifier.padding(start = ButtonDefaults.IconSpacing))
                    }
                }
            }
        }
    }
}

private val snapSpecFloat = androidx.compose.animation.core.snap<Float>()

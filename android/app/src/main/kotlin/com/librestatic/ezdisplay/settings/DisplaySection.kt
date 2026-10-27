package com.librestatic.ezdisplay.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.R
import com.librestatic.ezdisplay.display.PanelType
import com.librestatic.ezdisplay.screensaver.ScreensaverContent
import com.librestatic.ezdisplay.screensaver.ScreensaverMotionMode
import com.librestatic.ezdisplay.screensaver.roundBlankMin

private val DisplayRowMinHeight = 72.dp
private val DisplayRowPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 16.dp, vertical = 12.dp)

@Composable
private fun panelName(type: PanelType): String = stringResource(
    when (type) {
        PanelType.Oled -> R.string.panel_oled
        PanelType.Lcd -> R.string.panel_lcd
        PanelType.Unknown -> R.string.panel_unknown
    },
)

/** Screensaver switch, its settings, a live preview with a preview button, and the replay row. */
@Composable
fun DisplayContent(state: SettingsState, actions: SettingsActions, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxWidth()) {
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            ScreensaverSwitchRow(state, actions, segmentShape(0, 7))
            IdleSlider(state, actions, segmentShape(1, 7))
            BrightnessSlider(state, actions, segmentShape(2, 7))
            BlankSlider(state, actions, segmentShape(3, 7))
            SpeedSlider(state, actions, segmentShape(4, 7))
            MotionRow(state, actions, segmentShape(5, 7))
            Surface(shape = segmentShape(6, 7), color = MaterialTheme.colorScheme.surfaceBright, modifier = Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Box(Modifier.fillMaxWidth().height(140.dp).clip(RoundedCornerShape(20.dp)).testTag("screensaver_preview_box")) {
                        ScreensaverContent(
                            Modifier.fillMaxSize(), count = 3,
                            speed = state.screensaverSpeed, motion = state.screensaverMotion,
                        )
                    }
                    PreviewButton(actions)
                }
            }
        }
        ReplayRow(actions, Modifier.padding(top = 12.dp))
    }
}

/** Compact entry for the Wide page, which must not scroll: "Screensaver · 5 minutes" or "Screensaver off". */
@Composable
fun DisplaySheetButton(state: SettingsState, onClick: () -> Unit, modifier: Modifier = Modifier) {
    FilledTonalButton(onClick = onClick, modifier = modifier.testTag("screensaver_button")) {
        Symbol(R.drawable.ic_bedtime, size = 20.dp)
        Text(
            if (state.screensaverEnabled) {
                stringResource(R.string.screensaver_title) + " · " + displaySummary(state)
            } else {
                displaySummary(state)
            },
            Modifier.padding(start = 8.dp),
        )
    }
}

/** The full Display section in a bottom sheet. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DisplaySheet(state: SettingsState, actions: SettingsActions, onDismiss: () -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss, modifier = Modifier.testTag("display_sheet")) {
        Column(Modifier.verticalScroll(rememberScrollState()).padding(horizontal = 24.dp).padding(bottom = 24.dp)) {
            Text(
                stringResource(R.string.section_display), Modifier.padding(bottom = 12.dp),
                style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold,
            )
            DisplayContent(state, actions)
        }
    }
}

@Composable
private fun PreviewButton(actions: SettingsActions) {
    FilledTonalButton(onClick = actions::previewScreensaver, modifier = Modifier.testTag("screensaver_preview")) {
        Symbol(R.drawable.ic_play_arrow, size = 20.dp)
        Text(stringResource(R.string.screensaver_preview_button), Modifier.padding(start = 8.dp))
    }
}

@Composable
private fun ScreensaverSwitchRow(state: SettingsState, actions: SettingsActions, shape: androidx.compose.ui.graphics.Shape) {
    val colors = MaterialTheme.colorScheme
    val on = state.screensaverEnabled
    Surface(
        checked = on,
        onCheckedChange = actions::setScreensaverEnabled,
        shape = shape,
        color = colors.surfaceBright,
        modifier = Modifier.fillMaxWidth().semantics { role = Role.Switch }.testTag("screensaver_row"),
    ) {
        Row(
            Modifier.heightIn(min = DisplayRowMinHeight).padding(DisplayRowPadding),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Symbol(R.drawable.ic_bedtime, tint = colors.secondary)
            Column(Modifier.weight(1f)) {
                Text(
                    stringResource(R.string.screensaver_title),
                    style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
                )
                Text(
                    stringResource(R.string.screensaver_detected, panelName(state.panelType)),
                    style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant,
                    modifier = Modifier.testTag("panel_type_text"),
                )
            }
            Switch(
                checked = on,
                onCheckedChange = null,
                thumbContent = if (on) {
                    { Symbol(R.drawable.ic_check, size = 16.dp) }
                } else {
                    null
                },
            )
        }
    }
}

@Composable
private fun IdleSlider(
    state: SettingsState,
    actions: SettingsActions,
    shape: androidx.compose.ui.graphics.Shape,
) {
    val min = InputPrefs.SCREENSAVER_IDLE_MIN_VALUE
    val max = InputPrefs.SCREENSAVER_IDLE_MAX_VALUE
    ValueSliderRow(
        icon = R.drawable.ic_timer,
        title = stringResource(R.string.screensaver_idle_title),
        value = state.screensaverIdleMin.toFloat(),
        valueText = pluralStringResource(R.plurals.settings_minutes, state.screensaverIdleMin, state.screensaverIdleMin),
        range = min.toFloat()..max.toFloat(),
        steps = max - min - 1,
        minText = pluralStringResource(R.plurals.settings_minutes, min, min),
        maxText = pluralStringResource(R.plurals.settings_minutes, max, max),
        enabled = state.screensaverEnabled,
        tag = "screensaver_idle_slider",
        shape = shape,
        onChange = { actions.setScreensaverIdleMin(roundIdleMin(it)) },
    )
}

@Composable
private fun BrightnessSlider(
    state: SettingsState,
    actions: SettingsActions,
    shape: androidx.compose.ui.graphics.Shape,
) {
    val min = InputPrefs.SCREENSAVER_BRIGHTNESS_MIN_VALUE
    val max = InputPrefs.SCREENSAVER_BRIGHTNESS_MAX_VALUE
    ValueSliderRow(
        icon = R.drawable.ic_bedtime,
        title = stringResource(R.string.screensaver_brightness_title),
        value = state.screensaverBrightnessPct.toFloat(),
        valueText = stringResource(R.string.settings_percent, state.screensaverBrightnessPct),
        range = min.toFloat()..max.toFloat(),
        steps = max - min - 1,
        minText = stringResource(R.string.settings_percent, min),
        maxText = stringResource(R.string.settings_percent, max),
        enabled = state.screensaverEnabled,
        tag = "screensaver_brightness_slider",
        shape = shape,
        onChange = { actions.setScreensaverBrightnessPct(roundBrightnessPct(it)) },
    )
}

@Composable
private fun BlankSlider(
    state: SettingsState,
    actions: SettingsActions,
    shape: androidx.compose.ui.graphics.Shape,
) {
    val max = InputPrefs.SCREENSAVER_BLANK_MAX_VALUE
    ValueSliderRow(
        icon = R.drawable.ic_bedtime,
        title = stringResource(R.string.screensaver_blank_title),
        value = state.screensaverBlankMin.toFloat(),
        valueText = blankText(state.screensaverBlankMin),
        range = 0f..max.toFloat(),
        steps = max / InputPrefs.SCREENSAVER_BLANK_STEP - 1,
        minText = blankText(0),
        maxText = blankText(max),
        enabled = state.screensaverEnabled,
        tag = "screensaver_blank_slider",
        shape = shape,
        onChange = { actions.setScreensaverBlankMin(roundBlankMin(it)) },
    )
}

@Composable
private fun SpeedSlider(
    state: SettingsState,
    actions: SettingsActions,
    shape: androidx.compose.ui.graphics.Shape,
) {
    val min = InputPrefs.SCREENSAVER_SPEED_MIN_VALUE
    val max = InputPrefs.SCREENSAVER_SPEED_MAX_VALUE
    ValueSliderRow(
        icon = R.drawable.ic_bedtime,
        title = stringResource(R.string.screensaver_speed_title),
        value = state.screensaverSpeed.toFloat(),
        valueText = state.screensaverSpeed.toString(),
        range = min.toFloat()..max.toFloat(),
        steps = max - min - 1,
        minText = stringResource(R.string.screensaver_speed_slow),
        maxText = stringResource(R.string.screensaver_speed_fast),
        enabled = state.screensaverEnabled,
        tag = "screensaver_speed_slider",
        shape = shape,
        onChange = { actions.setScreensaverSpeed(roundSpeed(it)) },
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun MotionRow(
    state: SettingsState,
    actions: SettingsActions,
    shape: androidx.compose.ui.graphics.Shape,
) {
    val colors = MaterialTheme.colorScheme
    val modes = listOf(
        ScreensaverMotionMode.Drift to R.string.screensaver_motion_drift,
        ScreensaverMotionMode.Bounce to R.string.screensaver_motion_bounce,
    )
    Surface(shape = shape, color = colors.surfaceBright, modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(horizontal = 16.dp, vertical = 14.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Symbol(R.drawable.ic_bedtime, tint = colors.secondary)
                Text(
                    stringResource(R.string.screensaver_motion_title),
                    style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
                )
            }
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                modes.forEachIndexed { index, (mode, label) ->
                    SegmentedButton(
                        selected = state.screensaverMotion == mode,
                        onClick = { actions.setScreensaverMotion(mode) },
                        shape = SegmentedButtonDefaults.itemShape(index, modes.size),
                        enabled = state.screensaverEnabled,
                        modifier = Modifier.testTag("screensaver_motion_${mode.key}"),
                    ) {
                        Text(stringResource(label), maxLines = 2)
                    }
                }
            }
        }
    }
}

@Composable
private fun ReplayRow(actions: SettingsActions, modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    Surface(
        onClick = actions::replayOnboarding,
        shape = RoundedCornerShape(24.dp),
        color = colors.surfaceBright,
        modifier = modifier.fillMaxWidth().semantics { role = Role.Button }.testTag("replay_onboarding"),
    ) {
        Row(
            Modifier.heightIn(min = DisplayRowMinHeight).padding(DisplayRowPadding),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Symbol(R.drawable.ic_replay, tint = colors.secondary)
            Column(Modifier.weight(1f)) {
                Text(
                    stringResource(R.string.onboarding_replay_title),
                    style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
                )
                Text(
                    stringResource(R.string.onboarding_replay_desc),
                    style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant,
                )
            }
        }
    }
}

/** "Never" for 0, otherwise minutes. */
@Composable
internal fun blankText(min: Int): String =
    if (min == 0) stringResource(R.string.screensaver_blank_never) else pluralStringResource(R.plurals.settings_minutes, min, min)

/** Summary of the screensaver for the section list. */
@Composable
internal fun displaySummary(state: SettingsState): String =
    if (state.screensaverEnabled) {
        pluralStringResource(R.plurals.settings_minutes, state.screensaverIdleMin, state.screensaverIdleMin)
    } else {
        stringResource(R.string.screensaver_off)
    }

@Composable
private fun ValueSliderRow(
    icon: Int,
    title: String,
    value: Float,
    valueText: String,
    range: ClosedFloatingPointRange<Float>,
    steps: Int,
    minText: String,
    maxText: String,
    enabled: Boolean,
    tag: String,
    shape: androidx.compose.ui.graphics.Shape,
    onChange: (Float) -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    Surface(shape = shape, color = colors.surfaceBright, modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(horizontal = 16.dp, vertical = 14.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Symbol(icon, tint = colors.secondary)
                Text(
                    title, Modifier.weight(1f),
                    style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
                )
                Text(
                    valueText,
                    style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
                    color = colors.onSurface,
                )
            }
            Slider(
                value = value,
                onValueChange = onChange,
                valueRange = range,
                steps = steps,
                enabled = enabled,
                modifier = Modifier.testTag(tag),
            )
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                val style = MaterialTheme.typography.labelSmall
                Text(minText, style = style, color = colors.onSurfaceVariant)
                Text(maxText, style = style, color = colors.onSurfaceVariant)
            }
        }
    }
}

package com.librestatic.ezdisplay.onboarding

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import androidx.annotation.DrawableRes
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.core.net.toUri
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.librestatic.ezdisplay.R
import com.librestatic.ezdisplay.display.PanelType
import com.librestatic.ezdisplay.screensaver.ScreensaverContent
import com.librestatic.ezdisplay.settings.ConnectionContent
import com.librestatic.ezdisplay.settings.Profiles
import com.librestatic.ezdisplay.settings.SettingsActions
import com.librestatic.ezdisplay.settings.SettingsState
import com.librestatic.ezdisplay.settings.Symbol
import kotlinx.coroutines.delay

/** Where the PC app and its packages are published. */
internal const val ReleasesUrl = "https://github.com/LibreStatic/ezdisplay/releases"

/** Starts the host from a terminal; the packaged tray app does the same from the launcher. */
internal const val HostCommand = "ezd-host run --wifi"

@Composable
private fun InfoCard(@DrawableRes icon: Int, @StringRes title: Int, @StringRes desc: Int, tag: String) {
    val colors = MaterialTheme.colorScheme
    Surface(shape = RoundedCornerShape(24.dp), color = colors.surfaceBright, modifier = Modifier.fillMaxWidth().testTag(tag)) {
        Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Symbol(icon, tint = colors.secondary)
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(stringResource(title), style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold)
                Text(stringResource(desc), style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant)
            }
        }
    }
}

/** Installing the PC app: what it runs on, a download button and the command that starts it. */
@Composable
internal fun ColumnScope.HostInstallContent() {
    val colors = MaterialTheme.colorScheme
    val context = LocalContext.current
    InfoCard(R.drawable.ic_computer, R.string.onboarding_host_linux_title, R.string.onboarding_host_linux_desc, "onboarding_host_linux")
    InfoCard(R.drawable.ic_desktop_windows, R.string.onboarding_host_windows_title, R.string.onboarding_host_windows_desc, "onboarding_host_windows")
    FilledTonalButton(
        onClick = { runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, ReleasesUrl.toUri())) } },
        modifier = Modifier.testTag("onboarding_download"),
    ) {
        Symbol(R.drawable.ic_download, size = 20.dp)
        Text(stringResource(R.string.onboarding_host_download), Modifier.padding(start = 8.dp))
    }
    Text(
        stringResource(R.string.onboarding_host_run),
        style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant,
    )
    CommandRow(HostCommand, context)
}

@Composable
private fun CommandRow(command: String, context: Context) {
    val colors = MaterialTheme.colorScheme
    var copied by remember { mutableStateOf(false) }
    LaunchedEffect(copied) {
        if (copied) {
            delay(2000)
            copied = false
        }
    }
    Surface(shape = RoundedCornerShape(20.dp), color = colors.surfaceContainerHighest, modifier = Modifier.fillMaxWidth().testTag("onboarding_command")) {
        Row(Modifier.padding(start = 16.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(
                command, Modifier.weight(1f).padding(vertical = 12.dp),
                style = MaterialTheme.typography.bodyLarge, fontFamily = FontFamily.Monospace,
            )
            IconButton(
                onClick = {
                    runCatching {
                        context.getSystemService(ClipboardManager::class.java)
                            .setPrimaryClip(ClipData.newPlainText("ezd-host", command))
                        copied = true
                    }
                },
                modifier = Modifier.testTag("onboarding_copy"),
            ) {
                Symbol(
                    if (copied) R.drawable.ic_check else R.drawable.ic_content_copy,
                    contentDescription = stringResource(R.string.onboarding_host_copy),
                )
            }
        }
    }
}

@Composable
private fun ChoiceCard(
    @DrawableRes icon: Int,
    @StringRes title: Int,
    selected: Boolean,
    onClick: () -> Unit,
    tag: String,
    modifier: Modifier = Modifier,
) {
    val colors = MaterialTheme.colorScheme
    Surface(
        selected = selected,
        onClick = onClick,
        shape = RoundedCornerShape(24.dp),
        color = if (selected) colors.primaryContainer else colors.surfaceBright,
        contentColor = if (selected) colors.onPrimaryContainer else colors.onSurface,
        modifier = modifier.semantics { role = Role.RadioButton }.testTag(tag),
    ) {
        Column(
            Modifier.fillMaxWidth().heightIn(min = 96.dp).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterVertically),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Symbol(icon, size = 32.dp)
            Text(stringResource(title), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
        }
    }
}

/** USB or Wi-Fi: how to plug in, or the same pairing UI as the settings. */
@Composable
internal fun ColumnScope.ConnectionStepContent(state: SettingsState, actions: SettingsActions) {
    var wifi by rememberSaveable { mutableStateOf(state.wifiEnabled) }
    Row(Modifier.fillMaxWidth().selectableGroup(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        ChoiceCard(R.drawable.ic_usb, R.string.onboarding_connection_usb, !wifi, { wifi = false }, "onboarding_choice_usb", Modifier.weight(1f))
        ChoiceCard(R.drawable.ic_wifi, R.string.onboarding_connection_wifi, wifi, { wifi = true }, "onboarding_choice_wifi", Modifier.weight(1f))
    }
    if (!wifi) {
        InfoCard(R.drawable.ic_usb, R.string.onboarding_connection_usb, R.string.onboarding_connection_usb_desc, "onboarding_usb_info")
    } else {
        Text(
            stringResource(R.string.onboarding_connection_wifi_desc),
            style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.testTag("onboarding_wifi_info"),
        )
        ConnectionContent(state, actions)
    }
}

/** One selectable card per pen-button profile, each with a small animation of what it does. */
@Composable
internal fun ColumnScope.PenProfileContent(state: SettingsState, actions: SettingsActions) {
    val colors = MaterialTheme.colorScheme
    Column(Modifier.fillMaxWidth().selectableGroup(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Profiles.filter { it.value != "custom" }.forEach { profile ->
            val selected = state.profile == profile.value
            Surface(
                selected = selected,
                onClick = { actions.setProfile(profile.value) },
                shape = RoundedCornerShape(24.dp),
                color = if (selected) colors.primaryContainer else colors.surfaceBright,
                contentColor = if (selected) colors.onPrimaryContainer else colors.onSurface,
                modifier = Modifier.fillMaxWidth().semantics { role = Role.RadioButton }.testTag("onboarding_profile_${profile.value}"),
            ) {
                Row(
                    Modifier.padding(12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    ProfileHint(profile.value)
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(stringResource(profile.title), style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold)
                        Text(stringResource(profile.desc), style = MaterialTheme.typography.bodySmall)
                    }
                    if (selected) Symbol(R.drawable.ic_check_circle, contentDescription = stringResource(R.string.profile_selected))
                }
            }
        }
    }
    Text(
        stringResource(R.string.onboarding_pen_later),
        style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant,
    )
}

@Composable
private fun panelName(type: PanelType): String = stringResource(
    when (type) {
        PanelType.Oled -> R.string.panel_oled
        PanelType.Lcd -> R.string.panel_lcd
        PanelType.Unknown -> R.string.panel_unknown
    },
)

/** Detected screen, the screensaver switch with a live preview and its timings, and the swipe-down gesture. */
@Composable
internal fun ColumnScope.DisplayStepContent(state: SettingsState, actions: SettingsActions) {
    val colors = MaterialTheme.colorScheme
    Surface(
        checked = state.screensaverEnabled,
        onCheckedChange = actions::setScreensaverEnabled,
        shape = RoundedCornerShape(24.dp),
        color = colors.surfaceBright,
        modifier = Modifier.fillMaxWidth().semantics { role = Role.Switch }.testTag("onboarding_screensaver_row"),
    ) {
        Row(
            Modifier.heightIn(min = 72.dp).padding(horizontal = 16.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Symbol(R.drawable.ic_bedtime, tint = colors.secondary)
            Column(Modifier.weight(1f)) {
                Text(stringResource(R.string.screensaver_title), style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold)
                Text(
                    stringResource(R.string.screensaver_detected, panelName(state.panelType)),
                    style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant,
                    modifier = Modifier.testTag("onboarding_panel_type"),
                )
            }
            Switch(checked = state.screensaverEnabled, onCheckedChange = null)
        }
    }
    Box(Modifier.fillMaxWidth().height(120.dp).clip(RoundedCornerShape(20.dp)).testTag("onboarding_screensaver_preview")) {
        ScreensaverContent(
            Modifier.fillMaxSize(), count = 3, speed = state.screensaverSpeed, motion = state.screensaverMotion,
        )
    }
    val idle = pluralStringResource(R.plurals.settings_minutes, state.screensaverIdleMin, state.screensaverIdleMin)
    Text(
        if (state.screensaverBlankMin == 0) {
            stringResource(R.string.onboarding_display_explain_never, idle, state.screensaverBrightnessPct)
        } else {
            stringResource(
                R.string.onboarding_display_explain, idle, state.screensaverBrightnessPct,
                pluralStringResource(R.plurals.settings_minutes, state.screensaverBlankMin, state.screensaverBlankMin),
            )
        },
        style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant,
        modifier = Modifier.testTag("onboarding_display_explain"),
    )
    Surface(shape = RoundedCornerShape(24.dp), color = colors.surfaceBright, modifier = Modifier.fillMaxWidth().testTag("onboarding_gesture")) {
        Row(
            Modifier.padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            SwipeDownHint()
            Text(stringResource(R.string.onboarding_display_gesture), style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
        }
    }
}

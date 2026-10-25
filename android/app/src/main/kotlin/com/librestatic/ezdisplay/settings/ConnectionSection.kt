package com.librestatic.ezdisplay.settings

import androidx.annotation.StringRes
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.sp
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.librestatic.ezdisplay.R
import com.librestatic.ezdisplay.wifi.DiscoveredHost
import com.librestatic.ezdisplay.wifi.PIN_LENGTH
import com.librestatic.ezdisplay.wifi.WifiHost
import com.librestatic.ezdisplay.wifi.fingerprintMatches
import com.librestatic.ezdisplay.wifi.isLocalAddress
import com.librestatic.ezdisplay.wifi.isValidPin
import com.librestatic.ezdisplay.wifi.parseManualAddress

private val RowMinHeight = 72.dp
private val RowPadding = PaddingValues(horizontal = 16.dp, vertical = 12.dp)

/** The localized message of a pairing error code (`wifiPair` JSON `error`). */
@StringRes
fun wifiErrorText(code: String?): Int = when (code) {
    "wrong_pin" -> R.string.pair_error_wrong_pin
    "pairing_closed" -> R.string.pair_error_closed
    "too_many_attempts" -> R.string.pair_error_attempts
    "busy" -> R.string.pair_error_busy
    "not_paired" -> R.string.pair_error_not_paired
    "unreachable" -> R.string.pair_error_unreachable
    "timeout" -> R.string.pair_error_timeout
    "fingerprint_mismatch" -> R.string.pair_error_fingerprint
    "invalid_pin" -> R.string.pair_error_invalid_pin
    "invalid_uri" -> R.string.pair_error_invalid_uri
    "not_local_address" -> R.string.pair_error_not_local
    "invalid_address" -> R.string.pair_error_invalid_address
    else -> R.string.pair_error_other
}

/** Discovered hosts that are not paired yet. */
internal fun unpairedDiscovered(state: SettingsState): List<DiscoveredHost> =
    state.discovered.filter { d -> state.wifiHosts.none { fingerprintMatches(it.fingerprint, d.shortFp) } }

/** The whole Connection section: [WifiSwitchAndWarning] then [HostsContent]. */
@Composable
fun ConnectionContent(state: SettingsState, actions: SettingsActions, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxWidth().testTag("connection_content"), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        WifiSwitchAndWarning(state, actions)
        HostsContent(state, actions)
    }
}

/** The Wi-Fi switch and the latency warning under it. */
@Composable
fun WifiSwitchAndWarning(state: SettingsState, actions: SettingsActions, modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        WifiSwitchRow(state.wifiEnabled, actions::setWifiEnabled)
        Surface(
            shape = RoundedCornerShape(28.dp), color = colors.surfaceContainerHigh,
            modifier = Modifier.fillMaxWidth().testTag("wifi_warning"),
        ) {
            Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Symbol(R.drawable.ic_info, tint = colors.onSurfaceVariant)
                Text(
                    stringResource(R.string.wifi_latency_warning),
                    style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant,
                )
            }
        }
    }
}

/**
 * Paired hosts (Forget), hosts found on the network (Pair), a manual address and this device's fingerprint. The
 * pairing dialog itself is [PairDialog].
 */
@Composable
fun HostsContent(state: SettingsState, actions: SettingsActions, modifier: Modifier = Modifier) {
    val found = unpairedDiscovered(state)
    Column(modifier.fillMaxWidth().testTag("hosts_content"), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        GroupTitle(R.string.wifi_paired_title)
        HostGroup(count = state.wifiHosts.size, empty = R.string.wifi_paired_empty, emptyTag = "paired_empty") { i, shape ->
            PairedRow(state.wifiHosts[i], shape, actions)
        }
        GroupTitle(R.string.wifi_discovered_title)
        HostGroup(count = found.size, empty = R.string.wifi_discovered_empty, emptyTag = "discovered_empty") { i, shape ->
            DiscoveredRow(found[i], shape, actions)
        }
        ManualAddress(actions)
        DeviceFingerprint(state.deviceFingerprint)
    }
}

/**
 * Wide page version of the section: the switch and warning, and a row that opens the hosts in a dialog, so the
 * page keeps its height (it must not scroll).
 */
@Composable
fun ConnectionSummary(state: SettingsState, actions: SettingsActions, modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    var open by rememberSaveable { mutableStateOf(false) }
    Column(modifier.fillMaxWidth().testTag("connection_content"), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        WifiSwitchAndWarning(state, actions)
        Surface(
            onClick = { open = true },
            shape = RoundedCornerShape(24.dp), color = colors.surfaceBright,
            modifier = Modifier.fillMaxWidth().testTag("connection_manage"),
        ) {
            Row(
                Modifier.heightIn(min = RowMinHeight).padding(RowPadding),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Symbol(R.drawable.ic_computer, tint = colors.secondary)
                Column(Modifier.weight(1f)) {
                    Text(
                        stringResource(R.string.wifi_manage_title),
                        style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
                    )
                    Text(
                        pluralStringResource(R.plurals.wifi_manage_summary, state.wifiHosts.size, state.wifiHosts.size),
                        style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant,
                    )
                }
                Symbol(R.drawable.ic_chevron_right, tint = colors.primary)
            }
        }
    }
    if (open) {
        Dialog(onDismissRequest = { open = false }, properties = DialogProperties(usePlatformDefaultWidth = false)) {
            Surface(
                shape = RoundedCornerShape(28.dp), color = colors.surfaceContainer,
                modifier = Modifier.widthIn(max = 560.dp).fillMaxWidth(0.9f).testTag("hosts_dialog"),
            ) {
                Column(Modifier.verticalScroll(rememberScrollState()).padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            stringResource(R.string.wifi_manage_title), Modifier.weight(1f),
                            style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold,
                        )
                        TextButton(onClick = { open = false }, modifier = Modifier.testTag("hosts_dialog_close")) {
                            Text(stringResource(R.string.wifi_manage_close))
                        }
                    }
                    HostsContent(state, actions)
                }
            }
        }
    }
}

@Composable
private fun PairedRow(h: WifiHost, shape: RoundedCornerShape, actions: SettingsActions) {
    val name = h.name.ifEmpty { h.shortFp }
    HostRow(icon = R.drawable.ic_link, title = name, subtitle = h.shortFp, shape = shape, tag = "paired_${h.fingerprint}") {
        IconButton(onClick = { actions.forgetHost(h.fingerprint) }, modifier = Modifier.testTag("forget_${h.fingerprint}")) {
            Symbol(R.drawable.ic_delete, contentDescription = stringResource(R.string.wifi_forget_cd, name))
        }
    }
}

@Composable
private fun DiscoveredRow(h: DiscoveredHost, shape: RoundedCornerShape, actions: SettingsActions) {
    HostRow(
        icon = R.drawable.ic_computer, title = h.name, subtitle = h.shortFp.ifEmpty { h.addr }, shape = shape,
        tag = "discovered_${h.serviceName}",
    ) {
        TextButton(
            onClick = { actions.startPairing(h.name, h.shortFp, h.addr) },
            modifier = Modifier.testTag("pair_${h.serviceName}"),
        ) {
            Text(stringResource(R.string.wifi_pair_action))
        }
    }
}

@Composable
private fun GroupTitle(@StringRes text: Int) {
    Text(
        stringResource(text), Modifier.padding(start = 8.dp, top = 4.dp),
        style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun WifiSwitchRow(on: Boolean, onChange: (Boolean) -> Unit) {
    val colors = MaterialTheme.colorScheme
    Surface(
        checked = on,
        onCheckedChange = onChange,
        shape = RoundedCornerShape(24.dp),
        color = colors.surfaceBright,
        modifier = Modifier.fillMaxWidth().semantics { role = Role.Switch }.testTag("wifi_row"),
    ) {
        Row(
            Modifier.heightIn(min = RowMinHeight).padding(RowPadding),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Symbol(R.drawable.ic_wifi, tint = colors.secondary)
            Column(Modifier.weight(1f)) {
                Text(
                    stringResource(R.string.wifi_switch_title),
                    style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
                )
                Text(
                    stringResource(R.string.wifi_switch_desc),
                    style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant,
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

/** A segmented group of [count] rows from [row], or one row with the [empty] text. */
@Composable
private fun HostGroup(
    count: Int,
    @StringRes empty: Int,
    emptyTag: String,
    row: @Composable (index: Int, shape: RoundedCornerShape) -> Unit,
) {
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        if (count == 0) {
            Surface(shape = RoundedCornerShape(24.dp), color = MaterialTheme.colorScheme.surfaceBright, modifier = Modifier.fillMaxWidth()) {
                Text(
                    stringResource(empty),
                    Modifier.padding(RowPadding).testTag(emptyTag),
                    style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        } else {
            repeat(count) { row(it, segmentShape(it, count)) }
        }
    }
}

@Composable
private fun HostRow(
    icon: Int,
    title: String,
    subtitle: String,
    shape: RoundedCornerShape,
    tag: String,
    trailing: @Composable () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    Surface(shape = shape, color = colors.surfaceBright, modifier = Modifier.fillMaxWidth().testTag(tag)) {
        Row(
            Modifier.heightIn(min = RowMinHeight).padding(RowPadding),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Surface(shape = RoundedCornerShape(14.dp), color = colors.secondaryContainer, modifier = Modifier.size(42.dp)) {
                Box(contentAlignment = Alignment.Center) { Symbol(icon, tint = colors.onSecondaryContainer) }
            }
            Column(Modifier.weight(1f)) {
                Text(
                    title, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
                )
                if (subtitle.isNotEmpty()) {
                    Text(subtitle, style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant)
                }
            }
            trailing()
        }
    }
}

@Composable
private fun ManualAddress(actions: SettingsActions) {
    val colors = MaterialTheme.colorScheme
    var text by rememberSaveable { mutableStateOf("") }
    val addr = parseManualAddress(text)
    Surface(shape = RoundedCornerShape(24.dp), color = colors.surfaceBright, modifier = Modifier.fillMaxWidth().testTag("manual_row")) {
        Column(Modifier.padding(RowPadding), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                stringResource(R.string.wifi_manual_title),
                style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
            )
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(
                    value = text,
                    onValueChange = { text = it.take(60) },
                    modifier = Modifier.weight(1f).testTag("manual_field"),
                    label = { Text(stringResource(R.string.wifi_manual_label)) },
                    singleLine = true,
                    isError = text.isNotBlank() && addr == null,
                    supportingText = if (addr != null && !isLocalAddress(addr)) {
                        { Text(stringResource(R.string.wifi_manual_public_warning), Modifier.testTag("manual_warning")) }
                    } else null,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                )
                Button(
                    onClick = { addr?.let { actions.startPairing(it, "", it) } },
                    enabled = addr != null,
                    modifier = Modifier.testTag("manual_pair"),
                ) { Text(stringResource(R.string.wifi_pair_action)) }
            }
        }
    }
}

@Composable
private fun DeviceFingerprint(shortFp: String) {
    val colors = MaterialTheme.colorScheme
    Surface(shape = RoundedCornerShape(24.dp), color = colors.surfaceBright, modifier = Modifier.fillMaxWidth().testTag("device_fp")) {
        Row(
            Modifier.heightIn(min = RowMinHeight).padding(RowPadding),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Symbol(R.drawable.ic_fingerprint, tint = colors.secondary)
            Column(Modifier.weight(1f)) {
                Text(
                    stringResource(R.string.wifi_device_fp_title),
                    style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
                )
                Text(
                    stringResource(R.string.wifi_device_fp_desc),
                    style = MaterialTheme.typography.bodySmall, color = colors.onSurfaceVariant,
                )
            }
            Text(
                shortFp.ifEmpty { "-" },
                style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Bold,
                fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace,
            )
        }
    }
}

/** Summary line of the Connection section for the list. */
@Composable
internal fun connectionSummary(state: SettingsState): String =
    if (!state.wifiEnabled) stringResource(R.string.connection_summary_usb)
    else pluralStringResource(R.plurals.connection_summary_wifi, state.wifiHosts.size, state.wifiHosts.size)

/** The pairing dialog: PIN entry for a found or typed host, or the confirmation for a scanned code. */
@Composable
fun PairDialog(pairing: PairingState, actions: SettingsActions) {
    var pin by rememberSaveable { mutableStateOf("") }
    val scanned = pairing.link != null
    val canPair = !pairing.busy && (scanned || isValidPin(pin))
    Dialog(
        onDismissRequest = { if (!pairing.busy) actions.cancelPairing() },
        properties = DialogProperties(usePlatformDefaultWidth = false),
    ) {
        Surface(
            shape = RoundedCornerShape(28.dp), color = MaterialTheme.colorScheme.surfaceContainerHigh,
            modifier = Modifier.widthIn(max = 560.dp).fillMaxWidth(0.9f).testTag("pair_dialog"),
        ) {
            Column(Modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(
                    stringResource(R.string.pair_dialog_title, pairing.name),
                    style = MaterialTheme.typography.headlineSmall,
                )
                if (pairing.shortFp.isNotEmpty()) {
                    Text(
                        stringResource(R.string.pair_dialog_fingerprint, pairing.shortFp),
                        Modifier.testTag("pair_fp"),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
                if (pairing.link != null) {
                    Text(
                        stringResource(R.string.pair_dialog_address, pairing.link.addr),
                        Modifier.testTag("pair_addr"),
                        style = MaterialTheme.typography.bodyLarge,
                    )
                    Text(
                        pairing.link.pin,
                        Modifier.fillMaxWidth().testTag("pair_pin"),
                        style = MaterialTheme.typography.displayMedium.copy(fontFamily = FontFamily.Monospace, letterSpacing = 6.sp),
                        fontWeight = FontWeight.Bold,
                        textAlign = TextAlign.Center,
                    )
                    Text(stringResource(R.string.pair_dialog_pin_check), style = MaterialTheme.typography.bodyMedium)
                    Text(stringResource(R.string.pair_dialog_scanned), style = MaterialTheme.typography.bodyMedium)
                } else {
                    OutlinedTextField(
                        value = pin,
                        onValueChange = { v -> pin = v.filter { it in '0'..'9' }.take(PIN_LENGTH) },
                        modifier = Modifier.fillMaxWidth().testTag("pin_field"),
                        label = { Text(stringResource(R.string.pair_dialog_pin_label)) },
                        supportingText = { Text(stringResource(R.string.pair_dialog_pin_help)) },
                        singleLine = true,
                        enabled = !pairing.busy,
                        isError = pairing.error != null,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                    )
                }
                if (pairing.busy) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 3.dp)
                        Text(stringResource(R.string.pair_dialog_busy), style = MaterialTheme.typography.bodyMedium)
                    }
                }
                pairing.error?.let {
                    Text(
                        stringResource(wifiErrorText(it)), Modifier.testTag("pair_error"),
                        style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error,
                    )
                }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                    TextButton(onClick = actions::cancelPairing, enabled = !pairing.busy, modifier = Modifier.testTag("pair_cancel")) {
                        Text(stringResource(R.string.pair_dialog_cancel))
                    }
                    TextButton(onClick = { actions.submitPairing(pin) }, enabled = canPair, modifier = Modifier.testTag("pair_confirm")) {
                        Text(stringResource(R.string.pair_dialog_confirm))
                    }
                }
            }
        }
    }
}

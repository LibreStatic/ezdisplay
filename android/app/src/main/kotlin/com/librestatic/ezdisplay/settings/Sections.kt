package com.librestatic.ezdisplay.settings

import androidx.annotation.DrawableRes
import androidx.annotation.StringRes
import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ExposedDropdownMenuAnchorType
import androidx.compose.material3.ExposedDropdownMenu
import androidx.compose.material3.ExposedDropdownMenuBox
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.ListItemShapes
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderState
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.ToggleButton
import androidx.compose.material3.ButtonGroupDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.InputPrefs.KeyCombo
import com.librestatic.ezdisplay.R
import java.text.NumberFormat

private val GroupEnd = 24.dp
private val GroupJoin = 4.dp
private val RowMinHeight = 72.dp
private val RowPadding = PaddingValues(horizontal = 16.dp, vertical = 12.dp)

/** Shape of item [index] of a segmented group of [count]: 24 dp at the group ends, 4 dp at the joins. */
internal fun segmentShape(index: Int, count: Int): RoundedCornerShape {
    val top = if (index == 0) GroupEnd else GroupJoin
    val bottom = if (index == count - 1) GroupEnd else GroupJoin
    return RoundedCornerShape(topStart = top, topEnd = top, bottomStart = bottom, bottomEnd = bottom)
}

@Composable
private fun segmentedItemShapes(index: Int, count: Int, selectedShape: Shape? = null): ListItemShapes =
    ListItemDefaults.shapes(shape = segmentShape(index, count), selectedShape = selectedShape)

@Composable
private fun groupItemColors() =
    ListItemDefaults.segmentedColors(containerColor = MaterialTheme.colorScheme.surfaceBright)

/** A Material Symbols drawable drawn as an icon. */
@Composable
internal fun Symbol(
    @DrawableRes id: Int,
    modifier: Modifier = Modifier,
    tint: Color = LocalContentColor.current,
    size: Dp = 24.dp,
    contentDescription: String? = null,
) {
    Icon(painterResource(id), contentDescription, modifier.size(size), tint)
}

/** Section heading above a group. */
@Composable
internal fun SectionTitle(text: String, modifier: Modifier = Modifier) {
    Text(
        text,
        modifier.padding(top = 16.dp, bottom = 12.dp),
        style = MaterialTheme.typography.titleLarge,
        fontWeight = FontWeight.Bold,
    )
}

// ---------------------------------------------------------------- Profiles

/** Gap between profile cards, also used above the row that follows them. */
internal fun profileGap(columns: Int): Dp = if (columns == 3) 12.dp else 8.dp

/**
 * The profile cards: unselected ones in their natural order, the selected one last and wider.
 * [columns] slots per row (2 or 3); an unselected card takes one slot and the selected one takes two
 * (the whole row with 2 columns). Widths are computed from the available width so a slot is the same size in
 * every row.
 */
@Composable
fun ProfileCards(
    selected: String,
    onSelect: (String) -> Unit,
    columns: Int,
    modifier: Modifier = Modifier,
    unselectedHeight: Dp = if (columns == 3) 182.dp else 86.dp,
    selectedHeight: Dp = if (columns == 3) 182.dp else 116.dp,
) {
    val items = Profiles.filter { it.value != selected }.map { it to 1 } +
        listOfNotNull(Profiles.firstOrNull { it.value == selected }?.let { it to 2 })
    val rows = mutableListOf<MutableList<Pair<ProfileSpec, Int>>>()
    for (item in items) {
        val last = rows.lastOrNull()
        if (last != null && last.sumOf { it.second } + item.second <= columns) last += item else rows += mutableListOf(item)
    }
    val gap = profileGap(columns)
    BoxWithConstraints(modifier.fillMaxWidth()) {
        val slot = (maxWidth - gap * (columns - 1)) / columns
        Column(Modifier.fillMaxWidth().animateContentSize(), verticalArrangement = Arrangement.spacedBy(gap)) {
            for (row in rows) {
                Row(Modifier.height(IntrinsicSize.Min), horizontalArrangement = Arrangement.spacedBy(gap)) {
                    for ((profile, slots) in row) {
                        val isSelected = profile.value == selected
                        ProfileCard(
                            profile, isSelected, columns == 3, { onSelect(profile.value) },
                            Modifier.width(slot * slots + gap * (slots - 1)).fillMaxHeight()
                                .heightIn(min = if (isSelected) selectedHeight else unselectedHeight),
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun ProfileCard(profile: ProfileSpec, isSelected: Boolean, large: Boolean, onClick: () -> Unit, modifier: Modifier) {
    val colors = MaterialTheme.colorScheme
    val type = MaterialTheme.typography
    val shape = RoundedCornerShape(if (isSelected) (if (large) 36.dp else 32.dp) else 28.dp)
    val container = if (isSelected) colors.primaryContainer else colors.surfaceBright
    val content = if (isSelected) colors.onPrimaryContainer else colors.onSurface
    val iconTint = if (isSelected) colors.primary else colors.secondary
    val pad = if (large) 22.dp else if (isSelected) 16.dp else 12.dp
    Surface(
        onClick = onClick,
        modifier = modifier.semantics { this.selected = isSelected },
        shape = shape,
        color = container,
        contentColor = content,
    ) {
        when {
            !isSelected -> Column(
                Modifier.padding(pad).fillMaxWidth(),
                verticalArrangement = Arrangement.SpaceBetween,
            ) {
                Symbol(profile.icon, tint = iconTint, size = if (large) 38.dp else 27.dp)
                Text(
                    stringResource(profile.title),
                    style = if (large) type.titleLarge else type.titleSmall,
                    fontWeight = FontWeight.Bold,
                )
            }
            large -> Column(Modifier.padding(pad).fillMaxWidth(), verticalArrangement = Arrangement.SpaceBetween) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
                    Symbol(profile.icon, tint = iconTint, size = 38.dp)
                    SelectedMarker()
                }
                Column {
                    Text(stringResource(profile.title), style = type.titleLarge, fontWeight = FontWeight.Bold)
                    Text(stringResource(profile.desc), Modifier.padding(top = 6.dp), style = type.bodySmall)
                }
            }
            else -> Row(Modifier.padding(pad).fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Symbol(profile.icon, tint = iconTint, size = 32.dp)
                Column(Modifier.weight(1f)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            stringResource(profile.title), Modifier.weight(1f),
                            style = type.titleMedium, fontWeight = FontWeight.Bold,
                        )
                        SelectedMarker()
                    }
                    Text(stringResource(profile.desc), Modifier.padding(top = 6.dp), style = type.bodySmall)
                }
            }
        }
    }
}

@Composable
private fun SelectedMarker() {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Symbol(R.drawable.ic_check_circle, tint = MaterialTheme.colorScheme.onPrimaryContainer, size = 20.dp)
        Text(
            stringResource(R.string.profile_selected),
            style = MaterialTheme.typography.labelLarge,
            fontWeight = FontWeight.Bold,
            color = MaterialTheme.colorScheme.onPrimaryContainer,
        )
    }
}

// ---------------------------------------------------------------- Shortcuts

@Composable
private fun comboParts(combo: KeyCombo): List<String> {
    val context = LocalContext.current
    val meta = stringResource(R.string.key_meta)
    return buildList {
        if (combo.mods and InputPrefs.MOD_CTRL != 0) add("Ctrl")
        if (combo.mods and InputPrefs.MOD_SHIFT != 0) add("Shift")
        if (combo.mods and InputPrefs.MOD_ALT != 0) add("Alt")
        if (combo.mods and InputPrefs.MOD_META != 0) add(meta)
        if (combo.usage != 0) add(InputPrefs.keyLabel(context, combo.usage))
        if (isEmpty()) add("-")
    }
}

/** A key combination drawn as keycaps with "+" between them; [large] is the editor preview style. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun KeyCaps(combo: KeyCombo, modifier: Modifier = Modifier, large: Boolean = false) {
    val colors = MaterialTheme.colorScheme
    val gap = if (large) 12.dp else 6.dp
    FlowRow(
        modifier,
        horizontalArrangement = Arrangement.spacedBy(gap, if (large) Alignment.CenterHorizontally else Alignment.End),
        verticalArrangement = Arrangement.spacedBy(gap),
        itemVerticalAlignment = Alignment.CenterVertically,
    ) {
        comboParts(combo).forEachIndexed { i, part ->
            if (i > 0) {
                Text(
                    "+",
                    style = if (large) MaterialTheme.typography.titleLarge else MaterialTheme.typography.labelLarge,
                    color = if (large) colors.onPrimaryContainer else colors.onSurfaceVariant,
                )
            }
            Surface(
                shape = RoundedCornerShape(if (large) 18.dp else 11.dp),
                color = if (large) colors.onPrimaryContainer else colors.surfaceContainerHighest,
                contentColor = if (large) colors.onPrimary else colors.onSurface,
                border = if (large) null else BorderStroke(1.dp, colors.outline),
            ) {
                Text(
                    part,
                    Modifier.padding(horizontal = if (large) 18.dp else 12.dp, vertical = if (large) 13.dp else 9.dp),
                    style = if (large) MaterialTheme.typography.titleLarge else MaterialTheme.typography.labelLarge,
                    fontWeight = FontWeight.ExtraBold,
                )
            }
        }
    }
}

/** Row showing the combination of [prefKey]; [onClick] opens its editor. [minHeight] is 72 dp on Phone, 88 dp on wider layouts. */
@Composable
fun ShortcutRow(
    prefKey: String,
    combo: KeyCombo,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    shape: Shape = RoundedCornerShape(GroupEnd),
    highlighted: Boolean = false,
    minHeight: Dp = RowMinHeight,
) {
    val colors = MaterialTheme.colorScheme
    val title = KeyEditors.first { it.prefKey == prefKey }.title
    Surface(
        onClick = onClick,
        modifier = modifier.fillMaxWidth().testTag("shortcut_$prefKey"),
        shape = shape,
        color = colors.surfaceBright,
        border = if (highlighted) BorderStroke(2.dp, colors.primary) else null,
    ) {
        Row(
            Modifier.heightIn(min = minHeight).padding(RowPadding),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Surface(shape = RoundedCornerShape(14.dp), color = colors.secondaryContainer, modifier = Modifier.size(42.dp)) {
                Box(contentAlignment = Alignment.Center) {
                    Symbol(R.drawable.ic_keyboard, tint = colors.onSecondaryContainer)
                }
            }
            Text(
                stringResource(title), Modifier.weight(1f),
                style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
            )
            KeyCaps(combo)
            Symbol(R.drawable.ic_chevron_right, tint = colors.primary, contentDescription = stringResource(R.string.cd_edit_shortcut))
        }
    }
}

private sealed interface GestureEntry {
    class Gesture(val trigger: Trigger) : GestureEntry
    class Shortcut(val prefKey: String) : GestureEntry
}

private val DragTriggers = setOf(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT, InputPrefs.CUSTOM_HOLD_DRAG_HOVER)

/** The Custom profile's gesture list; trigger rows open an action menu and `keys` actions add a shortcut line. */
@Composable
fun CustomGestures(
    state: SettingsState,
    actions: SettingsActions,
    onOpenEditor: (String) -> Unit,
    modifier: Modifier = Modifier,
    selectedEditor: String? = null,
    shortcutMinHeight: Dp = RowMinHeight,
) {
    val visible = visibleKeyEditors(state)
    val disabled = dragsDisabled(state)
    val first = buildList<GestureEntry> {
        for (t in Triggers.filter { it.key !in DragTriggers }) {
            add(GestureEntry.Gesture(t))
            KeyEditors.firstOrNull { it.triggerKey == t.key && it.prefKey in visible }
                ?.let { add(GestureEntry.Shortcut(it.prefKey)) }
        }
    }
    val drags = Triggers.filter { it.key in DragTriggers }.map { GestureEntry.Gesture(it) }
    Column(modifier.fillMaxWidth()) {
        GestureGroup(first, state, actions, onOpenEditor, selectedEditor, shortcutMinHeight, enabled = true)
        Spacer(Modifier.height(8.dp))
        if (disabled) {
            Text(
                stringResource(R.string.keys_drag_unused),
                Modifier.padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 12.dp),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        GestureGroup(drags, state, actions, onOpenEditor, selectedEditor, shortcutMinHeight, enabled = !disabled)
    }
}

@Composable
private fun GestureGroup(
    entries: List<GestureEntry>,
    state: SettingsState,
    actions: SettingsActions,
    onOpenEditor: (String) -> Unit,
    selectedEditor: String?,
    shortcutMinHeight: Dp,
    enabled: Boolean,
) {
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        entries.forEachIndexed { i, entry ->
            when (entry) {
                is GestureEntry.Gesture -> TriggerRow(entry.trigger, state, actions, segmentedItemShapes(i, entries.size), enabled)
                is GestureEntry.Shortcut -> ShortcutRow(
                    entry.prefKey, state.keys.getValue(entry.prefKey), { onOpenEditor(entry.prefKey) },
                    shape = segmentShape(i, entries.size), highlighted = entry.prefKey == selectedEditor,
                    minHeight = shortcutMinHeight,
                )
            }
        }
    }
}

@Composable
private fun TriggerRow(t: Trigger, state: SettingsState, actions: SettingsActions, shapes: ListItemShapes, enabled: Boolean) {
    var expanded by remember { mutableStateOf(false) }
    val current = state.custom[t.key] ?: t.default
    Box(Modifier.fillMaxWidth()) {
        SegmentedListItem(
            onClick = { expanded = true },
            shapes = shapes,
            modifier = Modifier.heightIn(min = RowMinHeight).testTag("trigger_${t.key}"),
            enabled = enabled,
            colors = groupItemColors(),
            trailingContent = {
                Symbol(R.drawable.ic_expand_more, contentDescription = stringResource(R.string.cd_choose_action))
            },
            supportingContent = { Text(stringResource(ActionLabels.getValue(current))) },
        ) {
            Text(stringResource(t.label), fontWeight = FontWeight.Bold)
        }
        DropdownMenu(expanded, { expanded = false }) {
            for (a in t.actions) {
                DropdownMenuItem(
                    text = { Text(stringResource(ActionLabels.getValue(a))) },
                    onClick = {
                        actions.setCustomAction(t.key, a)
                        expanded = false
                    },
                    modifier = Modifier.testTag("action_$a"),
                    trailingIcon = if (a == current) {
                        { Symbol(R.drawable.ic_check, size = 20.dp) }
                    } else {
                        null
                    },
                )
            }
        }
    }
}

// ---------------------------------------------------------------- Zoom, palm, air actions

/** Zoom level buttons and double-click time slider as one segmented group. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ZoomGroup(state: SettingsState, actions: SettingsActions, modifier: Modifier = Modifier) {
    val colors = MaterialTheme.colorScheme
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Surface(shape = segmentShape(0, 2), color = colors.surfaceBright, modifier = Modifier.fillMaxWidth()) {
            Row(
                Modifier.heightIn(min = RowMinHeight).padding(RowPadding),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Symbol(R.drawable.ic_zoom_in, tint = colors.secondary)
                Text(
                    stringResource(R.string.settings_zoom_factor), Modifier.weight(1f),
                    style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(ButtonGroupDefaults.ConnectedSpaceBetween)) {
                    val factors = InputPrefs.ZOOM_FACTORS
                    factors.forEachIndexed { i, f ->
                        ToggleButton(
                            checked = state.zoomFactor == f,
                            onCheckedChange = { actions.setZoomFactor(f) },
                            shapes = when (i) {
                                0 -> ButtonGroupDefaults.connectedLeadingButtonShapes()
                                factors.lastIndex -> ButtonGroupDefaults.connectedTrailingButtonShapes()
                                else -> ButtonGroupDefaults.connectedMiddleButtonShapes()
                            },
                            modifier = Modifier.semantics { role = Role.RadioButton },
                        ) {
                            Text(stringResource(R.string.settings_zoom_value, formatZoom(f)))
                        }
                    }
                }
            }
        }
        DoubleClickSlider(state.doubleClickMs, actions, segmentShape(1, 2))
    }
}

/** "1.5" / "2" in the user's locale. */
@Composable
internal fun formatZoom(f: Float): String {
    val locale = LocalConfiguration.current.locales[0]
    return NumberFormat.getInstance(locale).apply { maximumFractionDigits = 1 }.format(f.toDouble())
}

@Composable
private fun DoubleClickSlider(ms: Int, actions: SettingsActions, shape: Shape) {
    val colors = MaterialTheme.colorScheme
    val slider = remember {
        SliderState(ms.toFloat(), 0, InputPrefs.DOUBLE_CLICK_MIN.toFloat()..InputPrefs.DOUBLE_CLICK_MAX.toFloat())
    }
    // Follow outside changes, but keep the continuous thumb position while the stored value is its rounding.
    LaunchedEffect(ms) { if (roundDoubleClickMs(slider.value) != ms) slider.value = ms.toFloat() }
    val shown = slider.value
    Surface(shape = shape, color = colors.surfaceBright, modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(horizontal = 16.dp, vertical = 14.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Symbol(R.drawable.ic_timer, tint = colors.secondary)
                Text(
                    stringResource(R.string.settings_double_click_title), Modifier.weight(1f),
                    style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
                )
                Text(
                    stringResource(R.string.settings_ms, roundDoubleClickMs(shown)),
                    style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
                    color = colors.onSurface,
                )
            }
            Slider(
                state = slider,
                modifier = Modifier.testTag("double_click_slider"),
                onValueChange = {
                    slider.value = it
                    val rounded = roundDoubleClickMs(it)
                    if (rounded != ms) actions.setDoubleClickMs(rounded)
                },
            )
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                val style = MaterialTheme.typography.labelSmall
                Text(stringResource(R.string.settings_ms, InputPrefs.DOUBLE_CLICK_MIN), style = style, color = colors.onSurfaceVariant)
                Text(stringResource(R.string.settings_ms, InputPrefs.DOUBLE_CLICK_MAX), style = style, color = colors.onSurfaceVariant)
            }
        }
    }
}

/** Palm rejection toggle. */
@Composable
fun PalmRow(on: Boolean, onChange: (Boolean) -> Unit, modifier: Modifier = Modifier) {
    SwitchRow(
        on, onChange, R.drawable.ic_back_hand, R.string.settings_palm_title, R.string.settings_palm_desc,
        modifier.testTag("palm_row"),
    )
}

/** Audio toggle: play the PC's audio on this device. */
@Composable
fun AudioRow(on: Boolean, onChange: (Boolean) -> Unit, modifier: Modifier = Modifier) {
    SwitchRow(
        on, onChange, R.drawable.ic_volume_up, R.string.settings_audio_title, R.string.settings_audio_desc,
        modifier.testTag("audio_row"),
    )
}

@Composable
private fun SwitchRow(
    on: Boolean,
    onChange: (Boolean) -> Unit,
    @DrawableRes icon: Int,
    @StringRes title: Int,
    @StringRes desc: Int,
    modifier: Modifier,
) {
    val colors = MaterialTheme.colorScheme
    // The whole row is the switch, so TalkBack reads one control.
    Surface(
        checked = on,
        onCheckedChange = onChange,
        shape = RoundedCornerShape(GroupEnd),
        color = colors.surfaceBright,
        modifier = modifier.fillMaxWidth().semantics { role = Role.Switch },
    ) {
        Row(
            Modifier.heightIn(min = RowMinHeight).padding(RowPadding),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Symbol(icon, tint = colors.secondary)
            Column(Modifier.weight(1f)) {
                Text(
                    stringResource(title),
                    style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
                )
                Text(
                    stringResource(desc),
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

/** Samsung Air actions notice, shown only when they are on or unknown. */
@Composable
fun AirActionsCard(airActions: AirActions, modifier: Modifier = Modifier) {
    if (airActions != AirActions.On && airActions != AirActions.Unknown) return
    val colors = MaterialTheme.colorScheme
    Surface(shape = RoundedCornerShape(28.dp), color = colors.surfaceContainerHigh, modifier = modifier.fillMaxWidth().testTag("air_card")) {
        Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Symbol(R.drawable.ic_info, tint = colors.onSurfaceVariant)
            Text(
                stringResource(
                    if (airActions == AirActions.On) R.string.settings_air_actions_on else R.string.settings_air_actions_unknown,
                ),
                style = MaterialTheme.typography.bodySmall,
                color = colors.onSurfaceVariant,
            )
        }
    }
}

// ---------------------------------------------------------------- Key editor

/** Editor of the combination stored under [prefKey]; every change is saved at once. */
@OptIn(ExperimentalLayoutApi::class, ExperimentalMaterial3Api::class)
@Composable
fun KeyEditor(
    prefKey: String,
    state: SettingsState,
    actions: SettingsActions,
    modifier: Modifier = Modifier,
    previewMinHeight: Dp = 154.dp,
) {
    val colors = MaterialTheme.colorScheme
    val type = MaterialTheme.typography
    val combo = state.keys.getValue(prefKey)
    val preset = presetIndex(combo)
    Column(modifier.fillMaxWidth()) {
        Surface(shape = RoundedCornerShape(32.dp), color = colors.primaryContainer, modifier = Modifier.fillMaxWidth()) {
            Box(Modifier.fillMaxWidth().heightIn(min = previewMinHeight).padding(20.dp)) {
                Text(
                    stringResource(R.string.keys_current_combination),
                    Modifier.align(Alignment.TopStart),
                    style = type.labelMedium, fontWeight = FontWeight.Bold, color = colors.onPrimaryContainer,
                )
                // The top padding keeps wrapped keycaps clear of the label.
                KeyCaps(
                    combo,
                    Modifier.align(Alignment.Center).padding(top = 28.dp).fillMaxWidth().testTag("editor_preview"),
                    large = true,
                )
            }
        }

        EditorHeading(R.string.keys_presets_title)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            KeyPresets.forEachIndexed { i, p ->
                EditorChip(preset == i, { actions.setKeyCombo(prefKey, p.combo) }, stringResource(p.label))
            }
        }

        EditorHeading(R.string.keys_modifiers_title)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val mods = listOf(
                InputPrefs.MOD_CTRL to R.string.keys_mod_ctrl,
                InputPrefs.MOD_SHIFT to R.string.keys_mod_shift,
                InputPrefs.MOD_ALT to R.string.keys_mod_alt,
                InputPrefs.MOD_META to R.string.keys_mod_meta,
            )
            for ((bit, label) in mods) {
                EditorChip(combo.mods and bit != 0, { actions.toggleModifier(prefKey, bit) }, stringResource(label))
            }
        }

        EditorHeading(R.string.keys_key_title)
        KeyDropdown(combo.usage) { actions.setKey(prefKey, it) }
        Row(Modifier.padding(vertical = 14.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Symbol(R.drawable.ic_info, tint = colors.onSurfaceVariant, size = 20.dp)
            Text(stringResource(R.string.keys_note), style = type.bodySmall, color = colors.onSurfaceVariant)
        }
    }
}

/** A preset or modifier chip: 40 dp tall inside a 48 dp touch target, with a check when selected. */
@Composable
private fun EditorChip(selected: Boolean, onClick: () -> Unit, label: String) {
    FilterChip(
        selected = selected,
        onClick = onClick,
        label = { Text(label) },
        shape = RoundedCornerShape(8.dp),
        modifier = Modifier.heightIn(min = 40.dp),
        leadingIcon = if (selected) {
            { Symbol(R.drawable.ic_check, size = 18.dp) }
        } else {
            null
        },
        border = FilterChipDefaults.filterChipBorder(
            enabled = true, selected = selected, borderColor = MaterialTheme.colorScheme.outline,
        ),
    )
}

@Composable
private fun EditorHeading(res: Int) {
    Text(
        stringResource(res),
        Modifier.padding(top = 26.dp, bottom = 12.dp),
        style = MaterialTheme.typography.titleMedium,
        fontSize = 18.sp,
        fontWeight = FontWeight.Bold,
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun KeyDropdown(usage: Int, onSelect: (Int) -> Unit) {
    val colors = MaterialTheme.colorScheme
    val context = LocalContext.current
    var expanded by remember { mutableStateOf(false) }
    ExposedDropdownMenuBox(expanded, { expanded = it }) {
        Surface(
            shape = RoundedCornerShape(GroupEnd),
            color = colors.surfaceBright,
            modifier = Modifier.fillMaxWidth().menuAnchor(ExposedDropdownMenuAnchorType.PrimaryNotEditable)
                .testTag("key_selector"),
            onClick = { expanded = true },
        ) {
            Row(
                Modifier.heightIn(min = RowMinHeight).padding(horizontal = 16.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Symbol(R.drawable.ic_keyboard, tint = colors.secondary)
                Text(
                    stringResource(R.string.keys_key_title), Modifier.weight(1f),
                    style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold,
                )
                Text(
                    InputPrefs.keyLabel(context, usage), color = colors.onSurface,
                    style = MaterialTheme.typography.titleSmall, textAlign = TextAlign.End,
                )
                Symbol(R.drawable.ic_expand_more, tint = colors.onSurfaceVariant)
            }
        }
        ExposedDropdownMenu(expanded, { expanded = false }) {
            for (u in InputPrefs.KEY_USAGES) {
                DropdownMenuItem(
                    text = { Text(InputPrefs.keyLabel(context, u)) },
                    onClick = {
                        onSelect(u)
                        expanded = false
                    },
                    modifier = Modifier.testTag("key_$u"),
                )
            }
        }
    }
}

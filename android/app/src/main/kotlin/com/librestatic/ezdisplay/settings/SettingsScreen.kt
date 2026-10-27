package com.librestatic.ezdisplay.settings

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.IconButton
import androidx.compose.foundation.background
import androidx.compose.material3.LargeFlexibleTopAppBar
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MediumFlexibleTopAppBar
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.R

/** Settings sections; AirActions only exists while its card is shown (and is always last). */
enum class SettingsSection { Pen, Zoom, Touch, Connection, Display, AirActions }

private fun airCardShown(state: SettingsState) =
    state.airActions == AirActions.On || state.airActions == AirActions.Unknown

/** The settings screen backed by [store]. */
@Composable
fun SettingsScreen(store: SettingsStore) {
    val state by store.state
    SettingsContent(state, store)
}

private val ListDetailMinWidth = 600.dp
private val WideMinWidth = 1000.dp
private val PaneWidth = 480.dp

/** Padding below the last item of a scrolling page: the navigation bar inset plus 24 dp. */
@Composable
private fun Modifier.endOfPage(): Modifier =
    windowInsetsPadding(WindowInsets.navigationBars.only(WindowInsetsSides.Bottom)).padding(bottom = 24.dp)

/**
 * Stateless settings UI: Phone below 600 dp wide, ListDetail from 600 to 999 dp, Wide from 1000 dp. The width is read
 * from the constraints (not a window size class, which puts 841 and 1056 dp in the same bucket).
 */
@Composable
fun SettingsContent(state: SettingsState, actions: SettingsActions, modifier: Modifier = Modifier) {
    var openEditor by rememberSaveable { mutableStateOf<String?>(null) }
    var section by rememberSaveable { mutableStateOf(SettingsSection.Pen) }
    var paneEditor by rememberSaveable { mutableStateOf<String?>(null) }
    Surface(modifier.fillMaxSize(), color = MaterialTheme.colorScheme.surfaceContainer) {
        BoxWithConstraints(Modifier.fillMaxSize()) {
            when {
                maxWidth >= WideMinWidth -> WideLayout(state, actions, paneEditor) { paneEditor = it }
                maxWidth >= ListDetailMinWidth ->
                    ListDetailLayout(state, actions, section, { section = it }, openEditor) { openEditor = it }
                else -> PhoneLayout(state, actions, openEditor) { openEditor = it }
            }
        }
    }
    state.pairing?.let { PairDialog(it, actions) }
}

// ---------------------------------------------------------------- Shared content

/** Profile cards plus the shortcut row (graphics tablet) or the gesture list (Custom). */
@Composable
private fun PenContent(
    state: SettingsState,
    actions: SettingsActions,
    columns: Int,
    onOpenEditor: (String) -> Unit,
    modifier: Modifier = Modifier,
    unselectedHeight: Dp = if (columns == 3) 182.dp else 86.dp,
    selectedHeight: Dp = if (columns == 3) 182.dp else 116.dp,
    selectedEditor: String? = null,
    shortcutMinHeight: Dp = 72.dp,
) {
    Column(modifier.fillMaxWidth()) {
        ProfileCards(state.profile, actions::setProfile, columns, Modifier, unselectedHeight, selectedHeight)
        when (state.profile) {
            "graphics_tablet" -> for (key in visibleKeyEditors(state)) {
                ShortcutRow(
                    key, state.keys.getValue(key),
                    { onOpenEditor(key) },
                    Modifier.padding(top = profileGap(columns)), highlighted = selectedEditor == key,
                    minHeight = shortcutMinHeight,
                )
            }
            "custom" -> CustomGestures(state, actions, onOpenEditor, Modifier.padding(top = profileGap(columns)), selectedEditor, shortcutMinHeight)
        }
    }
}

private fun editorTitle(prefKey: String) = KeyEditors.first { it.prefKey == prefKey }.title

// ---------------------------------------------------------------- Phone

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun PhoneLayout(
    state: SettingsState,
    actions: SettingsActions,
    openEditor: String?,
    setOpenEditor: (String?) -> Unit,
) {
    val editor = openEditor?.takeIf { it in visibleKeyEditors(state) }
    BackHandler(enabled = editor != null) { setOpenEditor(null) }
    AnimatedContent(editor, label = "phone-editor") { target ->
        if (target == null) PhoneMain(state, actions, setOpenEditor) else PhoneEditor(target, state, actions) { setOpenEditor(null) }
    }
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun PhoneMain(state: SettingsState, actions: SettingsActions, onOpenEditor: (String) -> Unit) {
    val colors = MaterialTheme.colorScheme
    val scroll = TopAppBarDefaults.exitUntilCollapsedScrollBehavior()
    Scaffold(
        modifier = Modifier.nestedScroll(scroll.nestedScrollConnection),
        containerColor = colors.surfaceContainer,
        contentWindowInsets = WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal + WindowInsetsSides.Top),
        topBar = {
            LargeFlexibleTopAppBar(
                title = { Text(stringResource(R.string.settings_screen_title), fontWeight = FontWeight.ExtraBold) },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = colors.surfaceContainer,
                    scrolledContainerColor = colors.surfaceContainer,
                ),
                scrollBehavior = scroll,
            )
        },
    ) { padding ->
        Column(
            Modifier.padding(padding).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).endOfPage(),
        ) {
            SectionTitle(stringResource(R.string.settings_profile))
            PenContent(state, actions, 2, onOpenEditor)
            SectionTitle(stringResource(R.string.section_zoom))
            ZoomGroup(state, actions)
            SectionTitle(stringResource(R.string.section_touch))
            PalmRow(state.palmRejection, actions::setPalmRejection)
            AirActionsCard(state.airActions, Modifier.padding(top = profileGap(2)))
            SectionTitle(stringResource(R.string.section_connection))
            ConnectionContent(state, actions)
            SectionTitle(stringResource(R.string.section_display))
            DisplayContent(state, actions)
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun PhoneEditor(prefKey: String, state: SettingsState, actions: SettingsActions, onBack: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    val scroll = TopAppBarDefaults.exitUntilCollapsedScrollBehavior()
    Scaffold(
        modifier = Modifier.nestedScroll(scroll.nestedScrollConnection),
        containerColor = colors.surfaceContainer,
        contentWindowInsets = WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal + WindowInsetsSides.Top),
        topBar = {
            MediumFlexibleTopAppBar(
                title = { Text(stringResource(editorTitle(prefKey)), fontWeight = FontWeight.ExtraBold) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Symbol(R.drawable.ic_arrow_back, contentDescription = stringResource(R.string.cd_back))
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = colors.surfaceContainer,
                    scrolledContainerColor = colors.surfaceContainer,
                ),
                scrollBehavior = scroll,
            )
        },
    ) { padding ->
        Column(Modifier.padding(padding).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).endOfPage()) {
            KeyEditor(prefKey, state, actions)
        }
    }
}

// ---------------------------------------------------------------- List + detail

@Composable
private fun ListDetailLayout(
    state: SettingsState,
    actions: SettingsActions,
    selected: SettingsSection,
    onSelect: (SettingsSection) -> Unit,
    openEditor: String?,
    setOpenEditor: (String?) -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    val sections = buildList {
        add(SettingsSection.Pen); add(SettingsSection.Zoom); add(SettingsSection.Touch); add(SettingsSection.Connection); add(SettingsSection.Display)
        if (airCardShown(state)) add(SettingsSection.AirActions)
    }
    val current = selected.takeIf { it in sections } ?: SettingsSection.Pen
    val editor = openEditor?.takeIf { it in visibleKeyEditors(state) }
    BackHandler(enabled = editor != null) { setOpenEditor(null) }
    Row(Modifier.fillMaxSize()) {
        Column(
            Modifier.width(300.dp).fillMaxHeight().background(colors.surfaceContainerLow)
                .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Bottom + WindowInsetsSides.Start))
                .verticalScroll(rememberScrollState())
                .padding(start = 16.dp, end = 12.dp, top = 22.dp, bottom = 20.dp),
        ) {
            Text(
                stringResource(R.string.settings_screen_title),
                Modifier.padding(start = 8.dp, top = 8.dp, bottom = 24.dp),
                style = MaterialTheme.typography.displaySmall,
                fontWeight = FontWeight.ExtraBold,
            )
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                sections.forEachIndexed { i, s ->
                    SectionListItem(s, state, s == current, i, sections.size) {
                        onSelect(s)
                        setOpenEditor(null)
                    }
                }
            }
        }
        Column(
            Modifier.weight(1f).fillMaxHeight()
                .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Bottom + WindowInsetsSides.End))
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 22.dp, vertical = 26.dp),
        ) {
            if (editor != null) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    IconButton(onClick = { setOpenEditor(null) }) {
                        Symbol(R.drawable.ic_arrow_back, contentDescription = stringResource(R.string.cd_back))
                    }
                    Text(
                        stringResource(editorTitle(editor)),
                        style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.ExtraBold,
                    )
                }
                Spacer(Modifier.height(16.dp))
                KeyEditor(editor, state, actions, previewMinHeight = 190.dp)
            } else {
                Text(
                    stringResource(sectionTitle(current)),
                    Modifier.padding(bottom = 16.dp),
                    style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.ExtraBold,
                )
                when (current) {
                    SettingsSection.Pen -> PenContent(
                        state, actions, 2, setOpenEditor,
                        unselectedHeight = 112.dp, selectedHeight = 132.dp, shortcutMinHeight = 88.dp,
                    )
                    SettingsSection.Zoom -> ZoomGroup(state, actions)
                    SettingsSection.Touch -> PalmRow(state.palmRejection, actions::setPalmRejection)
                    SettingsSection.Connection -> ConnectionContent(state, actions)
                    SettingsSection.Display -> DisplayContent(state, actions)
                    SettingsSection.AirActions -> AirActionsCard(state.airActions)
                }
            }
        }
    }
}

private fun sectionTitle(s: SettingsSection) = when (s) {
    SettingsSection.Pen -> R.string.settings_profile
    SettingsSection.Zoom -> R.string.section_zoom
    SettingsSection.Touch -> R.string.settings_palm_title
    SettingsSection.Connection -> R.string.section_connection
    SettingsSection.Display -> R.string.section_display
    SettingsSection.AirActions -> R.string.section_air_actions
}

private fun sectionIcon(s: SettingsSection) = when (s) {
    SettingsSection.Pen -> R.drawable.ic_stylus
    SettingsSection.Zoom -> R.drawable.ic_zoom_in
    SettingsSection.Touch -> R.drawable.ic_back_hand
    SettingsSection.Connection -> R.drawable.ic_wifi
    SettingsSection.Display -> R.drawable.ic_bedtime
    SettingsSection.AirActions -> R.drawable.ic_info
}

@Composable
private fun sectionValue(s: SettingsSection, state: SettingsState): String = when (s) {
    SettingsSection.Pen -> stringResource(Profiles.firstOrNull { it.value == state.profile }?.title ?: R.string.profile_raw)
    SettingsSection.Zoom -> stringResource(R.string.settings_zoom_summary, formatZoom(state.zoomFactor), state.doubleClickMs)
    SettingsSection.Touch -> stringResource(if (state.palmRejection) R.string.settings_on else R.string.settings_off)
    SettingsSection.Connection -> connectionSummary(state)
    SettingsSection.Display -> displaySummary(state)
    SettingsSection.AirActions -> stringResource(R.string.section_air_actions_value)
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun SectionListItem(
    section: SettingsSection,
    state: SettingsState,
    selected: Boolean,
    index: Int,
    count: Int,
    onClick: () -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    SegmentedListItem(
        selected = selected,
        onClick = onClick,
        shapes = ListItemDefaults.shapes(shape = segmentShape(index, count), selectedShape = RoundedCornerShape(28.dp)),
        modifier = Modifier.heightIn(min = if (selected) 102.dp else 90.dp).testTag("section_${section.name}"),
        verticalAlignment = Alignment.CenterVertically,
        colors = ListItemDefaults.segmentedColors(
            containerColor = colors.surfaceBright,
            selectedContainerColor = colors.secondaryContainer,
            selectedContentColor = colors.onSecondaryContainer,
            selectedSupportingContentColor = colors.onSecondaryContainer,
            selectedLeadingContentColor = colors.onSecondary,
            selectedTrailingContentColor = colors.onSecondaryContainer,
        ),
        leadingContent = {
            Surface(
                shape = RoundedCornerShape(16.dp),
                color = if (selected) colors.onSecondaryContainer else colors.surfaceContainerHighest,
                modifier = Modifier.size(40.dp),
            ) {
                Box(contentAlignment = Alignment.Center) { Symbol(sectionIcon(section)) }
            }
        },
        trailingContent = { Symbol(R.drawable.ic_chevron_right, size = 20.dp) },
        supportingContent = { Text(sectionValue(section, state), maxLines = 1, overflow = TextOverflow.Ellipsis) },
    ) {
        Text(stringResource(sectionTitle(section)), fontWeight = FontWeight.Bold)
    }
}

// ---------------------------------------------------------------- Wide

/** Page width from which the zoom and touch groups sit side by side and the profile grid has three columns. */
private val TwoColumnPageWidth = 900.dp

@Composable
private fun WideHeading(text: String) {
    Text(
        text,
        Modifier.padding(bottom = 12.dp),
        style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold,
    )
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun WideLayout(
    state: SettingsState,
    actions: SettingsActions,
    paneEditor: String?,
    setPaneEditor: (String?) -> Unit,
) {
    val colors = MaterialTheme.colorScheme
    val visible = visibleKeyEditors(state)
    val editor = paneEditor?.takeIf { it in visible } ?: visible.firstOrNull()
    val pageSides = WindowInsetsSides.Top + WindowInsetsSides.Start +
        if (editor == null) WindowInsetsSides.End else WindowInsetsSides.Top
    Row(Modifier.fillMaxSize()) {
        Column(
            Modifier.weight(1f).fillMaxHeight()
                .windowInsetsPadding(WindowInsets.safeDrawing.only(pageSides))
                .verticalScroll(rememberScrollState())
                .testTag("wide_page")
                .padding(start = 36.dp, end = 36.dp, top = 20.dp)
                .endOfPage(),
        ) {
            var displaySheet by rememberSaveable { mutableStateOf(false) }
            // The Display section sits behind a button on the title row, so the page gains no height.
            Row(Modifier.fillMaxWidth().padding(bottom = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(
                    stringResource(R.string.settings_screen_title),
                    Modifier.weight(1f),
                    style = MaterialTheme.typography.displayMedium, fontWeight = FontWeight.ExtraBold,
                )
                DisplaySheetButton(state, { displaySheet = true })
            }
            if (displaySheet) DisplaySheet(state, actions) { displaySheet = false }
            BoxWithConstraints(Modifier.fillMaxWidth()) {
                val sideBySide = maxWidth >= TwoColumnPageWidth
                Column(Modifier.fillMaxWidth()) {
                    WideHeading(stringResource(R.string.settings_profile))
                    PenContent(
                        state, actions, if (sideBySide) 3 else 2, setPaneEditor,
                        selectedEditor = editor, shortcutMinHeight = 64.dp,
                    )
                    Spacer(Modifier.height(20.dp))
                    if (sideBySide) {
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(profileGap(3))) {
                            Column(Modifier.weight(1f)) {
                                WideHeading(stringResource(R.string.section_zoom))
                                ZoomGroup(state, actions)
                            }
                            Column(Modifier.weight(1f)) {
                                WideHeading(stringResource(R.string.section_touch))
                                PalmRow(state.palmRejection, actions::setPalmRejection)
                                AirActionsCard(state.airActions, Modifier.padding(top = profileGap(3)))
                            }
                            Column(Modifier.weight(1f)) {
                                WideHeading(stringResource(R.string.section_connection))
                                ConnectionSummary(state, actions)
                            }
                        }
                    } else {
                        WideHeading(stringResource(R.string.section_zoom))
                        ZoomGroup(state, actions)
                        Spacer(Modifier.height(20.dp))
                        WideHeading(stringResource(R.string.section_touch))
                        PalmRow(state.palmRejection, actions::setPalmRejection)
                        AirActionsCard(state.airActions, Modifier.padding(top = profileGap(2)))
                        Spacer(Modifier.height(20.dp))
                        WideHeading(stringResource(R.string.section_connection))
                        ConnectionSummary(state, actions)
                    }
                }
            }
        }
        if (editor != null) {
            Column(
                Modifier.width(PaneWidth).fillMaxHeight().background(colors.surfaceContainerLow)
                    .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.End))
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 24.dp, vertical = 28.dp)
                    .endOfPage()
                    .testTag("editor_pane"),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Surface(shape = RoundedCornerShape(20.dp), color = colors.secondaryContainer, modifier = Modifier.size(48.dp)) {
                        Box(contentAlignment = Alignment.Center) {
                            Symbol(R.drawable.ic_keyboard, tint = colors.onSecondaryContainer)
                        }
                    }
                    Text(
                        stringResource(editorTitle(editor)),
                        style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold,
                    )
                }
                Spacer(Modifier.height(22.dp))
                KeyEditor(editor, state, actions, previewMinHeight = 190.dp)
            }
        }
    }
}

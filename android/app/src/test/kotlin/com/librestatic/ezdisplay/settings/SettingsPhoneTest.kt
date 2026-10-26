package com.librestatic.ezdisplay.settings

import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performSemanticsAction
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.R
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

@Config(sdk = [36], qualifiers = "w412dp-h915dp")
class SettingsPhoneTest : SettingsUiTestBase() {
    @Test fun layoutIsSingleColumn() {
        launch(AirActions.On) { putString(InputPrefs.PROFILE, "graphics_tablet") }
        dumpTree("phone-main")
        rule.onNodeWithText(str(R.string.settings_screen_title)).assertExists()
        rule.onNodeWithText(str(R.string.profile_desktop)).assertExists()
        tag("shortcut_keys_hold").assertExists()
        tag("shortcut_keys_double_hold").assertExists()
        tag("section_Pen").assertDoesNotExist()
        tag("editor_pane").assertDoesNotExist()
        rule.onNodeWithText(str(R.string.section_zoom)).assertExists()
        rule.onNodeWithText(str(R.string.settings_air_actions_on)).performScrollTo().assertExists()
    }

    @Test fun airActionsCardOnlyWhenOnOrUnknown() {
        launch(AirActions.Off)
        rule.onNodeWithText(str(R.string.settings_air_actions_on)).assertDoesNotExist()
        rule.onNodeWithText(str(R.string.settings_air_actions_unknown)).assertDoesNotExist()
    }

    @Test fun profileCardSavesProfile() {
        launch()
        rule.onNodeWithText(str(R.string.profile_desktop)).performClick()
        assertEquals("desktop", prefs.getString(InputPrefs.PROFILE, null))
        assertEquals("desktop", store.state.value.profile)
    }

    @Test fun selectedProfileShowsDescriptionAndMarker() {
        launch { putString(InputPrefs.PROFILE, "drawing") }
        rule.onNodeWithText(str(R.string.profile_drawing_desc)).assertExists()
        rule.onNodeWithText(str(R.string.profile_selected)).assertExists()
        rule.onNodeWithText(str(R.string.profile_raw_desc)).assertDoesNotExist()
    }

    @Test fun graphicsTabletShowsHoldShortcutRow() {
        launch { putString(InputPrefs.PROFILE, "graphics_tablet") }
        tag("shortcut_keys_hold").assertExists()
        tag("trigger_custom_click").assertDoesNotExist()
    }

    @Test fun shortcutRowIsAtMost88dpTall() {
        launch { putString(InputPrefs.PROFILE, "graphics_tablet") }
        val px = tag("shortcut_keys_hold").fetchSemanticsNode().size.height
        val dp = px / rule.activity.resources.displayMetrics.density
        println("PHONE shortcut row height: ${dp}dp")
        assertTrue("shortcut row is ${dp}dp tall", dp <= 88f)
    }

    @Test fun presetProfilesShowNeitherShortcutNorGestures() {
        launch { putString(InputPrefs.PROFILE, "desktop") }
        tag("shortcut_keys_hold").assertDoesNotExist()
        tag("trigger_custom_click").assertDoesNotExist()
    }

    @Test fun customShowsGestures() {
        launch { putString(InputPrefs.PROFILE, "custom") }
        for (t in Triggers) tag("trigger_${t.key}").performScrollTo().assertExists()
        tag("shortcut_keys_hold").assertDoesNotExist()
    }

    @Test fun triggerMenuSavesAction() {
        launch { putString(InputPrefs.PROFILE, "custom") }
        tag("trigger_${InputPrefs.CUSTOM_HOLD}").performScrollTo().performClick()
        tag("action_eraser").performClick()
        assertEquals("eraser", prefs.getString(InputPrefs.CUSTOM_HOLD, null))
        rule.onNodeWithText(str(R.string.action_eraser)).assertExists()
    }

    @Test fun triggerMenuOffersOnlyAllowedActions() {
        launch { putString(InputPrefs.PROFILE, "custom") }
        tag("trigger_${InputPrefs.CUSTOM_CLICK}").performScrollTo().performClick()
        tag("action_right_click").assertExists()
        tag("action_eraser").assertDoesNotExist()
    }

    @Test fun customKeysActionAddsShortcutLine() {
        launch {
            putString(InputPrefs.PROFILE, "custom")
            putString(InputPrefs.CUSTOM_CLICK, "keys")
            putString(InputPrefs.CUSTOM_HOLD, "keys")
        }
        tag("shortcut_keys_click").assertExists()
        tag("shortcut_keys_hold").assertExists()
        tag("shortcut_keys_double_click").assertDoesNotExist()
    }

    @Test fun holdKeysDisablesDragsAndShowsNote() {
        launch {
            putString(InputPrefs.PROFILE, "custom")
            putString(InputPrefs.CUSTOM_HOLD, "keys")
        }
        tag("trigger_${InputPrefs.CUSTOM_HOLD_DRAG_CONTACT}").performScrollTo().assertIsNotEnabled()
        tag("trigger_${InputPrefs.CUSTOM_HOLD_DRAG_HOVER}").performScrollTo().assertIsNotEnabled()
        tag("trigger_${InputPrefs.CUSTOM_HOLD}").assertIsEnabled()
        rule.onNodeWithText(str(R.string.keys_drag_unused)).assertExists()
    }

    @Test fun dragsEnabledWithoutHoldKeys() {
        launch { putString(InputPrefs.PROFILE, "custom") }
        tag("trigger_${InputPrefs.CUSTOM_HOLD_DRAG_CONTACT}").performScrollTo().assertIsEnabled()
        rule.onNodeWithText(str(R.string.keys_drag_unused)).assertDoesNotExist()
    }

    @Test fun shortcutOpensEditorScreenAndBackReturns() {
        launch { putString(InputPrefs.PROFILE, "graphics_tablet") }
        tag("shortcut_keys_hold").performClick()
        dumpTree("phone-editor")
        rule.onNodeWithText(str(R.string.keys_current_combination)).assertExists()
        rule.onNodeWithText(str(R.string.profile_desktop)).assertDoesNotExist()
        back()
        rule.onNodeWithText(str(R.string.profile_desktop)).assertExists()
        rule.onNodeWithText(str(R.string.keys_current_combination)).assertDoesNotExist()
    }

    @Test fun topBarArrowReturnsFromEditor() {
        launch { putString(InputPrefs.PROFILE, "graphics_tablet") }
        tag("shortcut_keys_hold").performClick()
        rule.onNodeWithContentDescription(str(R.string.cd_back)).performClick()
        rule.onNodeWithText(str(R.string.profile_desktop)).assertExists()
    }

    private fun openHoldEditor(combo: String) {
        launch {
            putString(InputPrefs.PROFILE, "graphics_tablet")
            putString(InputPrefs.KEYS_HOLD, combo)
        }
        tag("shortcut_keys_hold").performClick()
    }

    @Test fun presetChipSavesCombo() {
        openHoldEditor("0:44")
        clickable(str(R.string.keys_preset_ctrl_space)).performClick()
        assertEquals("1:44", prefs.getString(InputPrefs.KEYS_HOLD, null))
    }

    @Test fun modifierChipFlipsBit() {
        openHoldEditor("1:44")
        clickable(str(R.string.keys_mod_shift)).performScrollTo().performClick()
        assertEquals("3:44", prefs.getString(InputPrefs.KEYS_HOLD, null))
        clickable(str(R.string.keys_mod_ctrl)).performClick()
        assertEquals("2:44", prefs.getString(InputPrefs.KEYS_HOLD, null))
    }

    /** Chips are 40 dp inside a 48 dp touch target, which leaves 8 dp between wrapped rows (a fixed height drops it). */
    @Test @GraphicsMode(GraphicsMode.Mode.NATIVE) // real text widths, so the presets wrap
    fun wrappedPresetChipRowsDoNotTouch() {
        openHoldEditor("1:44")
        val density = rule.activity.resources.displayMetrics.density
        val rows = listOf(
            // Not the "Space" preset: the key menu shows the same text.
            R.string.keys_preset_ctrl_space, R.string.keys_preset_alt_space,
            R.string.keys_preset_ctrl_z, R.string.keys_preset_ctrl_shift_z,
        ).map { clickable(str(it)).fetchSemanticsNode().boundsInRoot }.groupBy { it.top }.toSortedMap().values.toList()
        assertTrue("presets wrap at 412 dp: $rows", rows.size >= 2)
        val gap = (rows[1].first().top - rows[0].first().bottom) / density
        println("PHONE preset chip row gap: ${gap}dp")
        assertTrue("gap between chip rows is ${gap}dp", gap >= 7.5f)
    }

    @Test fun keyMenuChangesUsage() {
        openHoldEditor("1:44")
        tag("key_selector").performScrollTo().performClick()
        tag("key_4").performClick()
        assertEquals("1:4", prefs.getString(InputPrefs.KEYS_HOLD, null))
    }

    @Test fun editorPreviewFollowsCombo() {
        openHoldEditor("1:44")
        val ctrlKeycaps = hasText("Ctrl") and !hasClickAction()
        rule.onAllNodes(ctrlKeycaps).assertCountEquals(1)
        clickable(str(R.string.keys_mod_ctrl)).performScrollTo().performClick()
        rule.onAllNodes(ctrlKeycaps).assertCountEquals(0)
        assertEquals("0:44", prefs.getString(InputPrefs.KEYS_HOLD, null))
    }

    @Test fun zoomButtonsSaveFactor() {
        launch()
        rule.onNodeWithText("3×").performScrollTo().performClick()
        assertEquals(3f, prefs.getFloat(InputPrefs.ZOOM_FACTOR, 0f), 0f)
        rule.onNodeWithText("1.5×").performClick()
        assertEquals(1.5f, prefs.getFloat(InputPrefs.ZOOM_FACTOR, 0f), 0f)
    }

    @Test fun sliderSavesRoundedMs() {
        launch()
        tag("double_click_slider").performScrollTo()
            .performSemanticsAction(SemanticsActions.SetProgress) { it(447f) }
        rule.waitForIdle()
        assertEquals(450, prefs.getInt(InputPrefs.DOUBLE_CLICK_MS, 0))
        rule.onNodeWithText("450 ms").assertExists()
    }

    @Test fun sliderClampsToRange() {
        launch()
        tag("double_click_slider").performScrollTo()
            .performSemanticsAction(SemanticsActions.SetProgress) { it(5000f) }
        rule.waitForIdle()
        assertEquals(800, prefs.getInt(InputPrefs.DOUBLE_CLICK_MS, 0))
    }

    @Test fun audioRowTogglesAudio() {
        launch()
        rule.onNodeWithText(str(R.string.settings_audio_desc)).performScrollTo().performClick()
        assertEquals(false, prefs.getBoolean(SettingsStore.AUDIO_ENABLED, true))
        rule.onNodeWithText(str(R.string.settings_audio_desc)).performClick()
        assertEquals(true, prefs.getBoolean(SettingsStore.AUDIO_ENABLED, false))
    }

    @Test fun palmRowTogglesRejection() {
        launch()
        rule.onNodeWithText(str(R.string.settings_palm_title)).performScrollTo().performClick()
        assertEquals(false, prefs.getBoolean(InputPrefs.PALM_REJECTION, true))
        rule.onNodeWithText(str(R.string.settings_palm_title)).performClick()
        assertEquals(true, prefs.getBoolean(InputPrefs.PALM_REJECTION, false))
    }
}

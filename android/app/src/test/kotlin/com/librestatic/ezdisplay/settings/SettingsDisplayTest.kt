package com.librestatic.ezdisplay.settings

import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.assertIsSelected
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performSemanticsAction
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.R
import com.librestatic.ezdisplay.display.PanelType
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.robolectric.annotation.Config

/** The Display section in the three layouts. */
class SettingsDisplayTest : SettingsUiTestBase() {
    private fun checkContent(previewBox: Boolean = true) {
        tag("screensaver_row").performScrollTo().assertExists()
        tag("screensaver_idle_slider").assertExists()
        tag("screensaver_brightness_slider").assertExists()
        tag("screensaver_blank_slider").assertExists()
        if (previewBox) tag("screensaver_preview_box").assertExists() else tag("screensaver_preview_box").assertDoesNotExist()
        tag("screensaver_preview").assertExists()
        tag("replay_onboarding").assertExists()
    }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp")
    fun phoneShowsTheDisplaySection() {
        launch(panelType = PanelType.Oled)
        checkContent()
        rule.onNodeWithText(str(R.string.screensaver_detected, str(R.string.panel_oled))).assertExists()
        rule.onNodeWithText(str(R.string.section_display)).assertExists()
    }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp")
    fun detectedScreenText() {
        launch(panelType = PanelType.Lcd)
        rule.onNodeWithText(str(R.string.screensaver_detected, str(R.string.panel_lcd))).performScrollTo().assertExists()
    }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp")
    fun switchSaves() {
        launch(panelType = PanelType.Oled)
        tag("screensaver_row").performScrollTo()
        assertFalse(prefs.contains(InputPrefs.SCREENSAVER_ENABLED))
        tag("screensaver_row").performClick()
        assertEquals(false, prefs.getBoolean(InputPrefs.SCREENSAVER_ENABLED, true))
        tag("screensaver_row").performClick()
        assertTrue(prefs.getBoolean(InputPrefs.SCREENSAVER_ENABLED, false))
    }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp")
    fun slidersSaveRoundedValues() {
        launch { putBoolean(InputPrefs.SCREENSAVER_ENABLED, true) }
        tag("screensaver_idle_slider").performScrollTo().performSemanticsAction(SemanticsActions.SetProgress) { it(12f) }
        tag("screensaver_brightness_slider").performScrollTo().performSemanticsAction(SemanticsActions.SetProgress) { it(300f) }
        tag("screensaver_blank_slider").performScrollTo().performSemanticsAction(SemanticsActions.SetProgress) { it(47f) }
        rule.waitForIdle()
        assertEquals(12, prefs.getInt(InputPrefs.SCREENSAVER_IDLE_MIN, 0))
        assertEquals(30, prefs.getInt(InputPrefs.SCREENSAVER_BRIGHTNESS_PCT, 0))
        assertEquals(45, prefs.getInt(InputPrefs.SCREENSAVER_BLANK_MIN, 0))
        // The value label and the slider's maximum label.
        assertEquals(2, rule.onAllNodesWithText("30%").fetchSemanticsNodes().size)
    }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp")
    fun blankSliderShowsNever() {
        launch { putBoolean(InputPrefs.SCREENSAVER_ENABLED, true) }
        tag("screensaver_blank_slider").performScrollTo().performSemanticsAction(SemanticsActions.SetProgress) { it(0f) }
        rule.waitForIdle()
        assertEquals(0, prefs.getInt(InputPrefs.SCREENSAVER_BLANK_MIN, -1))
        // The value label and the slider's minimum label.
        assertEquals(2, rule.onAllNodesWithText(str(R.string.screensaver_blank_never)).fetchSemanticsNodes().size)
    }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp")
    fun minutesArePluralised() {
        launch { putInt(InputPrefs.SCREENSAVER_IDLE_MIN, 1) }
        // The value label and the slider's minimum label.
        val one = rule.activity.resources.getQuantityString(R.plurals.settings_minutes, 1, 1)
        assertEquals("1 minute", one)
        assertEquals(2, rule.onAllNodesWithText(one).fetchSemanticsNodes().size)
    }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp")
    fun previewAndReplayCallTheActions() {
        var preview = 0
        var replay = 0
        val actions = object : SettingsActions by (SettingsStore(prefs, AirActions.Off)) {
            override fun previewScreensaver() { preview++ }
            override fun replayOnboarding() { replay++ }
        }
        rule.setContent { SettingsTheme { SettingsContent(SettingsState(), actions) } }
        tag("screensaver_preview").performScrollTo().performClick()
        tag("replay_onboarding").performScrollTo().performClick()
        assertEquals(1, preview)
        assertEquals(1, replay)
    }

    @Test @Config(sdk = [36], qualifiers = "w841dp-h701dp")
    fun listDetailHasADisplaySection() {
        launch(panelType = PanelType.Oled)
        tag("section_Display").assertExists()
        tag("section_Display").performClick()
        tag("section_Display").assertIsSelected()
        checkContent()
        rule.onNodeWithText(str(R.string.profile_desktop)).assertDoesNotExist()
    }

    @Test @Config(sdk = [36], qualifiers = "w841dp-h701dp")
    fun listDetailSummary() {
        launch(panelType = PanelType.Oled)
        tag("section_Display").performClick()
        tag("section_Display").assertTextContains(rule.activity.resources.getQuantityString(R.plurals.settings_minutes, 5, 5))
        tag("screensaver_row").performClick()
        tag("section_Display").assertTextContains(str(R.string.screensaver_off))
    }

    @Test @Config(sdk = [36], qualifiers = "w1691dp-h978dp-280dpi")
    fun wideOpensTheDisplaySectionInASheet() {
        launch(panelType = PanelType.Oled)
        tag("display_sheet").assertDoesNotExist()
        tag("section_Display").assertDoesNotExist()
        tag("screensaver_button").assertTextContains(
            str(R.string.screensaver_title) + " · " + rule.activity.resources.getQuantityString(R.plurals.settings_minutes, 5, 5),
        )
        tag("screensaver_button").performClick()
        tag("display_sheet").assertExists()
        checkContent()
        tag("screensaver_row").performClick()
        assertEquals(false, prefs.getBoolean(InputPrefs.SCREENSAVER_ENABLED, true))
        tag("screensaver_button").assertTextContains(str(R.string.screensaver_off))
    }

    @Test @Config(sdk = [36], qualifiers = "w1056dp-h1691dp-280dpi")
    fun wideWithThePaneAlsoUsesTheSheet() {
        launch(panelType = PanelType.Oled) { putString(InputPrefs.PROFILE, "graphics_tablet") }
        tag("screensaver_button").performClick()
        checkContent()
    }
}

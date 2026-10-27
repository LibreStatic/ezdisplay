package com.librestatic.ezdisplay.settings

import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.assertIsSelected
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.R
import org.junit.Assert.assertEquals
import org.junit.Test
import org.robolectric.annotation.Config

@Config(sdk = [36], qualifiers = "w841dp-h701dp")
class SettingsListDetailTest : SettingsUiTestBase() {
    @Test fun showsSectionListAndPenDetail() {
        launch(AirActions.On) { putString(InputPrefs.PROFILE, "graphics_tablet") }
        dumpTree("list-detail")
        tag("section_Pen").assertExists().assertIsSelected()
        tag("section_Zoom").assertExists()
        tag("section_Touch").assertExists()
        tag("section_AirActions").assertExists()
        rule.onNodeWithText(str(R.string.profile_desktop)).assertExists()
        tag("shortcut_keys_hold").assertExists()
    }

    @Test fun airActionsSectionOnlyWhenCardWouldShow() {
        launch(AirActions.Off)
        tag("section_AirActions").assertDoesNotExist()
    }

    @Test fun sectionValuesSummarizeState() {
        launch { putString(InputPrefs.PROFILE, "drawing") }
        rule.onNodeWithText("2× · 300 ms").assertExists()
        // Touch (palm rejection) and Audio are both on by default.
        rule.onAllNodesWithText(str(R.string.settings_on)).assertCountEquals(2)
    }

    @Test fun selectingZoomShowsZoomContent() {
        launch()
        tag("section_Zoom").performClick()
        tag("section_Zoom").assertIsSelected()
        rule.onNodeWithText("3×").assertExists()
        tag("double_click_slider").assertExists()
        rule.onNodeWithText(str(R.string.profile_desktop)).assertDoesNotExist()
    }

    @Test fun selectingTouchShowsPalmRow() {
        launch()
        tag("section_Touch").performClick()
        rule.onNodeWithText(str(R.string.settings_palm_desc)).assertExists()
        rule.onNodeWithText(str(R.string.settings_palm_desc)).performClick()
        assertEquals(false, prefs.getBoolean(InputPrefs.PALM_REJECTION, true))
        rule.onNodeWithText(str(R.string.settings_off)).assertExists()
    }

    @Test fun profileCardSavesInDetail() {
        launch()
        rule.onNodeWithText(str(R.string.profile_drawing)).performClick()
        assertEquals("drawing", prefs.getString(InputPrefs.PROFILE, null))
    }

    @Test fun editorReplacesDetailAndBackCloses() {
        launch { putString(InputPrefs.PROFILE, "graphics_tablet") }
        tag("shortcut_keys_hold").performScrollTo().performClick()
        rule.onNodeWithText(str(R.string.keys_current_combination)).assertExists()
        rule.onNodeWithText(str(R.string.profile_desktop)).assertDoesNotExist()
        tag("section_Pen").assertExists()
        back()
        rule.onNodeWithText(str(R.string.profile_desktop)).assertExists()
    }
}

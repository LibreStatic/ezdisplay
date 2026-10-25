package com.librestatic.ezdisplay.settings

import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.R
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.robolectric.annotation.Config

@Config(sdk = [36], qualifiers = "w1691dp-h978dp-280dpi")
class SettingsWideTest : SettingsUiTestBase() {
    private fun bounds(tag: String): Rect = tag(tag).fetchSemanticsNode().boundsInRoot

    private fun profileCard(title: Int): Rect =
        rule.onNode(clickableMatcher(str(title))).fetchSemanticsNode().boundsInRoot

    @Test fun showsPageAndEditorPaneWithoutRail() {
        launch(AirActions.Unknown) { putString(InputPrefs.PROFILE, "graphics_tablet") }
        dumpTree("wide")
        tag("wide_page").assertExists()
        tag("editor_pane").assertExists()
        tag("section_Pen").assertDoesNotExist()
        listOf("rail_Pen", "rail_Zoom", "rail_Touch").forEach { tag(it).assertDoesNotExist() }
        rule.onNode(inPane(str(R.string.keys_title_hold))).assertExists()
        rule.onNodeWithText(str(R.string.profile_desktop)).assertExists()
        rule.onNodeWithText(str(R.string.settings_air_actions_unknown)).assertExists()
        tag("double_click_slider").assertExists()
    }

    @Test fun paneIsShownForGraphicsTabletAndHiddenForRaw() {
        launch { putString(InputPrefs.PROFILE, "graphics_tablet") }
        tag("editor_pane").assertExists()
        rule.runOnUiThread { store.setProfile("raw") }
        rule.waitForIdle()
        tag("editor_pane").assertDoesNotExist()
    }

    @Test fun paneDisappearsWithoutKeyEditors() {
        launch { putString(InputPrefs.PROFILE, "graphics_tablet") }
        tag("editor_pane").assertExists()
        rule.onNodeWithText(str(R.string.profile_desktop)).performClick()
        assertEquals("desktop", prefs.getString(InputPrefs.PROFILE, null))
        tag("editor_pane").assertDoesNotExist()
        tag("shortcut_keys_hold").assertDoesNotExist()
    }

    @Test fun paneAppearsWhenSwitchingToGraphicsTablet() {
        launch()
        tag("editor_pane").assertDoesNotExist()
        rule.onNodeWithText(str(R.string.profile_graphics_tablet)).performClick()
        tag("editor_pane").assertExists()
    }

    @Test fun customPaneFollowsTappedShortcut() {
        launch {
            putString(InputPrefs.PROFILE, "custom")
            putString(InputPrefs.CUSTOM_CLICK, "keys")
            putString(InputPrefs.CUSTOM_HOLD, "keys")
        }
        rule.onNode(inPane(str(R.string.keys_title_click))).assertExists()
        tag("shortcut_keys_hold").performClick()
        rule.onNode(inPane(str(R.string.keys_title_hold))).assertExists()
        rule.onNode(inPane(str(R.string.keys_title_click))).assertDoesNotExist()
    }

    @Test fun customWithoutKeysHasNoPane() {
        launch { putString(InputPrefs.PROFILE, "custom") }
        tag("editor_pane").assertDoesNotExist()
        tag("trigger_${InputPrefs.CUSTOM_CLICK}").assertExists()
    }

    @Test fun paneEditsSaveAtOnce() {
        launch { putString(InputPrefs.PROFILE, "graphics_tablet") }
        rule.onNode(clickableMatcher(str(R.string.keys_mod_shift))).performClick()
        assertEquals("3:44", prefs.getString(InputPrefs.KEYS_HOLD, null))
    }

    @Test fun pageDoesNotScrollForTheStandardProfiles() {
        launch(AirActions.On)
        for (profile in listOf("raw", "desktop", "drawing", "graphics_tablet")) {
            rule.runOnUiThread { store.setProfile(profile) }
            rule.waitForIdle()
            val node = tag("wide_page").fetchSemanticsNode()
            val range = node.config[SemanticsProperties.VerticalScrollAxisRange]
            val last = bounds("air_card")
            println("WIDE-FIT $profile viewport=${node.size.height}px scrollMax=${range.maxValue()} airCardBottom=${last.bottom}")
            assertEquals("page scrolls with profile $profile", 0f, range.maxValue(), 0f)
        }
    }

    @Test fun airCardSitsBelowThePalmRow() {
        launch(AirActions.On) { putString(InputPrefs.PROFILE, "graphics_tablet") }
        val palm = bounds("palm_row")
        val air = bounds("air_card")
        assertEquals(palm.left, air.left, 0.5f)
        assertTrue("air card top ${air.top} should be below palm row bottom ${palm.bottom}", air.top >= palm.bottom)
    }

    @Test @Config(sdk = [36], qualifiers = "w1056dp-h1691dp-280dpi")
    fun narrowPageHasTwoProfileColumnsAndAPane() {
        launch(AirActions.On) { putString(InputPrefs.PROFILE, "graphics_tablet") }
        tag("editor_pane").assertExists()
        val raw = profileCard(R.string.profile_raw)
        val desktop = profileCard(R.string.profile_desktop)
        val drawing = profileCard(R.string.profile_drawing)
        assertEquals(raw.top, desktop.top, 0.5f)
        assertTrue("third card should wrap to the next row", drawing.top > raw.top)
    }

    @Test fun wideProfileGridHasThreeColumns() {
        launch(AirActions.On) { putString(InputPrefs.PROFILE, "graphics_tablet") }
        val raw = profileCard(R.string.profile_raw)
        val desktop = profileCard(R.string.profile_desktop)
        val drawing = profileCard(R.string.profile_drawing)
        assertEquals(raw.top, desktop.top, 0.5f)
        assertEquals(raw.top, drawing.top, 0.5f)
        assertEquals(raw.width, desktop.width, 0.5f)
    }

    @Test @Config(sdk = [36], qualifiers = "w999dp-h800dp")
    fun justBelowTheBreakpointIsListDetail() {
        launch { putString(InputPrefs.PROFILE, "graphics_tablet") }
        tag("section_Pen").assertExists()
        tag("wide_page").assertDoesNotExist()
    }

    @Test @Config(sdk = [36], qualifiers = "w1000dp-h800dp")
    fun atTheBreakpointIsWide() {
        launch { putString(InputPrefs.PROFILE, "graphics_tablet") }
        tag("editor_pane").assertExists()
        tag("section_Pen").assertDoesNotExist()
    }
}

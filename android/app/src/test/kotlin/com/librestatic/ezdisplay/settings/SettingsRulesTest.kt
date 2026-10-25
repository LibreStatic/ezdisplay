package com.librestatic.ezdisplay.settings

import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.InputPrefs.KeyCombo
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SettingsRulesTest {
    private fun custom(
        click: String = "raw",
        dbl: String = "local_zoom",
        hold: String = "raw",
        dblHold: String = "none",
        contact: String = "raw",
        hover: String = "raw",
    ) = SettingsState(
        profile = "custom",
        custom = SettingsState().custom + mapOf(
            InputPrefs.CUSTOM_CLICK to click,
            InputPrefs.CUSTOM_DOUBLE_CLICK to dbl,
            InputPrefs.CUSTOM_HOLD to hold,
            InputPrefs.CUSTOM_DOUBLE_HOLD to dblHold,
            InputPrefs.CUSTOM_HOLD_DRAG_CONTACT to contact,
            InputPrefs.CUSTOM_HOLD_DRAG_HOVER to hover,
        ),
    )

    @Test fun presetProfilesShowNoEditors() {
        for (p in listOf("raw", "desktop", "drawing")) {
            assertEquals(emptyList<String>(), visibleKeyEditors(SettingsState(profile = p)))
        }
    }

    @Test fun graphicsTabletShowsHoldAndDoubleHold() {
        assertEquals(
            listOf(InputPrefs.KEYS_HOLD, InputPrefs.KEYS_DOUBLE_HOLD),
            visibleKeyEditors(SettingsState(profile = "graphics_tablet")),
        )
    }

    @Test fun customWithoutKeysShowsNone() {
        assertEquals(emptyList<String>(), visibleKeyEditors(custom()))
    }

    @Test fun customShowsEditorsInOrder() {
        assertEquals(
            listOf(
                InputPrefs.KEYS_CLICK, InputPrefs.KEYS_DOUBLE_CLICK, InputPrefs.KEYS_HOLD, InputPrefs.KEYS_DOUBLE_HOLD,
            ),
            visibleKeyEditors(custom("keys", "keys", "keys", "keys")),
        )
        assertEquals(listOf(InputPrefs.KEYS_DOUBLE_HOLD), visibleKeyEditors(custom(dblHold = "keys")))
        assertEquals(emptyList<String>(), visibleKeyEditors(custom(dblHold = "eraser")))
        assertEquals(listOf(InputPrefs.KEYS_CLICK), visibleKeyEditors(custom(click = "keys")))
        assertEquals(listOf(InputPrefs.KEYS_DOUBLE_CLICK), visibleKeyEditors(custom(dbl = "keys")))
        assertEquals(listOf(InputPrefs.KEYS_HOLD), visibleKeyEditors(custom(hold = "keys")))
        assertEquals(
            listOf(InputPrefs.KEYS_CLICK, InputPrefs.KEYS_HOLD),
            visibleKeyEditors(custom(click = "keys", hold = "keys")),
        )
    }

    @Test fun stepZoomOnEitherDragShowsBothZoomEditors() {
        val both = listOf(InputPrefs.KEYS_ZOOM_IN, InputPrefs.KEYS_ZOOM_OUT)
        assertEquals(both, visibleKeyEditors(custom(contact = "step_zoom")))
        assertEquals(both, visibleKeyEditors(custom(hover = "step_zoom")))
        assertEquals(
            listOf(InputPrefs.KEYS_CLICK) + both,
            visibleKeyEditors(custom(click = "keys", contact = "step_zoom", hover = "step_zoom")),
        )
        assertEquals(emptyList<String>(), visibleKeyEditors(custom(contact = "scroll")))
    }

    @Test fun panOnTheContactDragShowsThePanEditor() {
        assertEquals(listOf(InputPrefs.KEYS_PAN), visibleKeyEditors(custom(contact = "pan")))
        assertEquals(emptyList<String>(), visibleKeyEditors(custom(hover = "pan")))
        assertEquals(emptyList<String>(), visibleKeyEditors(custom(contact = "wheel_zoom", hover = "wheel_zoom")))
        assertEquals(listOf(InputPrefs.KEYS_HOLD), visibleKeyEditors(custom(hold = "keys", contact = "pan")))
    }

    @Test fun zoomEditorsAreHiddenWhileHoldSendsKeys() {
        assertEquals(listOf(InputPrefs.KEYS_HOLD), visibleKeyEditors(custom(hold = "keys", contact = "step_zoom")))
        assertEquals(emptyList<String>(), visibleKeyEditors(SettingsState(profile = "desktop")))
    }

    @Test fun nonCustomIgnoresKeysActions() {
        val s = custom("keys", "keys", "keys").copy(profile = "desktop")
        assertEquals(emptyList<String>(), visibleKeyEditors(s))
    }

    @Test fun dragsDisabledOnlyForCustomHoldKeys() {
        assertTrue(dragsDisabled(custom(hold = "keys")))
        assertFalse(dragsDisabled(custom(hold = "raw")))
        assertFalse(dragsDisabled(custom(click = "keys")))
        assertFalse(dragsDisabled(custom(hold = "keys").copy(profile = "graphics_tablet")))
    }

    @Test fun presetIndexMatchesAndMisses() {
        KeyPresets.forEachIndexed { i, p -> assertEquals(i, presetIndex(p.combo)) }
        assertEquals(-1, presetIndex(KeyCombo(InputPrefs.MOD_META, 0x04)))
        assertEquals(-1, presetIndex(KeyCombo(0, 0)))
    }

    @Test fun roundDoubleClickRoundsAndClamps() {
        assertEquals(300, roundDoubleClickMs(300f))
        assertEquals(300, roundDoubleClickMs(304f))
        assertEquals(310, roundDoubleClickMs(306f))
        assertEquals(150, roundDoubleClickMs(0f))
        assertEquals(150, roundDoubleClickMs(144f))
        assertEquals(800, roundDoubleClickMs(5000f))
        assertEquals(800, roundDoubleClickMs(796f))
    }

    @Test fun zoomNormalisation() {
        for (f in InputPrefs.ZOOM_FACTORS) assertEquals(f, normalizeZoomFactor(f), 0f)
        assertEquals(2f, normalizeZoomFactor(2.5f), 0f)
        assertEquals(2f, normalizeZoomFactor(0f), 0f)
    }
}

package com.librestatic.ezdisplay.screensaver

import android.content.SharedPreferences
import androidx.test.core.app.ApplicationProvider
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.display.PanelType
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class ScreensaverPrefsTest {
    private lateinit var prefs: SharedPreferences

    @Before fun setUp() {
        prefs = InputPrefs.prefs(ApplicationProvider.getApplicationContext())
        prefs.edit().clear().commit()
    }

    @Test fun defaultsFollowThePanel() {
        assertTrue(ScreensaverPrefs.read(prefs, PanelType.Oled).enabled)
        assertFalse(ScreensaverPrefs.read(prefs, PanelType.Lcd).enabled)
        assertFalse(ScreensaverPrefs.read(prefs, PanelType.Unknown).enabled)
        assertEquals(ScreensaverConfig(true, 5, 8, 30), ScreensaverPrefs.read(prefs, PanelType.Oled))
    }

    @Test fun anExplicitChoiceBeatsThePanel() {
        prefs.edit().putBoolean(InputPrefs.SCREENSAVER_ENABLED, false).commit()
        assertFalse(ScreensaverPrefs.read(prefs, PanelType.Oled).enabled)
        prefs.edit().putBoolean(InputPrefs.SCREENSAVER_ENABLED, true).commit()
        assertTrue(ScreensaverPrefs.read(prefs, PanelType.Lcd).enabled)
    }

    @Test fun storedValuesAreClamped() {
        prefs.edit().putInt(InputPrefs.SCREENSAVER_IDLE_MIN, 99).putInt(InputPrefs.SCREENSAVER_BRIGHTNESS_PCT, 0)
            .putInt(InputPrefs.SCREENSAVER_BLANK_MIN, 3).commit()
        assertEquals(ScreensaverConfig(false, 30, 1, 5), ScreensaverPrefs.read(prefs, PanelType.Lcd))
        prefs.edit().putInt(InputPrefs.SCREENSAVER_BLANK_MIN, 500).commit()
        assertEquals(120, ScreensaverPrefs.read(prefs, PanelType.Lcd).blankMin)
        prefs.edit().putInt(InputPrefs.SCREENSAVER_BLANK_MIN, 0).commit()
        assertEquals(0, ScreensaverPrefs.read(prefs, PanelType.Lcd).blankMin)
    }

    @Test fun blankSliderRounding() {
        assertEquals(0, roundBlankMin(0f))
        assertEquals(0, roundBlankMin(2f))
        assertEquals(5, roundBlankMin(3f))
        assertEquals(5, roundBlankMin(5f))
        assertEquals(30, roundBlankMin(31f))
        assertEquals(120, roundBlankMin(500f))
    }

    @Test fun neverReachesTheEngineJson() {
        prefs.edit().putBoolean(InputPrefs.SCREENSAVER_ENABLED, true).putInt(InputPrefs.SCREENSAVER_IDLE_MIN, 9).commit()
        val json = JSONObject(InputPrefs.toJson(ApplicationProvider.getApplicationContext()))
        assertFalse(json.keys().asSequence().any { it.startsWith("screensaver") || it.startsWith("panel") })
    }
}

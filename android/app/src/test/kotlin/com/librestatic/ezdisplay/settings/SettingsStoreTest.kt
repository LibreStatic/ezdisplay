package com.librestatic.ezdisplay.settings

import android.content.Context
import android.content.SharedPreferences
import androidx.test.core.app.ApplicationProvider
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.InputPrefs.KeyCombo
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class SettingsStoreTest {
    private lateinit var context: Context
    private lateinit var prefs: SharedPreferences
    private lateinit var store: SettingsStore

    @Before fun setUp() {
        context = ApplicationProvider.getApplicationContext()
        prefs = InputPrefs.prefs(context)
        prefs.edit().clear().commit()
        store = SettingsStore(prefs, AirActions.Off)
    }

    private val s get() = store.state.value

    @Test fun defaultsOnEmptyPrefs() {
        assertEquals("raw", s.profile)
        assertEquals("raw", s.custom[InputPrefs.CUSTOM_CLICK])
        assertEquals("local_zoom", s.custom[InputPrefs.CUSTOM_DOUBLE_CLICK])
        assertEquals("raw", s.custom[InputPrefs.CUSTOM_HOLD])
        assertEquals("none", s.custom[InputPrefs.CUSTOM_DOUBLE_HOLD])
        assertEquals("raw", s.custom[InputPrefs.CUSTOM_HOLD_DRAG_CONTACT])
        assertEquals("raw", s.custom[InputPrefs.CUSTOM_HOLD_DRAG_HOVER])
        assertEquals(InputPrefs.DEFAULT_KEYS_CLICK, s.keys[InputPrefs.KEYS_CLICK])
        assertEquals(InputPrefs.DEFAULT_KEYS_DOUBLE_CLICK, s.keys[InputPrefs.KEYS_DOUBLE_CLICK])
        assertEquals(InputPrefs.DEFAULT_KEYS_HOLD, s.keys[InputPrefs.KEYS_HOLD])
        assertEquals(KeyCombo(InputPrefs.MOD_CTRL or InputPrefs.MOD_ALT, 0x2C), s.keys[InputPrefs.KEYS_DOUBLE_HOLD])
        assertEquals(2f, s.zoomFactor, 0f)
        assertEquals(300, s.doubleClickMs)
        assertTrue(s.palmRejection)
        assertEquals(AirActions.Off, s.airActions)
    }

    @Test fun defaultsMatchSettingsStateDefaults() {
        assertEquals(SettingsState(airActions = AirActions.Off), s)
    }

    @Test fun readsStoredValuesAndNormalises() {
        prefs.edit().putString(InputPrefs.PROFILE, "graphics_tablet").putFloat(InputPrefs.ZOOM_FACTOR, 2.7f)
            .putInt(InputPrefs.DOUBLE_CLICK_MS, 5000).putString(InputPrefs.KEYS_HOLD, "bogus").commit()
        val st = SettingsStore(prefs, AirActions.On).state.value
        assertEquals("graphics_tablet", st.profile)
        assertEquals(2f, st.zoomFactor, 0f)
        assertEquals(800, st.doubleClickMs)
        assertEquals(InputPrefs.DEFAULT_KEYS_HOLD, st.keys[InputPrefs.KEYS_HOLD])
    }

    @Test fun setProfileWrites() {
        store.setProfile("drawing")
        assertEquals("drawing", prefs.getString(InputPrefs.PROFILE, null))
        assertEquals("drawing", s.profile)
    }

    @Test fun setProfileRejectsUnknown() {
        store.setProfile("nope")
        assertEquals("raw", s.profile)
        assertFalse(prefs.contains(InputPrefs.PROFILE))
    }

    @Test fun setCustomActionWrites() {
        store.setCustomAction(InputPrefs.CUSTOM_HOLD, "eraser")
        assertEquals("eraser", prefs.getString(InputPrefs.CUSTOM_HOLD, null))
        assertEquals("eraser", s.custom[InputPrefs.CUSTOM_HOLD])
    }

    @Test fun setCustomActionRejectsDisallowed() {
        store.setCustomAction(InputPrefs.CUSTOM_CLICK, "eraser")
        store.setCustomAction(InputPrefs.CUSTOM_HOLD_DRAG_HOVER, "keys")
        store.setCustomAction("unknown_trigger", "raw")
        assertFalse(prefs.contains(InputPrefs.CUSTOM_CLICK))
        assertFalse(prefs.contains(InputPrefs.CUSTOM_HOLD_DRAG_HOVER))
        assertEquals("raw", s.custom[InputPrefs.CUSTOM_CLICK])
        assertEquals("raw", s.custom[InputPrefs.CUSTOM_HOLD_DRAG_HOVER])
    }

    @Test fun setKeyComboWrites() {
        store.setKeyCombo(InputPrefs.KEYS_CLICK, KeyCombo(InputPrefs.MOD_ALT, 0x2C))
        assertEquals("4:44", prefs.getString(InputPrefs.KEYS_CLICK, null))
        assertEquals(KeyCombo(4, 0x2C), s.keys[InputPrefs.KEYS_CLICK])
    }

    @Test fun stepZoomIsOnlyAcceptedByTheDragTriggers() {
        store.setCustomAction(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT, "step_zoom")
        store.setCustomAction(InputPrefs.CUSTOM_HOLD_DRAG_HOVER, "step_zoom")
        assertEquals("step_zoom", s.custom[InputPrefs.CUSTOM_HOLD_DRAG_CONTACT])
        assertEquals("step_zoom", prefs.getString(InputPrefs.CUSTOM_HOLD_DRAG_HOVER, null))
        store.setCustomAction(InputPrefs.CUSTOM_HOLD, "step_zoom")
        store.setCustomAction(InputPrefs.CUSTOM_CLICK, "step_zoom")
        assertEquals("raw", s.custom[InputPrefs.CUSTOM_HOLD])
        assertEquals("raw", s.custom[InputPrefs.CUSTOM_CLICK])
    }

    @Test fun zoomShortcutsDefaultAndReachTheJson() {
        assertEquals(KeyCombo(InputPrefs.MOD_CTRL, 0x2C), s.keys[InputPrefs.KEYS_ZOOM_IN])
        assertEquals(KeyCombo(InputPrefs.MOD_CTRL or InputPrefs.MOD_ALT, 0x2C), s.keys[InputPrefs.KEYS_ZOOM_OUT])
        store.setKey(InputPrefs.KEYS_ZOOM_IN, 0x04)
        store.toggleModifier(InputPrefs.KEYS_ZOOM_OUT, InputPrefs.MOD_SHIFT)
        val k = JSONObject(InputPrefs.toJson(context)).getJSONObject("keys")
        assertEquals(1, k.getJSONObject("zoom_in").getInt("mods"))
        assertEquals(0x04, k.getJSONObject("zoom_in").getInt("key"))
        assertEquals(7, k.getJSONObject("zoom_out").getInt("mods"))
        assertEquals(0x2C, k.getJSONObject("zoom_out").getInt("key"))
    }

    @Test fun stepZoomMakesTheProfileUseKeys() {
        store.setProfile("custom")
        assertFalse(InputPrefs.usesKeys(context))
        store.setCustomAction(InputPrefs.CUSTOM_HOLD_DRAG_HOVER, "step_zoom")
        assertTrue(InputPrefs.usesKeys(context))
    }

    @Test fun wheelZoomAndPanAreOnlyAcceptedByTheirDragTriggers() {
        store.setCustomAction(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT, "wheel_zoom")
        store.setCustomAction(InputPrefs.CUSTOM_HOLD_DRAG_HOVER, "wheel_zoom")
        assertEquals("wheel_zoom", s.custom[InputPrefs.CUSTOM_HOLD_DRAG_CONTACT])
        assertEquals("wheel_zoom", s.custom[InputPrefs.CUSTOM_HOLD_DRAG_HOVER])
        store.setCustomAction(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT, "pan")
        assertEquals("pan", prefs.getString(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT, null))
        store.setCustomAction(InputPrefs.CUSTOM_HOLD_DRAG_HOVER, "pan")
        assertEquals("wheel_zoom", s.custom[InputPrefs.CUSTOM_HOLD_DRAG_HOVER])
        store.setCustomAction(InputPrefs.CUSTOM_HOLD, "wheel_zoom")
        assertEquals("raw", s.custom[InputPrefs.CUSTOM_HOLD])
    }

    @Test fun panShortcutDefaultsToSpaceAndReachesTheJson() {
        assertEquals(KeyCombo(0, 0x2C), s.keys[InputPrefs.KEYS_PAN])
        store.setKey(InputPrefs.KEYS_PAN, 0x04)
        store.toggleModifier(InputPrefs.KEYS_PAN, InputPrefs.MOD_ALT)
        val k = JSONObject(InputPrefs.toJson(context)).getJSONObject("keys")
        assertEquals(4, k.getJSONObject("pan").getInt("mods"))
        assertEquals(0x04, k.getJSONObject("pan").getInt("key"))
    }

    @Test fun panMakesTheProfileUseKeysButWheelZoomDoesNot() {
        store.setProfile("custom")
        store.setCustomAction(InputPrefs.CUSTOM_HOLD_DRAG_HOVER, "wheel_zoom")
        store.setCustomAction(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT, "wheel_zoom")
        assertFalse(InputPrefs.usesKeys(context))
        store.setCustomAction(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT, "pan")
        assertTrue(InputPrefs.usesKeys(context))
    }

    @Test fun doubleHoldOnlyAcceptsEraserKeysAndNone() {
        store.setCustomAction(InputPrefs.CUSTOM_DOUBLE_HOLD, "right_click")
        assertEquals("none", s.custom[InputPrefs.CUSTOM_DOUBLE_HOLD])
        store.setCustomAction(InputPrefs.CUSTOM_DOUBLE_HOLD, "eraser")
        assertEquals("eraser", prefs.getString(InputPrefs.CUSTOM_DOUBLE_HOLD, null))
    }

    @Test fun toggleModifierFlipsBit() {
        store.toggleModifier(InputPrefs.KEYS_CLICK, InputPrefs.MOD_SHIFT)
        assertEquals(KeyCombo(InputPrefs.MOD_CTRL or InputPrefs.MOD_SHIFT, 0x1D), s.keys[InputPrefs.KEYS_CLICK])
        assertEquals("3:29", prefs.getString(InputPrefs.KEYS_CLICK, null))
        store.toggleModifier(InputPrefs.KEYS_CLICK, InputPrefs.MOD_CTRL)
        assertEquals("2:29", prefs.getString(InputPrefs.KEYS_CLICK, null))
    }

    @Test fun setKeyKeepsModifiers() {
        store.setKey(InputPrefs.KEYS_HOLD, 0x04)
        assertEquals("1:4", prefs.getString(InputPrefs.KEYS_HOLD, null))
        assertEquals(KeyCombo(InputPrefs.MOD_CTRL, 0x04), s.keys[InputPrefs.KEYS_HOLD])
    }

    @Test fun setZoomFactorWrites() {
        store.setZoomFactor(3f)
        assertEquals(3f, prefs.getFloat(InputPrefs.ZOOM_FACTOR, 0f), 0f)
        assertEquals(3f, s.zoomFactor, 0f)
    }

    @Test fun setDoubleClickMsWrites() {
        store.setDoubleClickMs(roundDoubleClickMs(447f))
        assertEquals(450, prefs.getInt(InputPrefs.DOUBLE_CLICK_MS, 0))
        assertEquals(450, s.doubleClickMs)
    }

    @Test fun setPalmRejectionWrites() {
        store.setPalmRejection(false)
        assertFalse(prefs.getBoolean(InputPrefs.PALM_REJECTION, true))
        assertFalse(s.palmRejection)
    }

    @Test fun toJsonContract() {
        store.setProfile("custom")
        store.setCustomAction(InputPrefs.CUSTOM_CLICK, "keys")
        store.setCustomAction(InputPrefs.CUSTOM_DOUBLE_CLICK, "middle_click")
        store.setCustomAction(InputPrefs.CUSTOM_HOLD, "eraser")
        store.setCustomAction(InputPrefs.CUSTOM_DOUBLE_HOLD, "keys")
        store.setCustomAction(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT, "scroll")
        store.setCustomAction(InputPrefs.CUSTOM_HOLD_DRAG_HOVER, "pc_zoom")
        store.setKeyCombo(InputPrefs.KEYS_CLICK, KeyCombo(InputPrefs.MOD_META, 0x04))
        store.toggleModifier(InputPrefs.KEYS_DOUBLE_CLICK, InputPrefs.MOD_ALT)
        store.setKey(InputPrefs.KEYS_HOLD, 0x29)
        store.setKey(InputPrefs.KEYS_DOUBLE_HOLD, 0x05)
        store.setZoomFactor(1.5f)
        store.setDoubleClickMs(250)
        store.setPalmRejection(false)

        val j = JSONObject(InputPrefs.toJson(context))
        assertEquals("custom", j.getString("profile"))
        val c = j.getJSONObject("custom")
        assertEquals("keys", c.getString("click"))
        assertEquals("middle_click", c.getString("double_click"))
        assertEquals("eraser", c.getString("hold"))
        assertEquals("keys", c.getString("double_hold"))
        assertEquals("scroll", c.getString("hold_drag_contact"))
        assertEquals("pc_zoom", c.getString("hold_drag_hover"))
        val k = j.getJSONObject("keys")
        assertEquals(8, k.getJSONObject("click").getInt("mods"))
        assertEquals(0x04, k.getJSONObject("click").getInt("key"))
        assertEquals(7, k.getJSONObject("double_click").getInt("mods"))
        assertEquals(0x1D, k.getJSONObject("double_click").getInt("key"))
        assertEquals(1, k.getJSONObject("hold").getInt("mods"))
        assertEquals(0x29, k.getJSONObject("hold").getInt("key"))
        assertEquals(5, k.getJSONObject("double_hold").getInt("mods"))
        assertEquals(0x05, k.getJSONObject("double_hold").getInt("key"))
        assertEquals(1.5, j.getDouble("zoom_factor"), 0.0)
        assertEquals(250, j.getInt("double_click_ms"))
        assertFalse(j.getBoolean("palm_rejection"))
    }

    @Test fun untouchedPrefsGiveDefaultJson() {
        val j = JSONObject(InputPrefs.toJson(context))
        assertEquals("raw", j.getString("profile"))
        assertEquals("local_zoom", j.getJSONObject("custom").getString("double_click"))
        assertEquals("none", j.getJSONObject("custom").getString("double_hold"))
        assertEquals(5, j.getJSONObject("keys").getJSONObject("double_hold").getInt("mods"))
        assertEquals(0x2C, j.getJSONObject("keys").getJSONObject("double_hold").getInt("key"))
        assertEquals(2.0, j.getDouble("zoom_factor"), 0.0)
        assertEquals(300, j.getInt("double_click_ms"))
        assertTrue(j.getBoolean("palm_rejection"))
    }

    @Test fun unknownStoredProfileFallsBackToRaw() {
        prefs.edit().putString(InputPrefs.PROFILE, "retired_value").commit()
        assertEquals("raw", InputPrefs.profile(prefs))
        assertEquals("raw", JSONObject(InputPrefs.toJson(context)).getString("profile"))
        assertEquals("raw", SettingsStore(prefs, AirActions.Off).state.value.profile)
    }
}

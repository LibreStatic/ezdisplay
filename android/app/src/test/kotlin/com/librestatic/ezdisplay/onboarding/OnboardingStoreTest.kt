package com.librestatic.ezdisplay.onboarding

import android.content.Context
import android.content.SharedPreferences
import androidx.test.core.app.ApplicationProvider
import com.librestatic.ezdisplay.InputPrefs
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class OnboardingStoreTest {
    private lateinit var context: Context
    private lateinit var prefs: SharedPreferences

    @Before fun setUp() {
        context = ApplicationProvider.getApplicationContext()
        prefs = InputPrefs.prefs(context)
        prefs.edit().clear().commit()
    }

    @Test fun freshInstallShowsTheGuide() {
        assertTrue(OnboardingStore.shouldShow(context))
        assertFalse(OnboardingStore(prefs).isCompleted())
    }

    @Test fun panelCacheAloneIsStillAFreshInstall() {
        prefs.edit().putString(InputPrefs.PANEL_TYPE_DETECTED, "Oled").commit()
        assertTrue(OnboardingStore.shouldShow(context))
    }

    @Test fun markCompletedHidesTheGuide() {
        OnboardingStore(prefs).markCompleted()
        assertFalse(OnboardingStore.shouldShow(context))
        assertEquals(ONBOARDING_VERSION, prefs.getInt(OnboardingStore.KEY, 0))
    }

    @Test fun existingSettingsCountAsCompletedAndAreRecorded() {
        prefs.edit().putString(InputPrefs.PROFILE, "drawing").commit()
        assertFalse(OnboardingStore.shouldShow(context))
        assertEquals(ONBOARDING_VERSION, prefs.getInt(OnboardingStore.KEY, 0))
    }

    @Test fun versionBumpShowsTheGuideAgain() {
        OnboardingStore(prefs, version = 1).markCompleted()
        assertTrue(OnboardingStore(prefs, version = 1).isCompleted())
        assertFalse(OnboardingStore(prefs, version = 2).isCompleted())
        OnboardingStore(prefs, version = 2).markCompleted()
        assertTrue(OnboardingStore(prefs, version = 2).isCompleted())
    }

    @Test fun resetShowsTheGuideAgainEvenWithOtherSettings() {
        prefs.edit().putString(InputPrefs.PROFILE, "drawing").commit()
        val store = OnboardingStore(prefs)
        assertTrue(store.isCompleted())
        store.reset()
        assertFalse(prefs.contains(OnboardingStore.KEY))
        // Another pref is there, so the upgrade rule applies again: reset is for a forced replay, not for the check.
        assertTrue(store.isCompleted())
    }

    @Test fun resetOnAFreshInstallShowsTheGuide() {
        val store = OnboardingStore(prefs)
        store.markCompleted()
        prefs.edit().clear().commit()
        store.reset()
        assertFalse(store.isCompleted())
    }
}

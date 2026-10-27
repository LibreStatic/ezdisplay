package com.librestatic.ezdisplay.onboarding

import android.content.Context
import android.content.SharedPreferences
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsSelected
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.display.PanelType
import com.librestatic.ezdisplay.settings.AirActions
import com.librestatic.ezdisplay.settings.SettingsStore
import com.librestatic.ezdisplay.settings.SettingsTheme
import com.librestatic.ezdisplay.wifi.WifiBackend
import java.util.concurrent.Executor
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class OnboardingUiTest {
    @get:Rule val rule = createAndroidComposeRule<ComponentActivity>()

    private lateinit var prefs: SharedPreferences
    private lateinit var store: SettingsStore
    private var finished = 0

    @Before fun setUp() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        prefs = InputPrefs.prefs(context)
        prefs.edit().clear().commit()
        // The hero and hints animate for ever, which would keep Compose from ever going idle.
        Settings.Global.putFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
    }

    private fun launch(panel: PanelType = PanelType.Oled, initial: OnboardingStep = OnboardingStep.Welcome) {
        store = SettingsStore(prefs, AirActions.Off, WifiBackend.None, "test tablet", io = Executor { it.run() }, panelType = panel)
        rule.setContent {
            SettingsTheme(dynamicColor = false) {
                OnboardingContent(store.state.value, store, onFinish = { finished++ }, initialStep = initial)
            }
        }
        rule.waitForIdle()
    }

    private fun tag(t: String) = rule.onNodeWithTag(t)

    private fun next() { tag("onboarding_next").performClick(); rule.waitForIdle() }

    private fun back() { tag("onboarding_back").performClick(); rule.waitForIdle() }

    @Test fun nextAndBackWalkThroughEveryStep() {
        launch()
        val steps = OnboardingStep.entries
        tag("onboarding_step_Welcome").assertIsDisplayed()
        for (i in 1 until steps.size) {
            next()
            tag("onboarding_step_${steps[i].name}").assertIsDisplayed()
        }
        for (i in steps.size - 2 downTo 0) {
            back()
            tag("onboarding_step_${steps[i].name}").assertIsDisplayed()
        }
        assertEquals(0, finished)
    }

    @Test fun skipIsHiddenOnTheLastStep() {
        launch(initial = OnboardingStep.Done)
        tag("onboarding_skip").assertDoesNotExist()
    }

    @Test fun skipFinishesOnce() {
        launch()
        tag("onboarding_skip").performClick()
        rule.waitForIdle()
        assertEquals(1, finished)
    }

    @Test fun connectOnTheLastStepFinishes() {
        launch(initial = OnboardingStep.Done)
        next()
        assertEquals(1, finished)
    }

    @Test fun profileChoiceIsSaved() {
        launch(initial = OnboardingStep.PenProfile)
        tag("onboarding_profile_raw").assertIsSelected()
        tag("onboarding_profile_graphics_tablet").performScrollTo().performClick()
        rule.waitForIdle()
        assertEquals("graphics_tablet", prefs.getString(InputPrefs.PROFILE, null))
        tag("onboarding_profile_graphics_tablet").assertIsSelected()
    }

    @Test fun screensaverSwitchIsSaved() {
        launch(panel = PanelType.Oled, initial = OnboardingStep.Display)
        assertTrue(store.state.value.screensaverEnabled)
        tag("onboarding_screensaver_row").performScrollTo().performClick()
        rule.waitForIdle()
        assertFalse(prefs.getBoolean(InputPrefs.SCREENSAVER_ENABLED, true))
        tag("onboarding_screensaver_row").performClick()
        rule.waitForIdle()
        assertTrue(prefs.getBoolean(InputPrefs.SCREENSAVER_ENABLED, false))
    }

    @Test fun displayStepExplainsTheTimings() {
        launch(initial = OnboardingStep.Display)
        val text = rule.activity.getString(
            com.librestatic.ezdisplay.R.string.onboarding_display_explain, "5 minutes", 8, "30 minutes",
        )
        tag("onboarding_display_explain").performScrollTo().assertIsDisplayed()
        tag("onboarding_display_explain").assertTextEquals(text)
    }

    @Test fun connectionStepSwitchesBetweenUsbAndWifi() {
        launch(initial = OnboardingStep.Connection)
        tag("onboarding_usb_info").assertIsDisplayed()
        tag("onboarding_choice_wifi").performClick()
        rule.waitForIdle()
        tag("connection_content").performScrollTo().assertIsDisplayed()
        tag("onboarding_choice_usb").performScrollTo().performClick()
        rule.waitForIdle()
        tag("onboarding_usb_info").performScrollTo().assertIsDisplayed()
    }

    @Test fun hostStepHasDownloadAndCopy() {
        launch(initial = OnboardingStep.HostInstall)
        tag("onboarding_download").performScrollTo().assertIsDisplayed()
        tag("onboarding_copy").performScrollTo().performClick()
        rule.waitForIdle()
        val clip = rule.activity.getSystemService(android.content.ClipboardManager::class.java).primaryClip
        assertEquals(HostCommand, clip?.getItemAt(0)?.text?.toString())
    }
}

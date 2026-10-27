package com.librestatic.ezdisplay.onboarding

import android.content.Context
import android.content.SharedPreferences
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.printToString
import androidx.test.core.app.ApplicationProvider
import com.github.takahirom.roborazzi.captureRoboImage
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.display.PanelType
import com.librestatic.ezdisplay.settings.AirActions
import com.librestatic.ezdisplay.settings.SettingsStore
import com.librestatic.ezdisplay.settings.SettingsTheme
import com.librestatic.ezdisplay.wifi.WifiBackend
import java.util.concurrent.Executor
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * Renders every step at phone and tablet widths. Under plain unit tests these only render; run
 * `./gradlew :app:recordRoborazziFullDebug` to write the PNGs to `app/build/outputs/roborazzi/`.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class OnboardingScreenshotTest {
    @get:Rule val rule = createAndroidComposeRule<ComponentActivity>()

    private lateinit var prefs: SharedPreferences

    @Before fun setUp() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        prefs = InputPrefs.prefs(context)
        prefs.edit().clear().commit()
        // The hero and hints animate for ever, which would keep Compose from ever going idle.
        Settings.Global.putFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
    }

    private fun show(step: OnboardingStep, name: String) {
        val store = SettingsStore(
            prefs, AirActions.Off, WifiBackend.None, "test tablet", io = Executor { it.run() }, panelType = PanelType.Oled,
        )
        rule.setContent {
            SettingsTheme(dynamicColor = false) {
                OnboardingContent(store.state.value, store, onFinish = {}, initialStep = step)
            }
        }
        rule.waitForIdle()
        val roots = rule.onAllNodes(isRoot())
        val last = roots[roots.fetchSemanticsNodes().size - 1]
        println("=== SEMANTICS $name ===\n" + last.printToString(maxDepth = 14) + "\n=== END $name ===")
        last.captureRoboImage("build/outputs/roborazzi/$name.png")
    }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun phone_welcome() = show(OnboardingStep.Welcome, "onboarding_phone_1_welcome")

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun phone_host() = show(OnboardingStep.HostInstall, "onboarding_phone_2_host")

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun phone_connection() = show(OnboardingStep.Connection, "onboarding_phone_3_connection")

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun phone_pen() = show(OnboardingStep.PenProfile, "onboarding_phone_4_pen")

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun phone_display() = show(OnboardingStep.Display, "onboarding_phone_5_display")

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun phone_done() = show(OnboardingStep.Done, "onboarding_phone_6_done")

    @Test @Config(sdk = [36], qualifiers = "w1280dp-h800dp-night")
    fun tablet_welcome() = show(OnboardingStep.Welcome, "onboarding_tablet_1_welcome")

    @Test @Config(sdk = [36], qualifiers = "w1280dp-h800dp-night")
    fun tablet_host() = show(OnboardingStep.HostInstall, "onboarding_tablet_2_host")

    @Test @Config(sdk = [36], qualifiers = "w1280dp-h800dp-night")
    fun tablet_connection() = show(OnboardingStep.Connection, "onboarding_tablet_3_connection")

    @Test @Config(sdk = [36], qualifiers = "w1280dp-h800dp-night")
    fun tablet_pen() = show(OnboardingStep.PenProfile, "onboarding_tablet_4_pen")

    @Test @Config(sdk = [36], qualifiers = "w1280dp-h800dp-night")
    fun tablet_display() = show(OnboardingStep.Display, "onboarding_tablet_5_display")

    @Test @Config(sdk = [36], qualifiers = "w1280dp-h800dp-night")
    fun tablet_done() = show(OnboardingStep.Done, "onboarding_tablet_6_done")
}

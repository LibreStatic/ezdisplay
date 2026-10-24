package io.github.facum.ezdisplay.settings

import androidx.activity.ComponentActivity
import androidx.compose.ui.test.hasScrollAction
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.printToString
import androidx.compose.ui.test.swipeUp
import com.github.takahirom.roborazzi.captureRoboImage
import io.github.facum.ezdisplay.InputPrefs
import io.github.facum.ezdisplay.InputPrefs.KeyCombo
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * Renders the screen at the target sizes. Under plain unit tests these only render; run
 * `./gradlew :app:recordRoborazziFullDebug` to write the PNGs to `app/build/outputs/roborazzi/`.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class SettingsScreenshotTest {
    @get:Rule val rule = createAndroidComposeRule<ComponentActivity>()

    private object NoOpActions : SettingsActions {
        override fun setProfile(profile: String) {}
        override fun setCustomAction(triggerKey: String, action: String) {}
        override fun setKeyCombo(prefKey: String, combo: KeyCombo) {}
        override fun toggleModifier(prefKey: String, bit: Int) {}
        override fun setKey(prefKey: String, usage: Int) {}
        override fun setZoomFactor(factor: Float) {}
        override fun setDoubleClickMs(ms: Int) {}
        override fun setPalmRejection(on: Boolean) {}
    }

    private val tabletState = SettingsState(
        profile = "graphics_tablet",
        keys = SettingsState().keys + (InputPrefs.KEYS_HOLD to KeyCombo(InputPrefs.MOD_CTRL, 0x2C)),
        airActions = AirActions.On,
    )

    private val customKeys = SettingsState(
        profile = "custom",
        custom = SettingsState().custom + mapOf(
            InputPrefs.CUSTOM_CLICK to "keys",
            InputPrefs.CUSTOM_HOLD to "keys",
        ),
        airActions = AirActions.On,
    )

    private fun show(state: SettingsState) {
        rule.setContent { SettingsTheme(dynamicColor = false) { SettingsContent(state, NoOpActions) } }
        rule.waitForIdle()
    }

    private fun capture(name: String) {
        println("=== SEMANTICS $name ===\n" + rule.onRoot().printToString(maxDepth = 14) + "\n=== END $name ===")
        rule.onRoot().captureRoboImage("build/outputs/roborazzi/$name.png")
    }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun phone_main_dark() { show(tabletState); capture("phone_main_dark") }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun phone_main_bottom_dark() {
        show(tabletState)
        // Swipe to the very end so the bottom inset and padding are in the capture too.
        repeat(2) { rule.onNode(hasScrollAction()).performTouchInput { swipeUp() } }
        rule.waitForIdle()
        capture("phone_main_bottom_dark")
    }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun phone_editor_dark() {
        show(tabletState)
        rule.onNodeWithTag("shortcut_keys_hold").performClick()
        rule.waitForIdle()
        capture("phone_editor_dark")
    }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-notnight")
    fun phone_main_light() { show(tabletState); capture("phone_main_light") }

    @Test @Config(sdk = [36], qualifiers = "de-w412dp-h915dp-notnight")
    fun phone_main_german_light() { show(tabletState); capture("phone_main_german_light") }

    @Test @Config(sdk = [36], qualifiers = "w841dp-h701dp-night")
    fun foldable_dark() { show(tabletState); capture("foldable_dark") }

    @Test @Config(sdk = [36], qualifiers = "w1691dp-h1056dp-night-280dpi")
    fun tablet_dark() { show(tabletState); capture("tablet_dark") }

    @Test @Config(sdk = [36], qualifiers = "w1691dp-h1056dp-notnight-280dpi")
    fun tablet_light() { show(tabletState); capture("tablet_light") }

    @Test @Config(sdk = [36], qualifiers = "w1691dp-h1056dp-night-280dpi")
    fun tablet_custom_dark() { show(customKeys); capture("tablet_custom_dark") }

    @Test @Config(sdk = [36], qualifiers = "w1056dp-h1691dp-night-280dpi")
    fun tablet_portrait_dark() { show(tabletState); capture("tablet_portrait_dark") }
}

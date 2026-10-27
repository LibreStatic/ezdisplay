package com.librestatic.ezdisplay.settings

import com.librestatic.ezdisplay.screensaver.ScreensaverMotionMode
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.hasScrollAction
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.printToString
import androidx.compose.ui.test.swipeUp
import com.github.takahirom.roborazzi.captureRoboImage
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.InputPrefs.KeyCombo
import com.librestatic.ezdisplay.display.PanelType
import com.librestatic.ezdisplay.screensaver.ScreensaverContent
import com.librestatic.ezdisplay.wifi.DiscoveredHost
import com.librestatic.ezdisplay.wifi.PairLink
import com.librestatic.ezdisplay.wifi.WifiHost
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
        override fun setWifiEnabled(on: Boolean) {}
        override fun setScreensaverEnabled(on: Boolean) {}
        override fun setScreensaverIdleMin(min: Int) {}
        override fun setScreensaverBrightnessPct(pct: Int) {}
        override fun setScreensaverBlankMin(min: Int) {}
        override fun setScreensaverSpeed(level: Int) {}
        override fun setScreensaverMotion(mode: ScreensaverMotionMode) {}
        override fun startPairing(name: String, shortFp: String, addr: String) {}
        override fun startPairing(link: PairLink) {}
        override fun submitPairing(pin: String) {}
        override fun cancelPairing() {}
        override fun forgetHost(fingerprint: String) {}
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

    @org.junit.Before fun noAnimations() {
        // The screensaver preview animates for ever, which would keep Compose from ever going idle.
        android.provider.Settings.Global.putFloat(
            androidx.test.core.app.ApplicationProvider.getApplicationContext<android.content.Context>().contentResolver,
            android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 0f,
        )
    }

    private fun show(state: SettingsState) {
        rule.setContent { SettingsTheme(dynamicColor = false) { SettingsContent(state, NoOpActions) } }
        rule.waitForIdle()
    }

    private fun capture(name: String) {
        val roots = rule.onAllNodes(isRoot())
        val last = roots[roots.fetchSemanticsNodes().size - 1]
        println("=== SEMANTICS $name ===\n" + last.printToString(maxDepth = 14) + "\n=== END $name ===")
        last.captureRoboImage("build/outputs/roborazzi/$name.png")
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

    private val connectionState = SettingsState(
        wifiEnabled = true,
        wifiHosts = listOf(WifiHost("Laptop", "cd".repeat(32), "cdcd-cdcd-cdcd-cdcd", "10.0.0.7:27183")),
        discovered = listOf(DiscoveredHost("desk", "Studio PC", "3f2a-91c0-77de-0b14", "192.168.1.9:27183")),
        deviceFingerprint = "abcd-0123-4567-89ef",
    )

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun phone_connection_dark() {
        show(connectionState)
        rule.onNodeWithTag("connection_content").performScrollTo()
        rule.waitForIdle()
        capture("phone_connection_dark")
    }

    @Test @Config(sdk = [36], qualifiers = "w841dp-h701dp-night")
    fun foldable_connection_dark() {
        show(connectionState)
        rule.onNodeWithTag("section_Connection").performClick()
        rule.waitForIdle()
        capture("foldable_connection_dark")
    }

    @Test @Config(sdk = [36], qualifiers = "w1691dp-h978dp-night-280dpi")
    fun tablet_connection_dark() { show(connectionState.copy(profile = "graphics_tablet")); capture("tablet_connection_dark") }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun phone_pair_dialog_dark() {
        show(connectionState.copy(pairing = PairingState("Studio PC", "3f2a-91c0-77de-0b14", "192.168.1.9:27183")))
        capture("phone_pair_dialog_dark")
    }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun phone_pair_link_dialog_dark() {
        val link = PairLink("192.168.1.9:27183", "3f2a91c077de0b14".padEnd(64, '0'), "424242", "Studio PC")
        show(connectionState.copy(pairing = PairingState(link.name, link.shortFp, link.addr, link)))
        capture("phone_pair_link_dialog_dark")
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

    private val displayState = SettingsState(
        panelType = PanelType.Oled,
        screensaverEnabled = true,
    )

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun phone_display_dark() {
        show(displayState)
        rule.onNodeWithTag("screensaver_row").performScrollTo()
        rule.waitForIdle()
        capture("phone_display_dark")
    }

    @Test @Config(sdk = [36], qualifiers = "w841dp-h701dp-night")
    fun foldable_display_dark() {
        show(displayState)
        rule.onNodeWithTag("section_Display").performClick()
        rule.waitForIdle()
        capture("foldable_display_dark")
    }

    @Test @Config(sdk = [36], qualifiers = "w1691dp-h978dp-night-280dpi")
    fun tablet_display_sheet_dark() {
        show(displayState)
        rule.onNodeWithTag("screensaver_button").performClick()
        rule.waitForIdle()
        capture("tablet_display_sheet_dark")
    }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun screensaver_content() {
        rule.setContent { ScreensaverContent(Modifier.fillMaxSize(), seed = 7) }
        rule.waitForIdle()
        capture("screensaver_content")
    }

    @Test @Config(sdk = [36], qualifiers = "w412dp-h915dp-night")
    fun screensaver_content_blank() {
        rule.setContent { ScreensaverContent(Modifier.fillMaxSize(), blank = true, seed = 7) }
        rule.waitForIdle()
        capture("screensaver_content_blank")
    }
}

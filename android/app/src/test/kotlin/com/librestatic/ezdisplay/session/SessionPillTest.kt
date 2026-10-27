package com.librestatic.ezdisplay.session

import androidx.activity.ComponentActivity
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.printToString
import com.github.takahirom.roborazzi.captureRoboImage
import com.librestatic.ezdisplay.settings.SettingsTheme
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [36], qualifiers = "w841dp-h701dp-night")
class SessionPillTest {
    @get:Rule val rule = createAndroidComposeRule<ComponentActivity>()

    @Before fun noAnimations() {
        android.provider.Settings.Global.putFloat(
            androidx.test.core.app.ApplicationProvider.getApplicationContext<android.content.Context>().contentResolver,
            android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 0f,
        )
    }

    @Test fun buttonsCallBack() {
        var settings = 0
        var disconnect = 0
        rule.setContent {
            SettingsTheme(dynamicColor = false) {
                SessionPill(true, { settings++ }, { disconnect++ }, {})
            }
        }
        rule.onNodeWithText("Input settings").performClick()
        rule.onNodeWithText("Disconnect").performClick()
        assertEquals(1, settings)
        assertEquals(1, disconnect)
    }

    @Test fun hiddenPillShowsNothing() {
        rule.setContent { SettingsTheme(dynamicColor = false) { SessionPill(false, {}, {}, {}) } }
        rule.onNodeWithText("Disconnect").assertDoesNotExist()
    }

    @Test fun hidesItselfAfterTheTimeout() {
        var visible by mutableStateOf(true)
        rule.mainClock.autoAdvance = false
        rule.setContent { SettingsTheme(dynamicColor = false) { SessionPill(visible, {}, {}, { visible = false }) } }
        rule.mainClock.advanceTimeBy(SESSION_PILL_TIMEOUT_MS + 100)
        rule.mainClock.autoAdvance = true
        rule.waitForIdle()
        assertEquals(false, visible)
    }

    @Test fun capture() {
        rule.setContent { SettingsTheme(dynamicColor = false) { SessionPill(true, {}, {}, {}) } }
        rule.waitForIdle()
        val roots = rule.onAllNodes(isRoot())
        val last = roots[roots.fetchSemanticsNodes().size - 1]
        println("=== SEMANTICS session_pill ===\n" + last.printToString(maxDepth = 14) + "\n=== END session_pill ===")
        last.captureRoboImage("build/outputs/roborazzi/session_pill.png")
    }
}

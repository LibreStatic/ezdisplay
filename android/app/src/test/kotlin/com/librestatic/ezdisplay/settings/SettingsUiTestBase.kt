package com.librestatic.ezdisplay.settings

import android.content.SharedPreferences
import androidx.activity.ComponentActivity
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.SemanticsNodeInteraction
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.AndroidComposeTestRule
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.printToString
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.onRoot
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.wifi.WifiBackend
import org.junit.Before
import org.junit.Rule
import org.junit.runner.RunWith
import java.util.concurrent.Executor

/** Shared setup: a real [SettingsStore] on cleared Robolectric prefs, rendered inside [SettingsTheme]. */
@RunWith(AndroidJUnit4::class)
abstract class SettingsUiTestBase {
    @get:Rule val rule: AndroidComposeTestRule<*, ComponentActivity> = createAndroidComposeRule<ComponentActivity>()

    protected lateinit var prefs: SharedPreferences
    protected lateinit var store: SettingsStore

    @Before fun clearPrefs() {
        prefs = InputPrefs.prefs(ApplicationProvider.getApplicationContext())
        prefs.edit().clear().commit()
    }

    /** Seeds [seed] into prefs, then shows the screen. */
    protected fun launch(
        airActions: AirActions = AirActions.Off,
        wifi: WifiBackend = WifiBackend.None,
        seed: SharedPreferences.Editor.() -> Unit = {},
    ) {
        prefs.edit().apply(seed).commit()
        store = SettingsStore(prefs, airActions, wifi, "test tablet", io = Executor { it.run() })
        rule.setContent { SettingsTheme { SettingsScreen(store) } }
        rule.waitForIdle()
    }

    protected fun str(id: Int, vararg args: Any): String = rule.activity.getString(id, *args)

    protected fun tag(t: String): SemanticsNodeInteraction = rule.onNodeWithTag(t)

    /** A clickable node (chip, button) with [text]; skips plain texts such as keycaps. */
    protected fun clickable(text: String): SemanticsNodeInteraction =
        rule.onNode(hasText(text) and hasClickAction())

    protected fun clickableMatcher(text: String): SemanticsMatcher = hasText(text) and hasClickAction()

    protected fun inPane(text: String): SemanticsMatcher = hasText(text) and hasAnyAncestor(hasTestTag("editor_pane"))

    protected fun back() {
        rule.runOnUiThread { rule.activity.onBackPressedDispatcher.onBackPressed() }
        rule.waitForIdle()
    }

    protected fun dumpTree(label: String) {
        println("=== SEMANTICS $label ===\n" + rule.onAllNodes(isRoot()).fetchSemanticsNodes().indices.joinToString("\n--- window ---\n") { rule.onAllNodes(isRoot())[it].printToString(maxDepth = 12) } + "\n=== END $label ===")
    }

    protected fun SharedPreferences.editor(block: SharedPreferences.Editor.() -> Unit) = edit().apply(block).commit()
}

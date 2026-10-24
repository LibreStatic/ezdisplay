package io.github.facum.ezdisplay

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import io.github.facum.ezdisplay.settings.SettingsScreen
import io.github.facum.ezdisplay.settings.SettingsStore
import io.github.facum.ezdisplay.settings.SettingsTheme
import io.github.facum.ezdisplay.settings.readAirActions

/** Pen and touch input settings, stored in [InputPrefs]; MainActivity re-reads them on resume. */
class SettingsActivity : ComponentActivity() {
    private lateinit var store: SettingsStore

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        store = SettingsStore.create(this)
        setContent { SettingsTheme { SettingsScreen(store) } }
    }

    override fun onResume() {
        super.onResume()
        // The user may have changed Samsung's Air actions while away.
        store.setAirActions(readAirActions(this))
    }
}

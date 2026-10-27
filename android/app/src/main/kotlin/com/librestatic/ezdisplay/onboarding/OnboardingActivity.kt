package com.librestatic.ezdisplay.onboarding

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import com.librestatic.ezdisplay.MainActivity
import com.librestatic.ezdisplay.settings.SettingsStore
import com.librestatic.ezdisplay.settings.SettingsTheme
import com.librestatic.ezdisplay.wifi.HostDiscovery

/**
 * The getting-started guide. Finishing or skipping marks it completed and opens [MainActivity]; a replay
 * (extra [EXTRA_REPLAY]) only closes, back to the settings it was opened from.
 */
class OnboardingActivity : ComponentActivity() {
    private lateinit var store: SettingsStore
    private lateinit var discovery: HostDiscovery

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        store = SettingsStore.create(this)
        discovery = HostDiscovery(this) { store.setDiscovered(it) }
        val replay = intent.getBooleanExtra(EXTRA_REPLAY, false)
        setContent { SettingsTheme { OnboardingScreen(store, onFinish = { finishGuide(replay) }) } }
    }

    private fun finishGuide(replay: Boolean) {
        OnboardingStore(this).markCompleted()
        if (!replay) {
            startActivity(Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP))
        }
        finish()
    }

    override fun onResume() {
        super.onResume()
        store.reloadHosts()
        discovery.start()
    }

    override fun onPause() {
        discovery.stop()
        super.onPause()
    }

    companion object {
        const val EXTRA_REPLAY = "replay"
    }
}

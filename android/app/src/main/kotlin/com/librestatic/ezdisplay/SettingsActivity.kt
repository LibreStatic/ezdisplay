package com.librestatic.ezdisplay

import android.content.Intent
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import com.librestatic.ezdisplay.settings.SettingsScreen
import com.librestatic.ezdisplay.settings.SettingsStore
import com.librestatic.ezdisplay.settings.SettingsTheme
import com.librestatic.ezdisplay.settings.readAirActions
import com.librestatic.ezdisplay.settings.wifiErrorText
import com.librestatic.ezdisplay.wifi.HostDiscovery
import com.librestatic.ezdisplay.wifi.isLocalAddress
import com.librestatic.ezdisplay.wifi.parsePairUri

/**
 * Pen and touch input settings, stored in [InputPrefs], and the Wi-Fi connection settings; MainActivity re-reads
 * them on resume. Also opens the `ezd://pair?...` code of the PC host (the system camera starts it): the pairing
 * dialog shows the host before anything is sent.
 */
class SettingsActivity : ComponentActivity() {
    private lateinit var store: SettingsStore
    private lateinit var discovery: HostDiscovery

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        store = SettingsStore.create(this)
        discovery = HostDiscovery(this) { store.setDiscovered(it) }
        if (savedInstanceState == null) handlePairIntent(intent)
        setContent { SettingsTheme { SettingsScreen(store) } }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handlePairIntent(intent)
    }

    private fun handlePairIntent(intent: Intent) {
        if (intent.action != Intent.ACTION_VIEW) return
        val link = parsePairUri(intent.dataString)
        if (link == null) {
            Toast.makeText(this, wifiErrorText("invalid_uri"), Toast.LENGTH_LONG).show()
            return
        }
        if (!isLocalAddress(link.addr)) {
            // A link can come from any web page or app: it may only point at the local network.
            Toast.makeText(this, wifiErrorText("not_local_address"), Toast.LENGTH_LONG).show()
            return
        }
        store.startPairing(link)
    }

    override fun onResume() {
        super.onResume()
        // The user may have changed Samsung's Air actions while away.
        store.setAirActions(readAirActions(this))
        // The host may have refused this device meanwhile, which forgets it.
        store.reloadHosts()
        discovery.start()
    }

    override fun onPause() {
        discovery.stop()
        super.onPause()
    }
}

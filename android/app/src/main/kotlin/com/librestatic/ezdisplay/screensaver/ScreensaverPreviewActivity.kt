package com.librestatic.ezdisplay.screensaver

import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.librestatic.ezdisplay.session.interceptKeys
import com.librestatic.ezdisplay.settings.SettingsTheme

/**
 * Shows the screensaver right away with the saved dim level, goes blank after [BLANK_AFTER_MS] to demonstrate it, and
 * closes on any touch or key. Started from the settings screen.
 */
class ScreensaverPreviewActivity : ComponentActivity() {
    private val handler = Handler(Looper.getMainLooper())
    private lateinit var controller: ScreensaverController
    private lateinit var config: ScreensaverConfig
    private val goBlank = Runnable { controller.apply(IdleState.Blank, config) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        WindowCompat.setDecorFitsSystemWindows(window, false)
        WindowInsetsControllerCompat(window, window.decorView).apply {
            systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            hide(WindowInsetsCompat.Type.systemBars())
        }
        window.interceptKeys {
            if (it.action == KeyEvent.ACTION_DOWN) finish()
            true
        }
        config = ScreensaverPrefs.read(this)
        controller = ScreensaverController(this, window)
        setContent {
            SettingsTheme {
                Box(Modifier.fillMaxSize().background(Color.Black)) {
                    ScreensaverOverlay(controller.state, speed = config.speed, motion = config.motion)
                }
            }
        }
        controller.apply(IdleState.Saver, config)
        handler.postDelayed(goBlank, BLANK_AFTER_MS)
    }

    override fun dispatchTouchEvent(ev: MotionEvent): Boolean {
        if (ev.actionMasked == MotionEvent.ACTION_DOWN) finish()
        return true
    }

    override fun onDestroy() {
        handler.removeCallbacks(goBlank)
        controller.release()
        super.onDestroy()
    }

    private companion object {
        const val BLANK_AFTER_MS = 10_000L
    }
}

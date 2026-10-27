package com.librestatic.ezdisplay.session

import android.view.KeyEvent
import android.view.Window

/**
 * Lets [handler] see every key event of this window before the activity does; when it returns false the event goes
 * on as usual. Used instead of overriding `ComponentActivity.dispatchKeyEvent`, which is restricted API.
 */
fun Window.interceptKeys(handler: (KeyEvent) -> Boolean) {
    val original = callback
    callback = object : Window.Callback by original {
        override fun dispatchKeyEvent(event: KeyEvent): Boolean = handler(event) || original.dispatchKeyEvent(event)
    }
}

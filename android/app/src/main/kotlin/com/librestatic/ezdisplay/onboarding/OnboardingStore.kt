package com.librestatic.ezdisplay.onboarding

import android.content.Context
import android.content.SharedPreferences
import androidx.core.content.edit
import com.librestatic.ezdisplay.InputPrefs

/** Bump when the guide gains content worth showing again to people who already finished it. */
const val ONBOARDING_VERSION = 1

/**
 * Remembers which version of the getting-started guide was completed, in the `input` prefs.
 * An install that already has settings but no record (an app update) counts as completed and is
 * not shown the guide.
 */
class OnboardingStore(
    private val prefs: SharedPreferences,
    private val version: Int = ONBOARDING_VERSION,
) {
    constructor(context: Context) : this(InputPrefs.prefs(context))

    fun isCompleted(): Boolean {
        if (prefs.getInt(KEY, 0) >= version) return true
        if (!prefs.contains(KEY) && hasExistingSettings()) {
            markCompleted()
            return true
        }
        return false
    }

    fun markCompleted() = prefs.edit { putInt(KEY, version) }

    fun reset() = prefs.edit { remove(KEY) }

    /** Any other pref means the app was used before the guide existed. The panel cache is written on first launch. */
    private fun hasExistingSettings(): Boolean = prefs.all.keys.any { it != KEY && it != InputPrefs.PANEL_TYPE_DETECTED }

    companion object {
        const val KEY = "onboarding_version"

        /** True when the guide has to be shown before the app starts. */
        fun shouldShow(context: Context): Boolean = !OnboardingStore(context).isCompleted()
    }
}

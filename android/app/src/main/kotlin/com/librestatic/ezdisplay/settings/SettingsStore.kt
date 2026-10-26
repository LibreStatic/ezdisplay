package com.librestatic.ezdisplay.settings

import android.content.Context
import android.content.SharedPreferences
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import androidx.core.content.edit
import androidx.compose.runtime.State
import androidx.compose.runtime.mutableStateOf
import com.librestatic.ezdisplay.InputPrefs
import com.librestatic.ezdisplay.InputPrefs.KeyCombo
import com.librestatic.ezdisplay.R
import com.librestatic.ezdisplay.wifi.DiscoveredHost
import com.librestatic.ezdisplay.wifi.NativeWifiBackend
import com.librestatic.ezdisplay.wifi.PairLink
import com.librestatic.ezdisplay.wifi.PairResult
import com.librestatic.ezdisplay.wifi.WifiBackend
import com.librestatic.ezdisplay.wifi.WifiHost
import com.librestatic.ezdisplay.wifi.isValidPin
import java.util.concurrent.Executor
import java.util.concurrent.Executors
import kotlin.math.roundToInt

/** State of the Samsung S Pen "Air actions" system setting. */
enum class AirActions { NotSamsung, Off, On, Unknown }

/** A custom-profile trigger: its pref [key], [label], [default] action and the [actions] it accepts. */
class Trigger(val key: String, val label: Int, val default: String, val actions: List<String>)

/**
 * One key-combo editor: where it is stored ([prefKey]), its [title], and the Custom [triggerKeys] that enable it
 * when any of them is set to one of [actions].
 */
class KeyEditorSpec(
    val prefKey: String,
    val title: Int,
    val triggerKeys: List<String>,
    val actions: Set<String> = setOf("keys"),
) {
    constructor(prefKey: String, title: Int, triggerKey: String) : this(prefKey, title, listOf(triggerKey))
}

/** A profile choice: pref [value], [title] and [desc] string resources and its [icon] drawable. */
data class ProfileSpec(val value: String, val title: Int, val desc: Int, val icon: Int)

/** A key-combo preset with its [label] string resource. */
data class KeyPreset(val label: Int, val combo: KeyCombo)

val ActionLabels = mapOf(
    "raw" to R.string.action_raw,
    "right_click" to R.string.action_right_click,
    "middle_click" to R.string.action_middle_click,
    "local_zoom" to R.string.action_local_zoom,
    "eraser" to R.string.action_eraser,
    "scroll" to R.string.action_scroll,
    "pc_zoom" to R.string.action_pc_zoom,
    "step_zoom" to R.string.action_step_zoom,
    "wheel_zoom" to R.string.action_wheel_zoom,
    "pan" to R.string.action_pan,
    "pan_zoom" to R.string.action_pan_zoom,
    "none" to R.string.action_none,
    "keys" to R.string.action_keys,
)

val Profiles = listOf(
    ProfileSpec("raw", R.string.profile_raw, R.string.profile_raw_desc, R.drawable.ic_ads_click),
    ProfileSpec("desktop", R.string.profile_desktop, R.string.profile_desktop_desc, R.drawable.ic_desktop_windows),
    ProfileSpec("drawing", R.string.profile_drawing, R.string.profile_drawing_desc, R.drawable.ic_draw),
    ProfileSpec("graphics_tablet", R.string.profile_graphics_tablet, R.string.profile_graphics_tablet_desc, R.drawable.ic_stylus),
    ProfileSpec("custom", R.string.profile_custom, R.string.profile_custom_desc, R.drawable.ic_tune),
)

val Triggers = listOf(
    Trigger(InputPrefs.CUSTOM_CLICK, R.string.trigger_click, "raw",
        listOf("raw", "right_click", "middle_click", "local_zoom", "keys", "none")),
    Trigger(InputPrefs.CUSTOM_DOUBLE_CLICK, R.string.trigger_double_click, "local_zoom",
        listOf("right_click", "middle_click", "local_zoom", "keys", "none")),
    Trigger(InputPrefs.CUSTOM_HOLD, R.string.trigger_hold, "raw",
        listOf("raw", "right_click", "middle_click", "local_zoom", "eraser", "keys", "none")),
    Trigger(InputPrefs.CUSTOM_DOUBLE_HOLD, R.string.trigger_double_hold, "none",
        listOf("eraser", "keys", "none")),
    Trigger(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT, R.string.trigger_hold_drag_contact, "raw",
        listOf("raw", "scroll", "pc_zoom", "step_zoom", "wheel_zoom", "pan", "pan_zoom", "none")),
    Trigger(InputPrefs.CUSTOM_HOLD_DRAG_HOVER, R.string.trigger_hold_drag_hover, "raw",
        listOf("raw", "scroll", "pc_zoom", "step_zoom", "wheel_zoom", "none")),
)

/** In display order: click, double click, hold, double click + hold, zoom in, zoom out, pan (the last three follow the drags). */
val KeyEditors = listOf(
    KeyEditorSpec(InputPrefs.KEYS_CLICK, R.string.keys_title_click, InputPrefs.CUSTOM_CLICK),
    KeyEditorSpec(InputPrefs.KEYS_DOUBLE_CLICK, R.string.keys_title_double_click, InputPrefs.CUSTOM_DOUBLE_CLICK),
    KeyEditorSpec(InputPrefs.KEYS_HOLD, R.string.keys_title_hold, InputPrefs.CUSTOM_HOLD),
    KeyEditorSpec(InputPrefs.KEYS_DOUBLE_HOLD, R.string.keys_title_double_hold, InputPrefs.CUSTOM_DOUBLE_HOLD),
    KeyEditorSpec(
        InputPrefs.KEYS_ZOOM_IN, R.string.keys_title_zoom_in,
        listOf(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT, InputPrefs.CUSTOM_HOLD_DRAG_HOVER), setOf("step_zoom"),
    ),
    KeyEditorSpec(
        InputPrefs.KEYS_ZOOM_OUT, R.string.keys_title_zoom_out,
        listOf(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT, InputPrefs.CUSTOM_HOLD_DRAG_HOVER), setOf("step_zoom"),
    ),
    KeyEditorSpec(
        InputPrefs.KEYS_PAN, R.string.keys_title_pan,
        listOf(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT), setOf("pan", "pan_zoom"),
    ),
)

val KeyPresets = listOf(
    KeyPreset(R.string.keys_preset_ctrl_space, KeyCombo(InputPrefs.MOD_CTRL, 0x2C)),
    KeyPreset(R.string.keys_preset_space, KeyCombo(0, 0x2C)),
    KeyPreset(R.string.keys_preset_alt_space, KeyCombo(InputPrefs.MOD_ALT, 0x2C)),
    KeyPreset(R.string.keys_preset_ctrl_alt_space, KeyCombo(InputPrefs.MOD_CTRL or InputPrefs.MOD_ALT, 0x2C)),
    KeyPreset(R.string.keys_preset_ctrl_z, KeyCombo(InputPrefs.MOD_CTRL, 0x1D)),
    KeyPreset(R.string.keys_preset_ctrl_shift_z, KeyCombo(InputPrefs.MOD_CTRL or InputPrefs.MOD_SHIFT, 0x1D)),
)

/** Everything the settings screen shows. Defaults match what [InputPrefs.toJson] uses for unset prefs. */
data class SettingsState(
    val profile: String = "raw",
    /** Trigger pref key (`InputPrefs.CUSTOM_*`) to action. */
    val custom: Map<String, String> = Triggers.associate { it.key to it.default },
    /** Key editor pref key (`InputPrefs.KEYS_*`) to combo. */
    val keys: Map<String, KeyCombo> = defaultKeys(),
    val zoomFactor: Float = 2f,
    val doubleClickMs: Int = 300,
    val palmRejection: Boolean = true,
    val airActions: AirActions = AirActions.NotSamsung,
    /** Play the PC's audio on this device. */
    val audioEnabled: Boolean = true,
    /** Connect over Wi-Fi when the USB connection is not there. */
    val wifiEnabled: Boolean = false,
    val wifiHosts: List<WifiHost> = emptyList(),
    /** Hosts seen on the network right now (only while discovery runs). */
    val discovered: List<DiscoveredHost> = emptyList(),
    /** This device's short certificate fingerprint, for the host to show; empty until known. */
    val deviceFingerprint: String = "",
    /** The pairing dialog, when one is open. */
    val pairing: PairingState? = null,
)

/**
 * A pairing in progress. [link] is set when it comes from a scanned code (no PIN to type: the code has it);
 * [shortFp] is empty for a manually typed address (the host's fingerprint is not known yet).
 */
data class PairingState(
    val name: String,
    val shortFp: String,
    val addr: String,
    val link: PairLink? = null,
    val busy: Boolean = false,
    /** Error code of the last attempt (see `wifiErrorText`). */
    val error: String? = null,
)

private fun defaultKeys(): Map<String, KeyCombo> = mapOf(
    InputPrefs.KEYS_CLICK to InputPrefs.DEFAULT_KEYS_CLICK,
    InputPrefs.KEYS_DOUBLE_CLICK to InputPrefs.DEFAULT_KEYS_DOUBLE_CLICK,
    InputPrefs.KEYS_HOLD to InputPrefs.DEFAULT_KEYS_HOLD,
    InputPrefs.KEYS_DOUBLE_HOLD to InputPrefs.DEFAULT_KEYS_DOUBLE_HOLD,
    InputPrefs.KEYS_ZOOM_IN to InputPrefs.DEFAULT_KEYS_ZOOM_IN,
    InputPrefs.KEYS_ZOOM_OUT to InputPrefs.DEFAULT_KEYS_ZOOM_OUT,
    InputPrefs.KEYS_PAN to InputPrefs.DEFAULT_KEYS_PAN,
)

/** KEYS_* pref keys whose editor is shown: graphics tablet shows hold and double click + hold; Custom shows those whose trigger action is `keys` (the zoom shortcuts: a drag set to `step_zoom`). */
fun visibleKeyEditors(state: SettingsState): List<String> = when (state.profile) {
    "graphics_tablet" -> listOf(InputPrefs.KEYS_HOLD, InputPrefs.KEYS_DOUBLE_HOLD)
    "custom" -> KeyEditors.filter { e ->
        // The drag triggers are unused while hold sends keys.
        e.triggerKeys.any { state.custom[it] in e.actions && !(it in DragTriggerKeys && dragsDisabled(state)) }
    }.map { it.prefKey }
    else -> emptyList()
}

/** The hold-drag triggers (pref keys). */
val DragTriggerKeys = setOf(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT, InputPrefs.CUSTOM_HOLD_DRAG_HOVER)

/** The hold-drag triggers are unused while Custom hold sends keys. */
fun dragsDisabled(state: SettingsState): Boolean =
    state.profile == "custom" && state.custom[InputPrefs.CUSTOM_HOLD] == "keys"

/** Index into [KeyPresets] matching [combo], or -1. */
fun presetIndex(combo: KeyCombo): Int = KeyPresets.indexOfFirst { it.combo == combo }

/** Rounds [raw] to the nearest [InputPrefs.DOUBLE_CLICK_STEP], clamped to the allowed range. */
fun roundDoubleClickMs(raw: Float): Int {
    val step = InputPrefs.DOUBLE_CLICK_STEP
    return ((raw / step).roundToInt() * step).coerceIn(InputPrefs.DOUBLE_CLICK_MIN, InputPrefs.DOUBLE_CLICK_MAX)
}

/** A stored zoom factor that is not one of [InputPrefs.ZOOM_FACTORS] counts as 2x. */
fun normalizeZoomFactor(f: Float): Float = if (f in InputPrefs.ZOOM_FACTORS) f else 2f

/** Reads the Samsung S Pen "Air actions" system setting. */
fun readAirActions(context: Context): AirActions {
    if (!Build.MANUFACTURER.equals("samsung", ignoreCase = true)) return AirActions.NotSamsung
    val v = try {
        Settings.System.getInt(context.contentResolver, "spen_air_action", -1)
    } catch (e: Exception) {
        -1
    }
    return when (v) {
        0 -> AirActions.Off
        1 -> AirActions.On
        else -> AirActions.Unknown
    }
}

/** What the settings UI can change; lets previews and tests run without prefs. */
interface SettingsActions {
    fun setProfile(profile: String)
    fun setCustomAction(triggerKey: String, action: String)
    fun setKeyCombo(prefKey: String, combo: KeyCombo)
    fun toggleModifier(prefKey: String, bit: Int)
    fun setKey(prefKey: String, usage: Int)
    fun setZoomFactor(factor: Float)
    fun setDoubleClickMs(ms: Int)
    fun setPalmRejection(on: Boolean)
    fun setWifiEnabled(on: Boolean)
    fun setAudioEnabled(on: Boolean)

    /** Opens the PIN dialog for a host that was found or typed. */
    fun startPairing(name: String, shortFp: String, addr: String)

    /** Opens the confirmation for a scanned pairing code. */
    fun startPairing(link: PairLink)

    /** Pairs with the open dialog's host; [pin] is ignored for a scanned code. */
    fun submitPairing(pin: String)
    fun cancelPairing()
    fun forgetHost(fingerprint: String)
}

/** Holds [SettingsState] backed by the `input` prefs; every action persists at once with `apply()`. */
class SettingsStore(
    private val prefs: SharedPreferences,
    airActions: AirActions,
    private val wifi: WifiBackend = WifiBackend.None,
    private val deviceName: String = Build.MODEL,
    /** Runs the blocking pairing; replaced by a direct executor in tests. */
    private val io: Executor = Executors.newSingleThreadExecutor(),
) : SettingsActions {
    private val main = Handler(Looper.getMainLooper())
    private val _state = mutableStateOf(read(airActions))
    val state: State<SettingsState> get() = _state

    private fun read(airActions: AirActions) = SettingsState(
        profile = InputPrefs.profile(prefs),
        custom = Triggers.associate { it.key to (prefs.getString(it.key, it.default) ?: it.default) },
        keys = defaultKeys().mapValues { (k, d) -> KeyCombo.parse(prefs.getString(k, null), d) },
        zoomFactor = normalizeZoomFactor(prefs.getFloat(InputPrefs.ZOOM_FACTOR, 2f)),
        doubleClickMs = prefs.getInt(InputPrefs.DOUBLE_CLICK_MS, 300)
            .coerceIn(InputPrefs.DOUBLE_CLICK_MIN, InputPrefs.DOUBLE_CLICK_MAX),
        palmRejection = prefs.getBoolean(InputPrefs.PALM_REJECTION, true),
        airActions = airActions,
        audioEnabled = prefs.getBoolean(AUDIO_ENABLED, true),
        wifiEnabled = prefs.getBoolean(WIFI_ENABLED, false),
        wifiHosts = wifi.hosts(),
        deviceFingerprint = wifi.identity(),
    )

    override fun setProfile(profile: String) {
        if (profile !in InputPrefs.PROFILES) return
        prefs.edit { putString(InputPrefs.PROFILE, profile) }
        _state.value = _state.value.copy(profile = profile)
    }

    /** Re-reads the system setting, e.g. after the user comes back from Samsung's settings. */
    fun setAirActions(airActions: AirActions) {
        _state.value = _state.value.copy(airActions = airActions)
    }

    /** Ignored when [action] is not allowed for the trigger. */
    override fun setCustomAction(triggerKey: String, action: String) {
        val t = Triggers.firstOrNull { it.key == triggerKey } ?: return
        if (action !in t.actions) return
        prefs.edit { putString(triggerKey, action) }
        _state.value = _state.value.copy(custom = _state.value.custom + (triggerKey to action))
    }

    override fun setKeyCombo(prefKey: String, combo: KeyCombo) {
        if (prefKey !in _state.value.keys) return
        prefs.edit { putString(prefKey, combo.format()) }
        _state.value = _state.value.copy(keys = _state.value.keys + (prefKey to combo))
    }

    /** Flips modifier [bit] (`InputPrefs.MOD_*`) of the combo stored under [prefKey]. */
    override fun toggleModifier(prefKey: String, bit: Int) {
        val c = _state.value.keys[prefKey] ?: return
        setKeyCombo(prefKey, c.copy(mods = c.mods xor bit))
    }

    override fun setKey(prefKey: String, usage: Int) {
        val c = _state.value.keys[prefKey] ?: return
        setKeyCombo(prefKey, c.copy(usage = usage))
    }

    override fun setZoomFactor(factor: Float) {
        val f = normalizeZoomFactor(factor)
        prefs.edit { putFloat(InputPrefs.ZOOM_FACTOR, f) }
        _state.value = _state.value.copy(zoomFactor = f)
    }

    /** [ms] is the already rounded value (see [roundDoubleClickMs]). */
    override fun setDoubleClickMs(ms: Int) {
        val v = ms.coerceIn(InputPrefs.DOUBLE_CLICK_MIN, InputPrefs.DOUBLE_CLICK_MAX)
        prefs.edit { putInt(InputPrefs.DOUBLE_CLICK_MS, v) }
        _state.value = _state.value.copy(doubleClickMs = v)
    }

    override fun setPalmRejection(on: Boolean) {
        prefs.edit { putBoolean(InputPrefs.PALM_REJECTION, on) }
        _state.value = _state.value.copy(palmRejection = on)
    }

    override fun setAudioEnabled(on: Boolean) {
        prefs.edit { putBoolean(AUDIO_ENABLED, on) }
        _state.value = _state.value.copy(audioEnabled = on)
    }

    override fun setWifiEnabled(on: Boolean) {
        prefs.edit { putBoolean(WIFI_ENABLED, on) }
        _state.value = _state.value.copy(wifiEnabled = on)
    }

    /** Hosts seen on the network changed. */
    fun setDiscovered(hosts: List<DiscoveredHost>) {
        _state.value = _state.value.copy(discovered = hosts)
    }

    /** Re-reads the paired hosts (they live in the native state, not in the prefs). */
    fun reloadHosts() {
        _state.value = _state.value.copy(wifiHosts = wifi.hosts(), deviceFingerprint = wifi.identity())
    }

    override fun startPairing(name: String, shortFp: String, addr: String) {
        _state.value = _state.value.copy(pairing = PairingState(name, shortFp, addr))
    }

    override fun startPairing(link: PairLink) {
        _state.value = _state.value.copy(
            pairing = PairingState(link.name.ifEmpty { link.addr }, link.shortFp, link.addr, link),
        )
    }

    override fun cancelPairing() {
        _state.value = _state.value.copy(pairing = null)
    }

    override fun submitPairing(pin: String) {
        val p = _state.value.pairing ?: return
        if (p.busy) return
        val input = p.link?.let { it.toUri() } ?: pin.trim()
        if (p.link == null && !isValidPin(input)) {
            _state.value = _state.value.copy(pairing = p.copy(error = "invalid_pin"))
            return
        }
        _state.value = _state.value.copy(pairing = p.copy(busy = true, error = null))
        io.execute {
            val result = wifi.pair(p.addr, input, deviceName)
            main.post { pairingDone(p, result) }
        }
    }

    private fun pairingDone(p: PairingState, result: PairResult) {
        // Ignore the answer of a dialog that was cancelled or replaced meanwhile (the host is paired anyway).
        val current = _state.value.pairing
        val hosts = wifi.hosts()
        if (current?.addr != p.addr) {
            _state.value = _state.value.copy(wifiHosts = hosts)
            return
        }
        _state.value = if (result.ok) {
            // Pairing means the user wants to use it: turn Wi-Fi on.
            prefs.edit { putBoolean(WIFI_ENABLED, true) }
            _state.value.copy(pairing = null, wifiHosts = hosts, wifiEnabled = true)
        } else {
            _state.value.copy(pairing = current.copy(busy = false, error = result.error ?: "other"))
        }
    }

    override fun forgetHost(fingerprint: String) {
        wifi.forget(fingerprint)
        _state.value = _state.value.copy(wifiHosts = wifi.hosts())
    }

    companion object {
        /** Pref key of the Wi-Fi switch, read by MainActivity. */
        const val WIFI_ENABLED = "wifi_enabled"

        /** Pref key of the audio switch, read by MainActivity when a session starts. */
        const val AUDIO_ENABLED = "audio_enabled"

        fun create(context: Context) = SettingsStore(
            InputPrefs.prefs(context), readAirActions(context), NativeWifiBackend(context),
        )
    }
}

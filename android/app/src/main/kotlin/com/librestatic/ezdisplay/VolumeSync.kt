package com.librestatic.ezdisplay

import android.content.Context
import android.database.ContentObserver
import android.media.AudioManager
import android.os.Handler
import android.provider.Settings
import android.util.Log
import kotlin.math.roundToInt

/** The tablet's media volume as the platform exposes it. */
interface MusicVolume {
    val max: Int
    var index: Int
    var muted: Boolean

    /**
     * Moves the volume one step ([AudioManager.ADJUST_RAISE] or `ADJUST_LOWER`) or toggles mute.
     *
     * Whole steps through [index], not `adjustStreamVolume`: some devices move a finer hidden
     * scale on each press, which the step the PC follows only sees every few presses. No flag
     * shows the system volume panel either: its blur over the video surface stalls the decoder
     * for as long as the panel is up.
     */
    fun adjust(direction: Int) {
        when (direction) {
            AudioManager.ADJUST_TOGGLE_MUTE -> muted = !muted
            else -> index = (index + direction).coerceIn(0, max)
        }
    }
}

/** [MusicVolume] on top of [AudioManager]'s `STREAM_MUSIC`. */
class AudioManagerVolume(private val audio: AudioManager) : MusicVolume {
    override val max: Int get() = audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC).coerceAtLeast(1)

    override var index: Int
        get() = audio.getStreamVolume(AudioManager.STREAM_MUSIC)
        set(value) = audio.setStreamVolume(AudioManager.STREAM_MUSIC, value, 0)

    override var muted: Boolean
        get() = audio.isStreamMute(AudioManager.STREAM_MUSIC)
        set(value) = audio.adjustStreamVolume(
            AudioManager.STREAM_MUSIC,
            if (value) AudioManager.ADJUST_MUTE else AudioManager.ADJUST_UNMUTE,
            0,
        )
}

/** Perceptual volume (0..1, what a slider shows) of a volume step. */
fun volumeOfIndex(index: Int, max: Int): Float = (index.toFloat() / max.coerceAtLeast(1)).coerceIn(0f, 1f)

/** The volume step closest to the perceptual [volume]. */
fun indexOfVolume(volume: Float, max: Int): Int =
    (volume.coerceIn(0f, 1f) * max).roundToInt().coerceIn(0, max)

/**
 * Keeps the tablet's media volume and the PC's volume in step (protocol 0.7 `AudioVolume`).
 *
 * The native layer holds the tablet's value and sends it to the host once audio starts, then on
 * every change; a volume received from the host is polled with [pollHost] and applied to the
 * hardware volume. To avoid sending a host change back, this class remembers the state it last
 * saw (applied or reported) as a volume step plus the mute flag and reports only a different
 * one: this is the only place that suppresses echoes.
 */
class VolumeSync(
    private val music: MusicVolume,
    private val report: (volume: Float, muted: Boolean) -> Unit,
    private val pollHostVolume: () -> FloatArray,
) {
    private var knownIndex = -1
    private var knownMuted = false
    private var hostSeq = 0f

    /** Starts a session: reports the current volume (kept natively until audio starts). */
    fun begin() {
        hostSeq = 0f
        knownIndex = -1
        onLocalChange()
    }

    /** The tablet's volume may have changed (volume keys, the system panel, another app). */
    fun onLocalChange() {
        val index = music.index
        val muted = music.muted
        if (index == knownIndex && muted == knownMuted) return
        knownIndex = index
        knownMuted = muted
        report(volumeOfIndex(index, music.max), muted)
    }

    /** A volume key pressed on the tablet: changes the volume without the system panel and reports it. */
    fun onKey(direction: Int) {
        try {
            music.adjust(direction)
        } catch (e: SecurityException) {
            Log.w(TAG, "cannot set the media volume", e)
        }
        onLocalChange()
    }

    /** Applies a volume the host sent since the last call. */
    fun pollHost() {
        val v = pollHostVolume()
        if (v.size < 3 || v[0] == 0f || v[0] == hostSeq) return
        hostSeq = v[0]
        val index = indexOfVolume(v[1], music.max)
        val muted = v[2] != 0f
        // Remember before applying so the change notification that follows is not reported.
        knownIndex = index
        knownMuted = muted
        try {
            if (music.index != index) music.index = index
            // Setting a volume can unmute, so the mute state goes last.
            if (music.muted != muted) music.muted = muted
        } catch (e: SecurityException) {
            Log.w(TAG, "cannot set the media volume", e)
        }
    }

    private companion object {
        const val TAG = "VolumeSync"
    }
}

/** Runs [sync]'s local check whenever the system settings (where the volumes live) change. */
class VolumeWatcher(private val context: Context, handler: Handler, private val sync: VolumeSync) {
    private val observer = object : ContentObserver(handler) {
        override fun onChange(selfChange: Boolean) = sync.onLocalChange()
    }
    private var registered = false

    /** Registers the observer (once) and reports the current volume. */
    fun start() {
        if (!registered) {
            context.contentResolver.registerContentObserver(Settings.System.CONTENT_URI, true, observer)
            registered = true
        }
        sync.begin()
    }

    fun stop() {
        if (registered) {
            context.contentResolver.unregisterContentObserver(observer)
            registered = false
        }
    }

    fun pollHost() = sync.pollHost()

    fun onKey(direction: Int) = sync.onKey(direction)

    /** Safety net for devices that do not notify through the settings provider. */
    fun checkLocal() = sync.onLocalChange()
}

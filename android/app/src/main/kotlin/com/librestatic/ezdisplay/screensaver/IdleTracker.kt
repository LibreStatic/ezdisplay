package com.librestatic.ezdisplay.screensaver

/** What the screen is doing: showing the stream, dimmed with drifting shapes, or fully black. */
enum class IdleState { Active, Saver, Blank }

/**
 * Decides when the screensaver starts and stops. Pure Kotlin: the caller passes the clock (milliseconds, any
 * monotonic base) so it can be tested with a fake one.
 *
 * - Active -> Saver after `idleMin` without local input and without new frames.
 * - Saver -> Blank after `blankMin` in Saver (0 = never).
 * - In Saver a new frame or any local input goes back to Active; in Blank only local input does.
 * - A disabled config or a disarmed tracker is always Active, with the timers reset.
 */
class IdleTracker(config: ScreensaverConfig, nowMs: Long = 0) {
    var config: ScreensaverConfig = config
        private set
    var state: IdleState = IdleState.Active
        private set
    private var armed = false
    private var lastActivityMs = nowMs
    private var saverSinceMs = nowMs
    private var lastFrames = -1L

    private val running get() = armed && config.enabled

    /** Applies new settings; the timers restart. */
    fun setConfig(config: ScreensaverConfig, nowMs: Long) {
        this.config = config
        reset(nowMs)
    }

    /** Armed while the activity is resumed. Disarming (or arming) restarts the timers. */
    fun setArmed(armed: Boolean, nowMs: Long) {
        this.armed = armed
        reset(nowMs)
    }

    /**
     * Touch, pen, mouse or key input. Returns true when it woke the screen from Saver or Blank: that input
     * (the whole gesture) must not be forwarded to the host.
     */
    fun onLocalInput(nowMs: Long): Boolean {
        if (!running) return false
        val woke = state != IdleState.Active
        state = IdleState.Active
        lastActivityMs = nowMs
        return woke
    }

    /** [total] is the running count of received frames; only an increase counts as activity. */
    fun onFramesReceived(total: Long, nowMs: Long) {
        val previous = lastFrames
        lastFrames = total
        if (!running || previous < 0 || total <= previous) return
        when (state) {
            IdleState.Active -> lastActivityMs = nowMs
            IdleState.Saver -> {
                state = IdleState.Active
                lastActivityMs = nowMs
            }
            IdleState.Blank -> Unit // only local input wakes a blank screen
        }
    }

    /** Forget the frame counter, e.g. when a new session starts and counts from zero. */
    fun resetFrames() {
        lastFrames = -1
    }

    fun tick(nowMs: Long) {
        if (!running) return
        when (state) {
            IdleState.Active ->
                if (nowMs - lastActivityMs >= config.idleMin * MINUTE_MS) {
                    state = IdleState.Saver
                    saverSinceMs = nowMs
                }
            IdleState.Saver ->
                if (config.blankMin > 0 && nowMs - saverSinceMs >= config.blankMin * MINUTE_MS) {
                    state = IdleState.Blank
                }
            IdleState.Blank -> Unit
        }
    }

    private fun reset(nowMs: Long) {
        state = IdleState.Active
        lastActivityMs = nowMs
        saverSinceMs = nowMs
    }

    private companion object {
        const val MINUTE_MS = 60_000L
    }
}

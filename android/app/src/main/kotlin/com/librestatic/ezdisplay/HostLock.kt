package com.librestatic.ezdisplay

/** Pure rules for a host whose screen is locked (the native layer reports the code `screen_locked`). */
object HostLock {
    /** Status code the native layer sets when the host ends the session because its screen is locked. */
    const val CODE = "screen_locked"

    /** Fixed delay between reconnection attempts while the host is locked. */
    const val RETRY_MS = 5000L

    /** Whether an ended session carrying [code] means the host's screen is locked. */
    fun isLocked(code: String?): Boolean = code == CODE

    /** Whether a negotiated stream shows that the host was unlocked: only once a frame has arrived. */
    fun confirmsUnlock(framesReceived: Long): Boolean = framesReceived > 0

    /** Whether the connecting and handshake texts may replace the status; the locked message stays up. */
    fun showsProgress(hostLocked: Boolean): Boolean = !hostLocked
}

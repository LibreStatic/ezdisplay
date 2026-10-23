package io.github.facum.ezdisplay

import android.view.Surface

/**
 * JNI bridge to libezd_android.so (crates/ezd-android).
 *
 * Native threads never call back into the JVM: everything is polled, preferably once per
 * Choreographer frame.
 */
object NativeBridge {
    init {
        System.loadLibrary("ezd_android")
    }

    /** Starts one connection attempt to [host]:[port]. Returns null on success or an error message. */
    @JvmStatic external fun start(surface: Surface?, host: String, port: Int, capsJson: String): String?

    /** Stops the session; the decoder is released before this returns. */
    @JvmStatic external fun stop()

    /** A (new) output surface is available. */
    @JvmStatic external fun surfaceChanged(surface: Surface)

    /** The surface is going away; blocks until the decoder stopped using it. */
    @JvmStatic external fun surfaceDestroyed()

    @JvmStatic external fun requestKeyframe()

    /** `[seq, x, y, visible (0/1), shapeId]` in stream pixels; `seq == 0` means no cursor yet. */
    @JvmStatic external fun pollCursor(): LongArray

    /**
     * Cursor bitmap: width, height, hotX, hotY as u16 little endian followed by straight RGBA8.
     * Null if the shape is unknown.
     */
    @JvmStatic external fun takeCursorShape(shapeId: Int): ByteArray?

    /** Session phase, stream info and statistics as JSON. */
    @JvmStatic external fun pollStats(): String
}

package com.librestatic.ezdisplay

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

    /**
     * Forwards one [android.view.MotionEvent] to the input engine. Packing (arrays may be longer
     * than needed, only what the header says is read):
     * - [ints]: `[actionMasked, actionIndex, buttonState, flags, pointerCount, historySize]`,
     *   then per pointer `[pointerId, toolType]`.
     * - [floats]: `[vscroll, hscroll]`, then for each step (history oldest first, then the
     *   current one: `historySize + 1` steps) and each pointer `[x, y, pressure, tilt, orientation]`.
     * - [timesNs]: one event time per step, same order, in nanoseconds on the
     *   `SystemClock.uptimeMillis` (CLOCK_MONOTONIC) base.
     */
    @JvmStatic external fun inputEvent(ints: IntArray, floats: FloatArray, timesNs: LongArray)

    /** Fires the engine timers that are due; call once per frame. */
    @JvmStatic external fun inputTick()

    /**
     * `[scale, tx, ty, penX, penY]`: local zoom to apply to the stage (pivot at 0,0) and the pen
     * position normalized 0..1 over the stream, or -1 while the pen is out of range.
     */
    @JvmStatic external fun pollView(): FloatArray

    /** Applies the input configuration JSON. Null on success, else the error (old config kept). */
    @JvmStatic external fun setInputConfig(json: String): String?

    /** Size in pixels of the view that receives input (the zoomed stage). */
    @JvmStatic external fun setViewSize(width: Int, height: Int)
}

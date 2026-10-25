package com.librestatic.ezdisplay

import android.app.Activity
import android.graphics.Bitmap
import android.graphics.Color
import android.graphics.Rect
import android.graphics.Typeface
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.util.TypedValue
import android.view.Choreographer
import android.view.Gravity
import android.view.InputDevice
import android.view.MotionEvent
import android.view.PointerIcon
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.view.ViewGroup
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.ImageButton
import android.widget.ImageView
import android.widget.TextView
import org.json.JSONObject
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.Locale

/**
 * Fullscreen display client: SurfaceView fed by the native MediaCodec decoder, a locally drawn
 * cursor above it, a status line and a telemetry overlay (shown while the host enables it).
 *
 * While streaming, every pointer event is forwarded to the native input engine, which sends
 * touch/pen input to the host and drives the local zoom of the [stage] (surface + cursor).
 * Outside of streaming a settings button is shown.
 */
class MainActivity : Activity(), SurfaceHolder.Callback, Choreographer.FrameCallback {
    private companion object {
        const val TAG = "ezdisplay"
        const val HOST = "127.0.0.1"
        const val PORT = 27183
        const val BACKOFF_MIN_MS = 1000L
        const val BACKOFF_MAX_MS = 5000L
        const val STATS_INTERVAL_MS = 250L
        const val STATUS_HIDE_MS = 3000L
        const val NOTE_SHOW_MS = 6000L
        const val AIR_HINT_SHOW_MS = 8000L
        const val AIR_FOCUS_LOSS_MS = 1000L
        const val BARREL_MASK = MotionEvent.BUTTON_STYLUS_PRIMARY or MotionEvent.BUTTON_STYLUS_SECONDARY
        const val INT_HEADER = 6
        const val FLOAT_HEADER = 2
    }

    private lateinit var root: FrameLayout
    private lateinit var stage: FrameLayout
    private lateinit var settingsButton: ImageButton
    private lateinit var surfaceView: SurfaceView
    private lateinit var cursorView: ImageView
    private lateinit var statusView: TextView
    private lateinit var telemetryView: TextView

    private val handler = Handler(Looper.getMainLooper())
    private var chosenMode: android.view.Display.Mode? = null
    private var capsJson: String? = null

    private var started = false // between onStart and onStop
    private var surfaceReady = false
    private var sessionActive = false // NativeBridge.start succeeded and was not stopped
    private var backoffMs = BACKOFF_MIN_MS
    private var wasStreaming = false
    private var lastStatsPoll = 0L
    private var streamW = 0
    private var streamH = 0
    private var bufW = 0
    private var bufH = 0
    private var frameRateSet = 0
    private var telemetryOn = false
    private var lastInputNote = ""
    private var keysHintShown = false
    private var airHintShown = false
    private var barrelDown = false
    private var barrelPressAt = 0L
    private var stageScale = 1f
    private var stageTx = 0f
    private var stageTy = 0f
    private var viewW = 0
    private var viewH = 0

    private var intBuf = IntArray(64)
    private var floatBuf = FloatArray(512)
    private var timeBuf = LongArray(64)
    private var statusHideAt = 0L

    private var cursorSeq = 0L
    private var cursorShapeId = -1
    private var cursorHotX = 0
    private var cursorHotY = 0
    private var cursorBmpW = 0
    private var cursorBmpH = 0
    private var cursorScaleX = 0f
    private var cursorScaleY = 0f
    private var cursorVisible = false

    private val connectRunnable = Runnable { connect() }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        if (Build.VERSION.SDK_INT >= 28) {
            window.attributes = window.attributes.also {
                it.layoutInDisplayCutoutMode =
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
            }
        }
        pickDisplayMode()
        buildUi()
        hideSystemBars()
    }

    private fun buildUi() {
        root = FrameLayout(this).apply {
            setBackgroundColor(Color.BLACK)
            // Our cursor is drawn locally; hide the system hover icon.
            pointerIcon = PointerIcon.getSystemIcon(this@MainActivity, PointerIcon.TYPE_NULL)
            addOnLayoutChangeListener { _, l, t, r, b, _, _, _, _ ->
                if (r - l != viewW || b - t != viewH) {
                    viewW = r - l
                    viewH = b - t
                    NativeBridge.setViewSize(viewW, viewH)
                }
            }
        }
        // The stage is what the local zoom scales: video surface and cursor. Overlays stay in root.
        stage = FrameLayout(this).apply {
            pivotX = 0f
            pivotY = 0f
            addOnLayoutChangeListener { _, _, _, _, _, _, _, _, _ -> fitSurface() }
        }
        root.addView(stage, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
        surfaceView = SurfaceView(this).also { it.holder.addCallback(this) }
        stage.addView(surfaceView, ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)

        cursorView = ImageView(this).apply {
            scaleType = ImageView.ScaleType.FIT_XY
            visibility = View.GONE
            isClickable = false
        }
        stage.addView(cursorView, FrameLayout.LayoutParams(1, 1))

        statusView = overlayText(Gravity.CENTER, 20f).apply { setPadding(32, 16, 32, 16) }
        root.addView(statusView, FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.CENTER,
        ))

        telemetryView = overlayText(Gravity.START, 12f).apply {
            typeface = Typeface.MONOSPACE
            visibility = View.GONE
            setPadding(16, 8, 16, 8)
        }
        root.addView(telemetryView, FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.TOP or Gravity.START,
        ))
        settingsButton = ImageButton(this).apply {
            setImageResource(android.R.drawable.ic_menu_preferences)
            setBackgroundColor(0x99000000.toInt())
            contentDescription = getString(R.string.settings_button)
            setOnClickListener { startActivity(Intent(this@MainActivity, SettingsActivity::class.java)) }
        }
        root.addView(settingsButton, FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.TOP or Gravity.END,
        ).apply { setMargins(16, 16, 16, 16) })
        setContentView(root)
    }

    private fun overlayText(gravity: Int, sp: Float) = TextView(this).apply {
        setTextColor(Color.WHITE)
        setBackgroundColor(0x99000000.toInt())
        setTextSize(TypedValue.COMPLEX_UNIT_SP, sp)
        this.gravity = gravity
        isClickable = false
    }

    /** Highest refresh rate at native resolution. */
    private fun pickDisplayMode() {
        val native = Capabilities.nativeModes(Capabilities.display(this))
        val best = native.best
        chosenMode = best
        window.attributes = window.attributes.also { it.preferredDisplayModeId = best.modeId }
        Log.i(TAG, "display mode ${best.physicalWidth}x${best.physicalHeight}@${best.refreshRate} (id ${best.modeId})")
    }

    @Suppress("DEPRECATION")
    private fun hideSystemBars() {
        if (Build.VERSION.SDK_INT >= 30) {
            window.setDecorFitsSystemWindows(false)
            window.insetsController?.let {
                it.hide(WindowInsets.Type.systemBars())
                it.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            }
        } else {
            window.decorView.systemUiVisibility = View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY or
                View.SYSTEM_UI_FLAG_FULLSCREEN or View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or
                View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) {
            hideSystemBars()
            if (Build.VERSION.SDK_INT >= 31) root.requestUnbufferedDispatch(InputDevice.SOURCE_CLASS_POINTER)
        } else if (barrelPressAt != 0L && SystemClock.uptimeMillis() - barrelPressAt <= AIR_FOCUS_LOSS_MS) {
            // A barrel press just stole the focus: probably Samsung Air actions.
            barrelPressAt = 0
            if (!airHintShown && inputActive()) {
                airHintShown = true
                showTransient(getString(R.string.status_air_actions_hint), AIR_HINT_SHOW_MS)
            }
        }
    }

    override fun onResume() {
        super.onResume()
        applyInputConfig()
    }

    private fun applyInputConfig() {
        NativeBridge.setInputConfig(InputPrefs.toJson(this))?.let { Log.e(TAG, "bad input config: $it") }
    }

    override fun onStart() {
        super.onStart()
        started = true
        backoffMs = BACKOFF_MIN_MS
        setStatus(getString(R.string.status_waiting, "$HOST:$PORT"))
        Choreographer.getInstance().postFrameCallback(this)
        scheduleConnect(0)
    }

    override fun onStop() {
        started = false
        handler.removeCallbacks(connectRunnable)
        Choreographer.getInstance().removeFrameCallback(this)
        stopSession()
        super.onStop()
    }

    // --- Surface lifecycle ---

    override fun surfaceCreated(holder: SurfaceHolder) {
        surfaceReady = true
        if (sessionActive) NativeBridge.surfaceChanged(holder.surface) else scheduleConnect(0)
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        if (sessionActive) NativeBridge.surfaceChanged(holder.surface)
        frameRateSet = 0
        applyFrameRate()
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        surfaceReady = false
        if (sessionActive) NativeBridge.surfaceDestroyed()
    }

    // --- Connection loop ---

    private fun scheduleConnect(delayMs: Long) {
        handler.removeCallbacks(connectRunnable)
        if (started) handler.postDelayed(connectRunnable, delayMs)
    }

    private fun connect() {
        if (!started || sessionActive) return
        if (!surfaceReady) return // surfaceCreated reschedules
        val caps = capsJson ?: Capabilities.toJson(this, chosenMode).also { capsJson = it }
        val err = NativeBridge.start(surfaceView.holder.surface, HOST, PORT, caps)
        if (err != null) {
            Log.w(TAG, "start failed: $err")
            setStatus(getString(R.string.status_bad_caps, err))
            scheduleConnect(backoffMs)
            backoffMs = (backoffMs * 2).coerceAtMost(BACKOFF_MAX_MS)
            return
        }
        sessionActive = true
        applyInputConfig()
        wasStreaming = false
        lastInputNote = ""
        keysHintShown = false
        airHintShown = false
        streamW = 0
        streamH = 0
        bufW = 0
        bufH = 0
        fitSurface()
        frameRateSet = 0
        setStatus(getString(R.string.status_waiting, "$HOST:$PORT"))
    }

    private fun stopSession() {
        if (sessionActive) {
            NativeBridge.stop()
            sessionActive = false
        }
        cursorVisible = false
        cursorView.visibility = View.GONE
        cursorSeq = 0
        cursorShapeId = -1
        applyStage(floatArrayOf(1f, 0f, 0f, -1f, -1f))
        wasStreaming = false
        telemetryOn = false
        telemetryView.visibility = View.GONE
        updateChrome()
    }

    private fun inputActive() = sessionActive && wasStreaming

    /** The settings button is only available while input is not being forwarded. */
    private fun updateChrome() {
        settingsButton.visibility = if (inputActive()) View.GONE else View.VISIBLE
    }

    // --- Per-frame polling ---

    override fun doFrame(frameTimeNanos: Long) {
        if (!started) return
        Choreographer.getInstance().postFrameCallback(this)
        if (!sessionActive) return
        val view = NativeBridge.pollView()
        applyStage(view)
        NativeBridge.inputTick()
        updateCursor(view)
        val now = SystemClock.uptimeMillis()
        if (now - lastStatsPoll >= STATS_INTERVAL_MS) {
            lastStatsPoll = now
            updateStats(now)
        }
    }

    private fun applyStage(view: FloatArray) {
        if (view[0] == stageScale && view[1] == stageTx && view[2] == stageTy) return
        stageScale = view[0]
        stageTx = view[1]
        stageTy = view[2]
        stage.scaleX = stageScale
        stage.scaleY = stageScale
        stage.translationX = stageTx
        stage.translationY = stageTy
    }

    private fun updateCursor(view: FloatArray) {
        val c = NativeBridge.pollCursor()
        val seq = c[0]
        val visible = c[3] != 0L && streamW > 0
        if (seq != 0L && visible) {
            val shapeId = c[4].toInt()
            if (shapeId != cursorShapeId) loadCursorShape(shapeId)
            // The stage shows the stream's visible area; the surface may extend past it (fitSurface).
            val sx = stage.width.toFloat() / streamW
            val sy = stage.height.toFloat() / streamH
            if (sx != cursorScaleX || sy != cursorScaleY) {
                cursorScaleX = sx
                cursorScaleY = sy
                applyCursorSize()
            }
            // While the pen is in range the cursor follows it locally, otherwise the host position.
            val penX = view[3]
            val cx = if (penX >= 0f) penX * streamW else c[1].toFloat()
            val cy = if (penX >= 0f) view[4] * streamH else c[2].toFloat()
            cursorView.translationX = (cx - cursorHotX) * sx
            cursorView.translationY = (cy - cursorHotY) * sy
        }
        if (visible != cursorVisible && (!visible || cursorShapeId >= 0)) {
            cursorVisible = visible
            cursorView.visibility = if (visible) View.VISIBLE else View.GONE
        }
        cursorSeq = seq
    }

    private fun applyCursorSize() {
        if (cursorBmpW == 0) return
        val lp = cursorView.layoutParams
        lp.width = (cursorBmpW * cursorScaleX).toInt().coerceAtLeast(1)
        lp.height = (cursorBmpH * cursorScaleY).toInt().coerceAtLeast(1)
        cursorView.layoutParams = lp
    }

    private fun loadCursorShape(shapeId: Int) {
        val raw = NativeBridge.takeCursorShape(shapeId) ?: return
        val bb = ByteBuffer.wrap(raw).order(ByteOrder.LITTLE_ENDIAN)
        val w = bb.short.toInt() and 0xFFFF
        val h = bb.short.toInt() and 0xFFFF
        val hx = bb.short.toInt() and 0xFFFF
        val hy = bb.short.toInt() and 0xFFFF
        if (w == 0 || h == 0 || raw.size < 8 + w * h * 4) return
        val px = IntArray(w * h)
        var o = 8
        for (i in px.indices) {
            val r = raw[o].toInt() and 0xFF
            val g = raw[o + 1].toInt() and 0xFF
            val b = raw[o + 2].toInt() and 0xFF
            val a = raw[o + 3].toInt() and 0xFF
            px[i] = (a shl 24) or (r shl 16) or (g shl 8) or b
            o += 4
        }
        cursorView.setImageBitmap(Bitmap.createBitmap(px, w, h, Bitmap.Config.ARGB_8888))
        cursorShapeId = shapeId
        cursorHotX = hx
        cursorHotY = hy
        cursorBmpW = w
        cursorBmpH = h
        // Hotspot offset is applied in stream pixels (see updateCursor), so size follows the view scale.
        cursorScaleX = 0f
        cursorScaleY = 0f
    }

    /**
     * Sizes the surface so one decoded pixel maps to one view pixel along each axis. Decoders
     * output buffers padded to the codec's block size (2960x1856 for a 2960x1848 stream) and the
     * compositor scales the whole buffer to the view, which resamples every row. Growing the view
     * by the same ratio keeps the visible area 1:1 and pushes the padding past the stage edge.
     */
    private fun fitSurface() {
        val (vw, vh) = stage.width to stage.height
        val lp = surfaceView.layoutParams as FrameLayout.LayoutParams
        val (w, h) = if (streamW > 0 && streamH > 0 && bufW >= streamW && bufH >= streamH && vw > 0 && vh > 0) {
            (vw.toLong() * bufW / streamW).toInt() to (vh.toLong() * bufH / streamH).toInt()
        } else {
            ViewGroup.LayoutParams.MATCH_PARENT to ViewGroup.LayoutParams.MATCH_PARENT
        }
        if (lp.width == w && lp.height == h) return
        lp.width = w
        lp.height = h
        surfaceView.layoutParams = lp
        surfaceView.clipBounds = if (w > 0) Rect(0, 0, vw, vh) else null
    }

    private fun updateStats(now: Long) {
        val j = try {
            JSONObject(NativeBridge.pollStats())
        } catch (e: Exception) {
            Log.w(TAG, "bad stats", e)
            return
        }
        val bw = j.optInt("buffer_width")
        val bh = j.optInt("buffer_height")
        if (bw > 0 && bh > 0 && (bw != bufW || bh != bufH)) {
            bufW = bw
            bufH = bh
            fitSurface()
        }
        val addr = "$HOST:$PORT"
        when (j.optString("phase")) {
            "connecting" -> setStatus(getString(R.string.status_waiting, addr))
            "handshaking" -> setStatus(getString(R.string.status_handshake, addr))
            "streaming" -> {
                if (!wasStreaming) {
                    wasStreaming = true
                    updateChrome()
                    backoffMs = BACKOFF_MIN_MS
                    streamW = j.optInt("width")
                    streamH = j.optInt("height")
                    val hz = formatHz(j.optInt("refresh_mhz"))
                    setStatus(
                        getString(
                            R.string.status_connected, streamW, streamH, hz,
                            j.optString("codec"), j.optString("decoder").takeIf { it != "null" }.orEmpty(),
                        ).trim(),
                    )
                    statusHideAt = now + STATUS_HIDE_MS
                    applyFrameRate(j.optInt("refresh_mhz"))
                } else if (statusHideAt != 0L && now >= statusHideAt) {
                    statusHideAt = 0
                    statusView.visibility = View.GONE
                }
            }
            "ended" -> disconnected(getString(R.string.status_ended, j.optString("detail"), addr))
            "failed" -> disconnected(getString(R.string.status_error, j.optString("detail")))
        }
        val flags = if (j.isNull("client_flags")) 0 else j.optInt("client_flags")
        telemetryOn = (flags and 1) != 0
        telemetryView.visibility = if (telemetryOn) View.VISIBLE else View.GONE
        if (telemetryOn) showTelemetry(j)
        val note = if (j.isNull("input_note")) "" else j.optString("input_note")
        if (note != lastInputNote) {
            lastInputNote = note
            if (note.isNotEmpty()) {
                showTransient(getString(R.string.status_input_note, note), NOTE_SHOW_MS)
                return
            }
        }
        if (!keysHintShown && !j.isNull("input_caps") && (j.optInt("input_caps") and 8) == 0 &&
            InputPrefs.usesKeys(this)
        ) {
            keysHintShown = true
            showTransient(getString(R.string.status_keys_unsupported), NOTE_SHOW_MS)
        }
    }

    private fun showTransient(text: String, ms: Long) {
        setStatus(text)
        statusHideAt = SystemClock.uptimeMillis() + ms
    }

    private fun disconnected(message: String) {
        Log.i(TAG, "disconnected: $message")
        stopSession()
        setStatus(message)
        scheduleConnect(backoffMs)
        backoffMs = (backoffMs * 2).coerceAtMost(BACKOFF_MAX_MS)
    }

    private fun setStatus(text: String) {
        statusView.text = text
        statusView.visibility = View.VISIBLE
        statusHideAt = 0
    }

    private fun formatHz(mhz: Int): String =
        if (mhz % 1000 == 0) (mhz / 1000).toString() else String.format(Locale.US, "%.2f", mhz / 1000.0)

    private fun applyFrameRate(mhz: Int = frameRateSet) {
        if (mhz <= 0 || Build.VERSION.SDK_INT < 30) return
        val surface = surfaceView.holder.surface
        if (!surface.isValid) return
        surface.setFrameRate(
            mhz / 1000f, android.view.Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE,
        )
        frameRateSet = mhz
    }

    private fun showTelemetry(j: JSONObject) {
        fun ms(us: Int) = String.format(Locale.US, "%.1f", us / 1000.0)
        val rtt = if (j.isNull("rtt_us")) "-" else ms(j.optInt("rtt_us"))
        telemetryView.text = getString(
            R.string.telemetry,
            String.format(Locale.US, "%.0f", j.optDouble("fps")),
            String.format(Locale.US, "%.1f", j.optDouble("mbps")),
            ms(j.optInt("decode_us_p50")), ms(j.optInt("decode_us_p95")),
            ms(j.optInt("e2e_us_p50")), ms(j.optInt("e2e_us_p95")),
            rtt, j.optInt("frames_dropped"),
            "${j.optString("codec").uppercase(Locale.ROOT)} · " +
                (if (j.optString("dynamic_range") == "hdr10") "HDR10" else "SDR") +
                " · ${j.optString("encoder")} → ${j.optString("decoder")}",
        )
    }

    // --- Input forwarding ---

    override fun dispatchTouchEvent(ev: MotionEvent): Boolean {
        if (inputActive()) {
            if (Build.VERSION.SDK_INT < 31 && ev.actionMasked == MotionEvent.ACTION_DOWN) {
                root.requestUnbufferedDispatch(ev)
            }
            forward(ev)
            return true
        }
        return super.dispatchTouchEvent(ev)
    }

    override fun dispatchGenericMotionEvent(ev: MotionEvent): Boolean {
        if (inputActive() && ev.isFromSource(InputDevice.SOURCE_CLASS_POINTER)) {
            forward(ev)
            return true
        }
        return super.dispatchGenericMotionEvent(ev)
    }

    /** Packs [ev] (with its history) for [NativeBridge.inputEvent]; buffers are reused. */
    private fun forward(ev: MotionEvent) {
        val n = ev.pointerCount
        val history = ev.historySize
        val steps = history + 1
        val intNeed = INT_HEADER + 2 * n
        val floatNeed = FLOAT_HEADER + steps * n * 5
        if (intBuf.size < intNeed) intBuf = IntArray(intNeed * 2)
        if (floatBuf.size < floatNeed) floatBuf = FloatArray(floatNeed * 2)
        if (timeBuf.size < steps) timeBuf = LongArray(steps * 2)
        val ints = intBuf
        val floats = floatBuf
        val times = timeBuf
        ints[0] = ev.actionMasked
        ints[1] = ev.actionIndex
        ints[2] = ev.buttonState
        ints[3] = ev.flags
        ints[4] = n
        ints[5] = history
        for (p in 0 until n) {
            ints[INT_HEADER + 2 * p] = ev.getPointerId(p)
            ints[INT_HEADER + 2 * p + 1] = ev.getToolType(p)
        }
        floats[0] = ev.getAxisValue(MotionEvent.AXIS_VSCROLL)
        floats[1] = ev.getAxisValue(MotionEvent.AXIS_HSCROLL)
        var o = FLOAT_HEADER
        for (h in 0 until history) {
            times[h] = historicalTimeNanos(ev, h)
            for (p in 0 until n) {
                floats[o] = ev.getHistoricalX(p, h)
                floats[o + 1] = ev.getHistoricalY(p, h)
                floats[o + 2] = ev.getHistoricalPressure(p, h)
                floats[o + 3] = ev.getHistoricalAxisValue(MotionEvent.AXIS_TILT, p, h)
                floats[o + 4] = ev.getHistoricalAxisValue(MotionEvent.AXIS_ORIENTATION, p, h)
                o += 5
            }
        }
        times[history] = timeNanos(ev)
        for (p in 0 until n) {
            floats[o] = ev.getX(p)
            floats[o + 1] = ev.getY(p)
            floats[o + 2] = ev.getPressure(p)
            floats[o + 3] = ev.getAxisValue(MotionEvent.AXIS_TILT, p)
            floats[o + 4] = ev.getAxisValue(MotionEvent.AXIS_ORIENTATION, p)
            o += 5
        }
        trackBarrel(ev.buttonState)
        NativeBridge.inputEvent(ints, floats, times)
    }

    private fun timeNanos(ev: MotionEvent): Long =
        if (Build.VERSION.SDK_INT >= 34) ev.eventTimeNanos else ev.eventTime * 1_000_000L

    private fun historicalTimeNanos(ev: MotionEvent, h: Int): Long =
        if (Build.VERSION.SDK_INT >= 34) ev.getHistoricalEventTimeNanos(h)
        else ev.getHistoricalEventTime(h) * 1_000_000L

    /** Remembers when the barrel button went down, for the Air actions heuristic. */
    private fun trackBarrel(buttonState: Int) {
        val down = (buttonState and BARREL_MASK) != 0
        if (down && !barrelDown) barrelPressAt = SystemClock.uptimeMillis()
        barrelDown = down
    }
}

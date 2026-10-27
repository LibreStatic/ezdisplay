package com.librestatic.ezdisplay

import android.annotation.SuppressLint
import android.app.Activity
import android.graphics.Bitmap
import android.graphics.Color
import android.graphics.Rect
import android.graphics.Typeface
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.res.Configuration
import android.hardware.usb.UsbAccessory
import android.hardware.usb.UsbManager
import android.media.AudioManager
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
import android.view.KeyEvent
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
import com.librestatic.ezdisplay.settings.SettingsStore
import com.librestatic.ezdisplay.settings.wifiErrorText
import com.librestatic.ezdisplay.usb.Route
import com.librestatic.ezdisplay.usb.chooseRoute
import com.librestatic.ezdisplay.usb.isOurAccessory
import com.librestatic.ezdisplay.usb.shouldSwitchToAccessory
import com.librestatic.ezdisplay.wifi.HostDiscovery
import com.librestatic.ezdisplay.wifi.NativeWifiBackend
import com.librestatic.ezdisplay.wifi.WifiChoice
import com.librestatic.ezdisplay.wifi.WifiFailures
import com.librestatic.ezdisplay.wifi.chooseWifiTarget
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
 *
 * Connection order: a USB accessory (AOA, no developer mode needed) first, then the adb-reversed loopback port.
 * An attached accessory always wins and is retried with the normal backoff while it stays attached; the loopback
 * port and Wi-Fi are only used when none is attached. When the loopback attempt fails and Wi-Fi is enabled in the settings
 * with a paired host known, the next attempt goes to that host over Wi-Fi; if the Wi-Fi attempt fails the next one is
 * USB again, so plugging the cable in always wins.
 */
class MainActivity : Activity(), SurfaceHolder.Callback, Choreographer.FrameCallback {
    private companion object {
        const val TAG = "ezdisplay"
        const val HOST = "127.0.0.1"
        const val PORT = 27183
        const val ACTION_USB_PERMISSION = "com.librestatic.ezdisplay.USB_PERMISSION"
        const val LAST_WIFI_HOST = "wifi_last_host"
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
    private val volumeWatcher by lazy {
        VolumeWatcher(
            this, handler,
            VolumeSync(
                AudioManagerVolume(getSystemService(AudioManager::class.java)),
                NativeBridge::setLocalVolume,
                NativeBridge::pollHostVolume,
            ),
        )
    }
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
    private var hostLacksKeys = false // the host reported input_caps without the KEYS bit
    private val keysDown = LinkedHashSet<Int>() // hardware key codes currently down on the host
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

    private lateinit var discovery: HostDiscovery
    private val wifi by lazy { NativeWifiBackend(this) }
    private var activeWifi: WifiChoice? = null // set while the current session is over Wi-Fi
    private val wifiFailures = WifiFailures() // discovered or stored addresses that failed lately: tried last
    private var tryWifiNext = false // the loopback connection just failed: try the paired host
    private var pairingLost: String? = null // the host forgot this device: kept above later retry messages

    private val usbManager by lazy { getSystemService(Context.USB_SERVICE) as UsbManager }
    private var accessory: UsbAccessory? = null // our accessory while it is attached
    private var activeAccessory = false // the current session runs over the accessory
    private var accessoryPermissionAsked = false // a permission dialog was shown for the attached accessory
    private var launchIntent: Intent? = null // the intent that started or re-delivered this activity, read once
    private var usbReceiverRegistered = false

    private val usbReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            val acc = usbAccessoryExtra(intent)
            when (intent.action) {
                UsbManager.ACTION_USB_ACCESSORY_DETACHED -> {
                    if (acc == null || isOurAccessory(acc.manufacturer, acc.model)) accessoryGone()
                }
                ACTION_USB_PERMISSION -> {
                    if (intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)) {
                        checkAccessory()
                    } else {
                        Log.w(TAG, "USB accessory permission denied")
                    }
                }
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        launchIntent = intent
        volumeControlStream = AudioManager.STREAM_MUSIC // the volume keys always move the PC's audio
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        if (Build.VERSION.SDK_INT >= 28) {
            window.attributes = window.attributes.also {
                it.layoutInDisplayCutoutMode =
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
            }
        }
        pickDisplayMode()
        discovery = HostDiscovery(this) { }
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
        if (!hasFocus) releaseAllKeys()
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

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        pushKeyboardState(newConfig)
    }

    /** Tells the native side (and through it the host) whether a hardware keyboard is attached. */
    private fun pushKeyboardState(config: Configuration = resources.configuration) {
        NativeBridge.setKeyboardAttached(
            config.keyboard == Configuration.KEYBOARD_QWERTY &&
                config.hardKeyboardHidden == Configuration.HARDKEYBOARDHIDDEN_NO,
        )
    }

    override fun onResume() {
        super.onResume()
        applyInputConfig()
        updateDiscovery()
        checkAccessory()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        launchIntent = intent
        accessoryPermissionAsked = false
        checkAccessory()
    }

    @Suppress("DEPRECATION")
    private fun usbAccessoryExtra(intent: Intent?): UsbAccessory? =
        if (Build.VERSION.SDK_INT >= 33) intent?.getParcelableExtra(UsbManager.EXTRA_ACCESSORY, UsbAccessory::class.java)
        else intent?.getParcelableExtra(UsbManager.EXTRA_ACCESSORY)

    @SuppressLint("InlinedApi")
    private fun registerUsbReceiver() {
        if (usbReceiverRegistered) return
        val filter = IntentFilter().apply {
            addAction(UsbManager.ACTION_USB_ACCESSORY_DETACHED)
            addAction(ACTION_USB_PERMISSION)
        }
        // The flag is a constant, ignored before Android 13; the permission result is sent as this app.
        registerReceiver(usbReceiver, filter, Context.RECEIVER_NOT_EXPORTED)
        usbReceiverRegistered = true
    }

    private fun unregisterUsbReceiver() {
        if (!usbReceiverRegistered) return
        usbReceiverRegistered = false
        unregisterReceiver(usbReceiver)
    }

    /**
     * Finds our accessory (from the attach intent, else the system's list), asks for permission when the app
     * was not launched by the attach, and switches to it. Cheap and idempotent: called on start, resume,
     * new intents and permission results.
     */
    private fun checkAccessory() {
        val fromIntent = launchIntent?.takeIf { it.action == UsbManager.ACTION_USB_ACCESSORY_ATTACHED }
            ?.let { usbAccessoryExtra(it) }
        launchIntent = null
        if (!started) {
            // Keep the intent's accessory for onStart (onNewIntent arrives before onStart when stopped).
            if (fromIntent != null) launchIntent = intentFor(fromIntent)
            return
        }
        val found = fromIntent?.takeIf { isOurAccessory(it.manufacturer, it.model) }
            ?: usbManager.accessoryList?.firstOrNull { isOurAccessory(it.manufacturer, it.model) }
        if (found == null) {
            if (accessory != null) accessoryGone()
            return
        }
        accessory = found
        if (!usbManager.hasPermission(found)) {
            if (!accessoryPermissionAsked) {
                accessoryPermissionAsked = true
                val flags = PendingIntent.FLAG_UPDATE_CURRENT or
                    (if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0)
                val pi = PendingIntent.getBroadcast(
                    this, 0, Intent(ACTION_USB_PERMISSION).setPackage(packageName), flags,
                )
                setStatus(getString(R.string.status_usb_permission))
                usbManager.requestPermission(found, pi)
            }
            return
        }
        if (shouldSwitchToAccessory(true, sessionActive, activeAccessory)) {
            if (sessionActive) stopSession()
            backoffMs = BACKOFF_MIN_MS
            scheduleConnect(0)
        }
    }

    private fun intentFor(acc: UsbAccessory) =
        Intent(UsbManager.ACTION_USB_ACCESSORY_ATTACHED).putExtra(UsbManager.EXTRA_ACCESSORY, acc)

    /** The accessory is gone: end its session and go back to the loopback and Wi-Fi loop. */
    private fun accessoryGone() {
        accessory = null
        accessoryPermissionAsked = false
        if (!activeAccessory) return
        stopSession()
        backoffMs = BACKOFF_MIN_MS
        tryWifiNext = false
        setStatus(getString(R.string.status_waiting, "$HOST:$PORT"))
        scheduleConnect(0)
    }

    private fun wifiEnabled() = InputPrefs.prefs(this).getBoolean(SettingsStore.WIFI_ENABLED, false)

    /** Looks for hosts on the network while the waiting screen is up and Wi-Fi is enabled. */
    private fun updateDiscovery() {
        if (started && !inputActive() && wifiEnabled()) discovery.start() else discovery.stop()
    }

    /** The paired host to try over Wi-Fi, or null when Wi-Fi is off or no paired host has an address. */
    private fun wifiTarget(): WifiChoice? {
        if (!wifiEnabled()) return null
        val last = InputPrefs.prefs(this).getString(LAST_WIFI_HOST, null)
        return chooseWifiTarget(wifi.hosts(), discovery.hosts, last, wifiFailures.active(SystemClock.elapsedRealtime()))
    }

    private fun applyInputConfig() {
        NativeBridge.setInputConfig(InputPrefs.toJson(this))?.let { Log.e(TAG, "bad input config: $it") }
    }

    override fun onStart() {
        super.onStart()
        started = true
        backoffMs = BACKOFF_MIN_MS
        tryWifiNext = false
        setStatus(getString(R.string.status_waiting, "$HOST:$PORT"))
        Choreographer.getInstance().postFrameCallback(this)
        updateDiscovery()
        registerUsbReceiver()
        checkAccessory()
        scheduleConnect(0)
    }

    override fun onPause() {
        releaseAllKeys()
        super.onPause()
    }

    override fun onStop() {
        started = false
        handler.removeCallbacks(connectRunnable)
        Choreographer.getInstance().removeFrameCallback(this)
        stopSession()
        unregisterUsbReceiver()
        discovery.stop()
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
        val caps = Capabilities.withAudio(
            capsJson ?: Capabilities.toJson(this, chosenMode).also { capsJson = it },
            InputPrefs.prefs(this).getBoolean(SettingsStore.AUDIO_ENABLED, true),
        )
        pushKeyboardState()
        volumeWatcher.start()
        val acc = accessory?.takeIf { usbManager.hasPermission(it) }
        val wifiChoice = if (tryWifiNext) wifiTarget() else null
        val route = chooseRoute(acc != null, tryWifiNext, wifiChoice != null)
        tryWifiNext = false
        val choice = if (route == Route.WIFI) wifiChoice else null
        val err = when (route) {
            Route.ACCESSORY -> startAccessory(acc!!, caps)
            Route.WIFI -> NativeBridge.startWifi(
                surfaceView.holder.surface, choice!!.addr, choice.host.fingerprint,
                NativeWifiBackend.stateDir(this).absolutePath, caps,
            )
            Route.TCP -> NativeBridge.start(surfaceView.holder.surface, HOST, PORT, caps)
        }
        if (err != null) {
            Log.w(TAG, "start failed: $err")
            volumeWatcher.stop()
            setStatus(
                if (route == Route.ACCESSORY) getString(R.string.status_usb_open_failed)
                else getString(R.string.status_bad_caps, err),
            )
            scheduleConnect(backoffMs)
            backoffMs = (backoffMs * 2).coerceAtMost(BACKOFF_MAX_MS)
            return
        }
        sessionActive = true
        activeWifi = choice
        activeAccessory = route == Route.ACCESSORY
        applyInputConfig()
        wasStreaming = false
        lastInputNote = ""
        keysHintShown = false
        hostLacksKeys = false
        airHintShown = false
        streamW = 0
        streamH = 0
        bufW = 0
        bufH = 0
        fitSurface()
        frameRateSet = 0
        setStatus(
            when (route) {
                Route.ACCESSORY -> getString(R.string.status_usb_connecting)
                Route.WIFI -> getString(R.string.status_wifi_connecting, hostLabel(choice!!))
                Route.TCP -> getString(R.string.status_waiting, "$HOST:$PORT")
            },
        )
    }

    /** Opens [acc] and hands the descriptor to the native layer, which closes it. Null on success, else an error. */
    private fun startAccessory(acc: UsbAccessory, caps: String): String? {
        val pfd = try {
            usbManager.openAccessory(acc)
        } catch (e: Exception) {
            return "open accessory: ${e.message}"
        } ?: return "open accessory failed"
        // detachFd leaves the descriptor open and ours to close: the native session owns it from here on.
        val fd = pfd.detachFd()
        return NativeBridge.startFd(surfaceView.holder.surface, fd, caps)
    }

    private fun hostLabel(choice: WifiChoice) = choice.host.name.ifEmpty { choice.host.shortFp }

    private fun stopSession() {
        releaseAllKeys()
        volumeWatcher.stop()
        if (sessionActive) {
            NativeBridge.stop()
            sessionActive = false
        }
        activeWifi = null
        activeAccessory = false
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
        updateDiscovery()
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
            volumeWatcher.pollHost()
            volumeWatcher.checkLocal()
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
        val wifiChoice = activeWifi
        val usb = activeAccessory
        val addr = if (wifiChoice != null) hostLabel(wifiChoice) else "$HOST:$PORT"
        when (j.optString("phase")) {
            "connecting" -> setStatus(
                when {
                    usb -> getString(R.string.status_usb_connecting)
                    wifiChoice != null -> getString(R.string.status_wifi_connecting, addr)
                    else -> getString(R.string.status_waiting, addr)
                },
            )
            "handshaking" -> setStatus(
                when {
                    usb -> getString(R.string.status_usb_handshake)
                    wifiChoice != null -> getString(R.string.status_wifi_handshake, addr)
                    else -> getString(R.string.status_handshake, addr)
                },
            )
            "streaming" -> {
                if (!wasStreaming) {
                    wasStreaming = true
                    pairingLost = null
                    updateChrome()
                    backoffMs = BACKOFF_MIN_MS
                    streamW = j.optInt("width")
                    streamH = j.optInt("height")
                    val hz = formatHz(j.optInt("refresh_mhz"))
                    val connected = getString(
                        R.string.status_connected, streamW, streamH, hz,
                        j.optString("codec"), j.optString("decoder").takeIf { it != "null" }.orEmpty(),
                    ).trim()
                    if (wifiChoice != null) {
                        InputPrefs.prefs(this).edit().putString(LAST_WIFI_HOST, wifiChoice.host.fingerprint).apply()
                        wifiFailures.clear(wifiChoice.host.fingerprint, wifiChoice.addr)
                        // Drawing over Wi-Fi works but feels different: say so, and keep it up a little longer.
                        setStatus(connected + "\n" + getString(R.string.status_wifi_latency))
                    } else {
                        setStatus(connected)
                    }
                    statusHideAt = now + if (wifiChoice != null) NOTE_SHOW_MS else STATUS_HIDE_MS
                    applyFrameRate(j.optInt("refresh_mhz"))
                } else if (statusHideAt != 0L && now >= statusHideAt) {
                    statusHideAt = 0
                    statusView.visibility = View.GONE
                }
            }
            "ended" -> disconnected(
                if (usb) getString(R.string.status_ended_usb, j.optString("detail"))
                else getString(R.string.status_ended, j.optString("detail"), addr),
                false,
            )
            "failed" -> {
                // A Wi-Fi failure carries a stable code; the detail is English text for the log.
                val code = if (j.isNull("code")) null else j.optString("code")
                val message = if (code != null && activeWifi != null) {
                    getString(wifiErrorText(code))
                } else {
                    getString(R.string.status_error, j.optString("detail"))
                }
                // On not_paired the native layer already forgot the host, so wifiTarget() finds no
                // address for it and the retries go back to USB only.
                if (code == "not_paired" && activeWifi != null) pairingLost = message
                disconnected(message, true)
            }
        }
        val flags = if (j.isNull("client_flags")) 0 else j.optInt("client_flags")
        telemetryOn = (flags and 1) != 0
        telemetryView.visibility = if (telemetryOn) View.VISIBLE else View.GONE
        if (telemetryOn) showTelemetry(j)
        hostLacksKeys = !j.isNull("input_caps") && (j.optInt("input_caps") and 8) == 0
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

    private fun disconnected(message: String, failed: Boolean) {
        Log.i(TAG, "disconnected: $message")
        val wasWifi = activeWifi != null
        // Any failure over Wi-Fi demotes that address, so the next attempt tries another candidate.
        if (failed) activeWifi?.let { wifiFailures.record(it.host.fingerprint, it.addr, SystemClock.elapsedRealtime()) }
        stopSession()
        // Keep telling the user why Wi-Fi stopped until a host is paired again.
        if (wifiTarget() != null) pairingLost = null
        setStatus(message)
        // The loopback connection failed (no cable, no adb reverse): try the paired host over Wi-Fi right away.
        // While an accessory is attached it is retried with the normal backoff; no fallback.
        if (failed && !wasWifi && accessory == null && wifiTarget() != null) {
            tryWifiNext = true
            scheduleConnect(0)
            return
        }
        scheduleConnect(backoffMs)
        backoffMs = (backoffMs * 2).coerceAtMost(BACKOFF_MAX_MS)
    }

    private fun setStatus(text: String) {
        val notice = pairingLost?.takeIf { it != text }
        statusView.text = if (notice != null) "$notice\n$text" else text
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

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (inputActive()) {
            volumeKeyDirection(event.keyCode)?.let { direction ->
                // Handled here so the system volume panel stays hidden: its blur stalls the video.
                if (event.action == KeyEvent.ACTION_DOWN) volumeWatcher.onKey(direction)
                return true
            }
        }
        if (inputActive() && isHardwareKey(event)) {
            val code = event.keyCode
            val nanos = event.eventTime * 1_000_000L
            when (event.action) {
                // repeatCount > 0 is an autorepeat: the host treats a press of a key already down as one.
                KeyEvent.ACTION_DOWN -> if (NativeBridge.keyEvent(code, true, nanos)) {
                    keysDown.add(code)
                    if (hostLacksKeys && !keysHintShown) {
                        keysHintShown = true
                        showTransient(getString(R.string.status_keys_unsupported), NOTE_SHOW_MS)
                    }
                    return true
                }
                KeyEvent.ACTION_UP -> if (keysDown.remove(code)) {
                    NativeBridge.keyEvent(code, false, nanos)
                    return true
                }
            }
        }
        return super.dispatchKeyEvent(event)
    }

    /** The volume change a volume key asks for, or null for any other key. */
    private fun volumeKeyDirection(code: Int): Int? = when (code) {
        KeyEvent.KEYCODE_VOLUME_UP -> AudioManager.ADJUST_RAISE
        KeyEvent.KEYCODE_VOLUME_DOWN -> AudioManager.ADJUST_LOWER
        KeyEvent.KEYCODE_VOLUME_MUTE -> AudioManager.ADJUST_TOGGLE_MUTE
        else -> null
    }

    /** A key from a physical keyboard that Android does not need for itself. */
    private fun isHardwareKey(event: KeyEvent): Boolean {
        if (event.source and InputDevice.SOURCE_KEYBOARD != InputDevice.SOURCE_KEYBOARD) return false
        val device = event.device ?: return false
        if (device.isVirtual || device.keyboardType != InputDevice.KEYBOARD_TYPE_ALPHABETIC) return false
        return when (event.keyCode) {
            KeyEvent.KEYCODE_VOLUME_UP, KeyEvent.KEYCODE_VOLUME_DOWN, KeyEvent.KEYCODE_VOLUME_MUTE,
            KeyEvent.KEYCODE_POWER, KeyEvent.KEYCODE_HOME, KeyEvent.KEYCODE_APP_SWITCH,
            KeyEvent.KEYCODE_BRIGHTNESS_UP, KeyEvent.KEYCODE_BRIGHTNESS_DOWN -> false
            else -> true // media keys have no HID usage, so they fall through to Android
        }
    }

    /** Releases every forwarded key so none stays stuck on the PC. */
    private fun releaseAllKeys() {
        if (keysDown.isEmpty()) return
        val nanos = SystemClock.uptimeMillis() * 1_000_000L
        keysDown.toList().asReversed().forEach { NativeBridge.keyEvent(it, false, nanos) }
        keysDown.clear()
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

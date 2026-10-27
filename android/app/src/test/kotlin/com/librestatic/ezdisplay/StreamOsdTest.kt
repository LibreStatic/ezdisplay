package com.librestatic.ezdisplay

import androidx.activity.ComponentActivity
import android.graphics.Color
import android.view.Gravity
import android.view.View
import android.widget.FrameLayout
import androidx.test.core.app.ApplicationProvider
import com.github.takahirom.roborazzi.captureRoboImage
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class StreamOsdTest {
    private val context get() = ApplicationProvider.getApplicationContext<android.content.Context>()
    private val res get() = context.resources

    private fun status(report: String, extra: String = "") = JSONObject(
        """{"codec":"h264","bitrate_kbps":46000,"width":2960,"height":1848,"refresh_mhz":120000,
        "report_seq":2,"report":$report$extra}""",
    )

    private val autoReport = """{"wanted_kbps":46000,"user_bitrate":false,"ceiling_kbps":80000,
        "attempts":[{"encoder":"h264_vaapi","options":"rc_mode=VBR low_power=1","ok":false,"error":"Invalid argument"},
        {"encoder":"h264_nvenc","options":"rc=vbr cq=19","ok":true,"error":""}],
        "rate_mode":"vbr","quality":19,"target_kbps":46000,"max_kbps":92000}"""

    @Test fun reportWithAttemptsAndPeak() {
        val c = StreamOsd.render(res, StreamOsd.parse(status(autoReport)))
        assertEquals("Negotiated 46 Mbit/s · auto for 2960×1848 at 120 Hz · link limit 80 Mbit/s", c.lines[0])
        assertEquals("✗ h264_vaapi  rc_mode=VBR low_power=1 — Invalid argument", c.lines[1])
        assertEquals("✓ h264_nvenc  rc=vbr cq=19", c.lines[2])
        assertEquals("H.264 · variable bitrate, quality 19 · 46 Mbit/s (peak 92)", c.headline)
    }

    @Test fun clampedBitrateShowsWantedAndLimit() {
        val r = """{"wanted_kbps":95000,"user_bitrate":false,"ceiling_kbps":80000,"attempts":[],
            "rate_mode":"cbr","quality":0,"target_kbps":80000,"max_kbps":80000}"""
        val j = status(r).put("bitrate_kbps", 80000)
        val c = StreamOsd.render(res, StreamOsd.parse(j))
        assertTrue(c.lines[0], c.lines[0].contains("wanted 95, limited to 80"))
        assertEquals("H.264 · constant bitrate · 80 Mbit/s", c.headline)
    }

    @Test fun userBitrateSaysSetInSettings() {
        val r = autoReport.replace("\"user_bitrate\":false", "\"user_bitrate\":true")
        val c = StreamOsd.render(res, StreamOsd.parse(status(r)))
        assertTrue(c.lines[0], c.lines[0].contains("set in settings"))
        assertFalse(c.lines[0].contains("auto for"))
    }

    @Test fun smallBitrateHasOneDecimal() {
        assertEquals("4.5 Mbit/s", StreamOsd.mbit(res, 4500))
        assertEquals("46 Mbit/s", StreamOsd.mbit(res, 46000))
    }

    @Test fun longErrorIsEllipsizedOnOneLine() {
        val r = autoReport.replace("Invalid argument", "x".repeat(200) + "\\nsecond")
        val line = StreamOsd.render(res, StreamOsd.parse(status(r))).lines[1]
        assertTrue(line, line.endsWith("…"))
        assertFalse(line.contains("\n"))
        assertTrue(line.length < 120)
    }

    @Test fun attemptListIsCappedWithMoreLine() {
        val attempts = (1..9).joinToString(",") { """{"encoder":"e$it","options":"","ok":false,"error":""}""" }
        val r = """{"wanted_kbps":1,"user_bitrate":false,"ceiling_kbps":2,"attempts":[$attempts],
            "rate_mode":"unknown","quality":0,"target_kbps":1,"max_kbps":1}"""
        val c = StreamOsd.render(res, StreamOsd.parse(status(r)))
        assertEquals(1 + 1 + StreamOsd.MAX_ATTEMPTS, c.lines.size)
        assertEquals("+3 more", c.lines[1])
        assertEquals("✗ e4", c.lines[2])
        assertEquals("✗ e9", c.lines.last())
    }

    @Test fun oldHostShowsOnlyCodecAndBitrate() {
        val j = JSONObject("""{"codec":"hevc","bitrate_kbps":30000,"width":1,"height":1,"refresh_mhz":60000}""")
        val info = StreamOsd.parse(j)
        assertEquals(0, info.seq)
        assertNull(info.report)
        val c = StreamOsd.render(res, info)
        assertTrue(c.lines.isEmpty())
        assertEquals("HEVC · 30 Mbit/s", c.headline)
    }

    @Test fun unknownRateModeIsOmitted() {
        val r = autoReport.replace("\"vbr\"", "\"unknown\"").replace("92000", "46000")
        assertEquals("H.264 · 46 Mbit/s", StreamOsd.render(res, StreamOsd.parse(status(r))).headline)
    }

    @Test @Config(qualifiers = "es") fun spanishResolves() {
        val c = StreamOsd.render(res, StreamOsd.parse(status(autoReport)))
        assertTrue(c.lines[0], c.lines[0].startsWith("Negociado: 46 Mbit/s"))
        assertTrue(c.headline, c.headline.contains("tasa de bits variable, calidad 19"))
    }

    @Test fun viewIsNotInteractive() {
        val v = StreamOsdView(context)
        assertFalse(v.isClickable)
        assertFalse(v.isFocusable)
    }

    @Test @Config(qualifiers = "w1280dp-h800dp-notnight")
    fun captureOsd() {
        val activity = Robolectric.buildActivity(ComponentActivity::class.java).setup().get()
        val root = FrameLayout(activity).apply { setBackgroundColor(Color.rgb(40, 60, 90)) }
        val osd = StreamOsdView(activity)
        root.addView(osd, FrameLayout.LayoutParams(-2, -2, Gravity.BOTTOM or Gravity.START).apply { setMargins(32, 32, 32, 32) })
        activity.setContentView(root)
        osd.show(StreamOsd.render(activity.resources, StreamOsd.parse(status(autoReport))))
        assertEquals(View.VISIBLE, osd.visibility)
        root.captureRoboImage("build/outputs/roborazzi/stream_osd.png")
    }
}

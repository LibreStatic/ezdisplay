package com.librestatic.ezdisplay

import android.content.Context
import android.content.res.Resources
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.text.TextUtils
import android.util.TypedValue
import android.view.View
import android.widget.LinearLayout
import android.widget.TextView
import org.json.JSONObject
import java.util.Locale

/** One encoder the host tried to open. Encoder, options and error come from the host and are not translated. */
data class OsdAttempt(val encoder: String, val options: String, val ok: Boolean, val error: String)

/** The host's negotiation report (status JSON `report`). */
data class OsdReport(
    val wantedKbps: Int,
    val userBitrate: Boolean,
    val ceilingKbps: Int,
    val attempts: List<OsdAttempt>,
    val rateMode: String,
    val quality: Int,
    val targetKbps: Int,
    val maxKbps: Int,
    /** `normal` / `high` / `lossless`; `normal` and `auto` when an older host does not report them. */
    val wantedQuality: String = "normal",
    val appliedQuality: String = "normal",
    val wantedCodec: String = "auto",
)

/** What the on-screen display needs from one status poll. */
data class OsdInfo(
    val seq: Int,
    val codec: String,
    val bitrateKbps: Int,
    val width: Int,
    val height: Int,
    val refreshMhz: Int,
    val report: OsdReport?,
)

/** Text of the display: small monospace [lines] and the larger [headline]. */
data class OsdContent(val lines: List<String>, val headline: String)

object StreamOsd {
    const val MAX_ATTEMPTS = 6
    const val SHOW_MS = 6000L
    const val FADE_MS = 400L
    private const val MAX_ERROR_CHARS = 60

    /** Reads the fields of the status JSON. A missing `report_seq` or `report` (older native lib or host) is none. */
    fun parse(j: JSONObject): OsdInfo {
        val r = j.optJSONObject("report")
        val report = r?.let {
            val list = it.optJSONArray("attempts")
            OsdReport(
                wantedKbps = it.optInt("wanted_kbps"),
                userBitrate = it.optBoolean("user_bitrate"),
                ceilingKbps = it.optInt("ceiling_kbps"),
                attempts = (0 until (list?.length() ?: 0)).mapNotNull { i -> list?.optJSONObject(i) }.map { a ->
                    OsdAttempt(a.optString("encoder"), a.optString("options"), a.optBoolean("ok"), a.optString("error"))
                },
                rateMode = it.optString("rate_mode", "unknown"),
                quality = it.optInt("quality"),
                targetKbps = it.optInt("target_kbps"),
                maxKbps = it.optInt("max_kbps"),
                wantedQuality = it.optString("wanted_quality", "normal"),
                appliedQuality = it.optString("applied_quality", "normal"),
                wantedCodec = it.optString("wanted_codec", "auto"),
            )
        }
        return OsdInfo(
            seq = j.optInt("report_seq"),
            codec = j.optString("codec"),
            bitrateKbps = j.optInt("bitrate_kbps"),
            width = j.optInt("width"),
            height = j.optInt("height"),
            refreshMhz = j.optInt("refresh_mhz"),
            report = report,
        )
    }

    fun codecName(codec: String): String = when (codec.lowercase(Locale.ROOT)) {
        "h264" -> "H.264"
        "hevc" -> "HEVC"
        "av1" -> "AV1"
        else -> codec.uppercase(Locale.ROOT)
    }

    /** "46 Mbit/s": one decimal below 10, none from 10 up. */
    fun mbit(res: Resources, kbps: Int): String = res.getString(R.string.osd_mbit, mbitNumber(res, kbps))

    private fun locale(res: Resources): Locale = res.configuration.locales[0]

    private fun mbitNumber(res: Resources, kbps: Int): String {
        val mbit = kbps / 1000.0
        return if (mbit < 10) String.format(locale(res), "%.1f", mbit) else String.format(locale(res), "%d", Math.round(mbit))
    }

    private fun qualityName(res: Resources, q: String): String = when (q) {
        "high" -> res.getString(R.string.osd_quality_high)
        "lossless" -> res.getString(R.string.osd_quality_lossless)
        else -> res.getString(R.string.osd_quality_normal)
    }

    private fun hz(res: Resources, mhz: Int): String =
        if (mhz % 1000 == 0) (mhz / 1000).toString() else String.format(locale(res), "%.2f", mhz / 1000.0)

    private fun ellipsize(s: String): String =
        if (s.length <= MAX_ERROR_CHARS) s else s.take(MAX_ERROR_CHARS - 1).trimEnd() + "…"

    fun render(res: Resources, info: OsdInfo): OsdContent {
        val report = info.report
        val lines = mutableListOf<String>()
        if (report != null) {
            val parts = mutableListOf(res.getString(R.string.osd_negotiated, mbit(res, info.bitrateKbps)))
            parts += if (report.userBitrate) {
                res.getString(R.string.osd_set_in_settings)
            } else {
                res.getString(R.string.osd_auto_for, "${info.width}×${info.height}", hz(res, info.refreshMhz))
            }
            if (report.wantedKbps > report.ceilingKbps && report.ceilingKbps > 0) {
                parts += res.getString(
                    R.string.osd_clamped, mbitNumber(res, report.wantedKbps), mbitNumber(res, report.ceilingKbps),
                )
            }
            if (report.ceilingKbps > 0) parts += res.getString(R.string.osd_link_limit, mbit(res, report.ceilingKbps))
            lines += parts.joinToString(" · ")
            if (report.wantedQuality != "normal") {
                lines += if (report.appliedQuality == report.wantedQuality) {
                    res.getString(R.string.osd_quality, qualityName(res, report.wantedQuality))
                } else {
                    res.getString(
                        R.string.osd_quality_applied,
                        qualityName(res, report.wantedQuality), qualityName(res, report.appliedQuality),
                    )
                }
            }
            if (report.wantedCodec != "auto" && !report.wantedCodec.equals(info.codec, ignoreCase = true)) {
                lines += res.getString(R.string.osd_codec_wanted, codecName(report.wantedCodec))
            }
            val hidden = (report.attempts.size - MAX_ATTEMPTS).coerceAtLeast(0)
            if (hidden > 0) lines += res.getQuantityString(R.plurals.osd_more_attempts, hidden, hidden)
            for (a in report.attempts.drop(hidden)) {
                val head = "${if (a.ok) "✓" else "✗"} ${a.encoder}" + if (a.options.isNotEmpty()) "  ${a.options}" else ""
                lines += if (!a.ok && a.error.isNotEmpty()) "$head — ${ellipsize(a.error.lineSequence().first())}" else head
            }
        }
        val headline = mutableListOf(codecName(info.codec))
        if (report != null) {
            when (report.rateMode) {
                "cbr" -> res.getString(R.string.osd_rate_cbr)
                "vbr" -> res.getString(R.string.osd_rate_vbr, report.quality)
                "cqp" -> res.getString(R.string.osd_rate_cqp, report.quality)
                "crf" -> res.getString(R.string.osd_rate_crf, report.quality)
                "lossless" -> res.getString(R.string.osd_rate_lossless)
                else -> null
            }?.let { headline += it }
        }
        // A fixed quantiser or lossless stream has no target rate: the negotiated one would mislead.
        val fixedQuality = report?.rateMode == "cqp" || report?.rateMode == "lossless"
        if (!fixedQuality) {
            headline += if (report != null && report.maxKbps > info.bitrateKbps && info.bitrateKbps > 0) {
                res.getString(R.string.osd_rate_peak, mbitNumber(res, info.bitrateKbps), mbitNumber(res, report.maxKbps))
            } else {
                mbit(res, info.bitrateKbps)
            }
        }
        return OsdContent(lines, headline.joinToString(" · "))
    }
}

/** Small, non-interactive overlay for the bottom-left corner. */
class StreamOsdView(context: Context) : LinearLayout(context) {
    private val detail = TextView(context).apply {
        setTextColor(Color.WHITE)
        setTextSize(TypedValue.COMPLEX_UNIT_SP, 11f)
        typeface = Typeface.MONOSPACE
        maxLines = 10
        ellipsize = TextUtils.TruncateAt.END
    }
    private val headline = TextView(context).apply {
        setTextColor(Color.WHITE)
        setTextSize(TypedValue.COMPLEX_UNIT_SP, 19f)
        typeface = Typeface.DEFAULT_BOLD
    }

    init {
        orientation = VERTICAL
        val pad = (10 * resources.displayMetrics.density).toInt()
        setPadding(pad, pad, pad, pad)
        background = GradientDrawable().apply {
            setColor(0x99000000.toInt())
            cornerRadius = 8 * resources.displayMetrics.density
        }
        isClickable = false
        isFocusable = false
        importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_NO
        addView(detail, LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT))
        addView(headline, LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT))
        visibility = GONE
    }

    fun show(content: OsdContent) {
        detail.text = content.lines.joinToString("\n")
        detail.visibility = if (content.lines.isEmpty()) GONE else VISIBLE
        headline.text = content.headline
        animate().cancel()
        alpha = 1f
        visibility = VISIBLE
    }

    fun hide(animated: Boolean) {
        if (visibility != VISIBLE) return
        if (animated) {
            animate().alpha(0f).setDuration(StreamOsd.FADE_MS).withEndAction { visibility = GONE }
        } else {
            animate().cancel()
            visibility = GONE
        }
    }
}

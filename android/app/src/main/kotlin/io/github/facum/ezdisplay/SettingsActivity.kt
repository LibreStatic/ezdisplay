package io.github.facum.ezdisplay

import android.app.Activity
import android.content.SharedPreferences
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.util.TypedValue
import android.view.View
import android.view.ViewGroup
import android.widget.AdapterView
import android.widget.ArrayAdapter
import android.widget.LinearLayout
import android.widget.RadioButton
import android.widget.RadioGroup
import android.widget.ScrollView
import android.widget.SeekBar
import android.widget.Spinner
import android.widget.Switch
import android.widget.TextView
import java.util.Locale

/** Pen and touch input settings, stored in [InputPrefs]; MainActivity re-reads them on resume. */
class SettingsActivity : Activity() {
    private lateinit var prefs: SharedPreferences
    private val triggerSpinners = mutableListOf<Spinner>()

    private val actionLabels = mapOf(
        "raw" to R.string.action_raw,
        "right_click" to R.string.action_right_click,
        "middle_click" to R.string.action_middle_click,
        "local_zoom" to R.string.action_local_zoom,
        "eraser" to R.string.action_eraser,
        "scroll" to R.string.action_scroll,
        "pc_zoom" to R.string.action_pc_zoom,
        "none" to R.string.action_none,
    )

    private val profileLabels = listOf(
        Triple("raw", R.string.profile_raw, R.string.profile_raw_desc),
        Triple("desktop", R.string.profile_desktop, R.string.profile_desktop_desc),
        Triple("drawing", R.string.profile_drawing, R.string.profile_drawing_desc),
        Triple("custom", R.string.profile_custom, R.string.profile_custom_desc),
    )

    private class Trigger(val key: String, val label: Int, val default: String, val actions: List<String>)

    private val triggers = listOf(
        Trigger(InputPrefs.CUSTOM_CLICK, R.string.trigger_click, "raw",
            listOf("raw", "right_click", "middle_click", "local_zoom", "none")),
        Trigger(InputPrefs.CUSTOM_DOUBLE_CLICK, R.string.trigger_double_click, "local_zoom",
            listOf("right_click", "middle_click", "local_zoom", "none")),
        Trigger(InputPrefs.CUSTOM_HOLD, R.string.trigger_hold, "raw",
            listOf("raw", "right_click", "middle_click", "local_zoom", "eraser", "none")),
        Trigger(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT, R.string.trigger_hold_drag_contact, "raw",
            listOf("raw", "scroll", "pc_zoom", "none")),
        Trigger(InputPrefs.CUSTOM_HOLD_DRAG_HOVER, R.string.trigger_hold_drag_hover, "raw",
            listOf("raw", "scroll", "pc_zoom", "none")),
    )

    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()

    private fun text(res: Int, sp: Float = 14f, bold: Boolean = false) = TextView(this).apply {
        setText(res)
        setTextSize(TypedValue.COMPLEX_UNIT_SP, sp)
        if (bold) setTypeface(typeface, android.graphics.Typeface.BOLD)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        prefs = InputPrefs.prefs(this)
        val col = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(16), dp(16), dp(16), dp(24))
        }
        addProfileSection(col)
        addCustomSection(col)
        addZoomFactor(col)
        addDoubleClick(col)
        addPalm(col)
        airActionsNote()?.let { col.addView(it, matchWrap().apply { topMargin = dp(20) }) }
        setContentView(ScrollView(this).apply { addView(col) })
        updateCustomEnabled(prefs.getString(InputPrefs.PROFILE, "raw") == "custom")
    }

    private fun matchWrap() = LinearLayout.LayoutParams(
        ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT,
    )

    private fun addProfileSection(col: LinearLayout) {
        col.addView(text(R.string.settings_profile, 18f, bold = true))
        val group = RadioGroup(this).apply { orientation = RadioGroup.VERTICAL }
        val current = prefs.getString(InputPrefs.PROFILE, "raw")
        val ids = HashMap<Int, String>()
        for ((value, title, desc) in profileLabels) {
            val rb = RadioButton(this).apply {
                id = View.generateViewId()
                setText(title)
            }
            ids[rb.id] = value
            group.addView(rb)
            group.addView(text(desc, 13f).apply { setPadding(dp(40), 0, 0, dp(8)) })
            if (value == current) rb.isChecked = true
        }
        group.setOnCheckedChangeListener { _, checkedId ->
            val v = ids[checkedId] ?: return@setOnCheckedChangeListener
            prefs.edit().putString(InputPrefs.PROFILE, v).apply()
            updateCustomEnabled(v == "custom")
        }
        col.addView(group, matchWrap())
    }

    private fun addCustomSection(col: LinearLayout) {
        for (t in triggers) {
            col.addView(text(t.label).apply { setPadding(0, dp(8), 0, 0) })
            val sp = Spinner(this)
            sp.adapter = ArrayAdapter(
                this, android.R.layout.simple_spinner_dropdown_item,
                t.actions.map { getString(actionLabels.getValue(it)) },
            )
            sp.setSelection(t.actions.indexOf(prefs.getString(t.key, t.default)).coerceAtLeast(0))
            sp.onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
                override fun onItemSelected(p: AdapterView<*>?, v: View?, pos: Int, id: Long) {
                    prefs.edit().putString(t.key, t.actions[pos]).apply()
                }

                override fun onNothingSelected(p: AdapterView<*>?) {}
            }
            triggerSpinners += sp
            col.addView(sp, matchWrap())
        }
    }

    private fun updateCustomEnabled(enabled: Boolean) {
        for (s in triggerSpinners) {
            s.isEnabled = enabled
            s.alpha = if (enabled) 1f else 0.4f
        }
    }

    private fun addZoomFactor(col: LinearLayout) {
        col.addView(text(R.string.settings_zoom_factor, 18f, bold = true).apply { setPadding(0, dp(20), 0, 0) })
        val sp = Spinner(this)
        val labels = InputPrefs.ZOOM_FACTORS.map { String.format(Locale.getDefault(), "%s×", formatFactor(it)) }
        sp.adapter = ArrayAdapter(this, android.R.layout.simple_spinner_dropdown_item, labels)
        val cur = prefs.getFloat(InputPrefs.ZOOM_FACTOR, 2f)
        sp.setSelection(InputPrefs.ZOOM_FACTORS.indexOf(cur).let { if (it < 0) 1 else it })
        sp.onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
            override fun onItemSelected(p: AdapterView<*>?, v: View?, pos: Int, id: Long) {
                prefs.edit().putFloat(InputPrefs.ZOOM_FACTOR, InputPrefs.ZOOM_FACTORS[pos]).apply()
            }

            override fun onNothingSelected(p: AdapterView<*>?) {}
        }
        col.addView(sp, matchWrap())
    }

    private fun formatFactor(f: Float) = if (f % 1f == 0f) f.toInt().toString() else f.toString()

    private fun addDoubleClick(col: LinearLayout) {
        val label = text(R.string.settings_double_click, 14f).apply { setPadding(0, dp(20), 0, 0) }
        val ms = prefs.getInt(InputPrefs.DOUBLE_CLICK_MS, 300)
            .coerceIn(InputPrefs.DOUBLE_CLICK_MIN, InputPrefs.DOUBLE_CLICK_MAX)
        fun show(v: Int) { label.text = getString(R.string.settings_double_click, v) }
        show(ms)
        val bar = SeekBar(this).apply {
            max = (InputPrefs.DOUBLE_CLICK_MAX - InputPrefs.DOUBLE_CLICK_MIN) / InputPrefs.DOUBLE_CLICK_STEP
            progress = (ms - InputPrefs.DOUBLE_CLICK_MIN) / InputPrefs.DOUBLE_CLICK_STEP
            setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(s: SeekBar, p: Int, fromUser: Boolean) {
                    val v = InputPrefs.DOUBLE_CLICK_MIN + p * InputPrefs.DOUBLE_CLICK_STEP
                    show(v)
                    prefs.edit().putInt(InputPrefs.DOUBLE_CLICK_MS, v).apply()
                }

                override fun onStartTrackingTouch(s: SeekBar) {}
                override fun onStopTrackingTouch(s: SeekBar) {}
            })
        }
        col.addView(label, matchWrap())
        col.addView(bar, matchWrap())
    }

    private fun addPalm(col: LinearLayout) {
        val sw = Switch(this).apply {
            setText(R.string.settings_palm_rejection)
            isChecked = prefs.getBoolean(InputPrefs.PALM_REJECTION, true)
            setOnCheckedChangeListener { _, on -> prefs.edit().putBoolean(InputPrefs.PALM_REJECTION, on).apply() }
        }
        col.addView(sw, matchWrap().apply { topMargin = dp(20) })
    }

    /** Samsung S Pen "Air actions" can swallow button presses that start with the tip down. */
    private fun airActionsNote(): TextView? {
        if (!Build.MANUFACTURER.equals("samsung", ignoreCase = true)) return null
        val state = try {
            Settings.System.getInt(contentResolver, "spen_air_action", -1)
        } catch (e: Exception) {
            -1
        }
        return when (state) {
            0 -> null
            1 -> text(R.string.settings_air_actions_on)
            else -> text(R.string.settings_air_actions_unknown)
        }
    }
}

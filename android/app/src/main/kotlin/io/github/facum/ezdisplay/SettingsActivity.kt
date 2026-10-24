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
import android.widget.CheckBox
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
    /** Labels and spinners of the Custom section, shown only with the Custom profile. */
    private val customViews = mutableListOf<View>()
    private val dragSpinners = mutableListOf<Spinner>()
    private val keyEditors = mutableListOf<KeyEditor>()
    private lateinit var dragHint: TextView

    private val actionLabels = mapOf(
        "raw" to R.string.action_raw,
        "right_click" to R.string.action_right_click,
        "middle_click" to R.string.action_middle_click,
        "local_zoom" to R.string.action_local_zoom,
        "eraser" to R.string.action_eraser,
        "scroll" to R.string.action_scroll,
        "pc_zoom" to R.string.action_pc_zoom,
        "none" to R.string.action_none,
        "keys" to R.string.action_keys,
    )

    private val profileLabels = listOf(
        Triple("raw", R.string.profile_raw, R.string.profile_raw_desc),
        Triple("desktop", R.string.profile_desktop, R.string.profile_desktop_desc),
        Triple("drawing", R.string.profile_drawing, R.string.profile_drawing_desc),
        Triple("graphics_tablet", R.string.profile_graphics_tablet, R.string.profile_graphics_tablet_desc),
        Triple("custom", R.string.profile_custom, R.string.profile_custom_desc),
    )

    private class Trigger(val key: String, val label: Int, val default: String, val actions: List<String>)

    private val triggers = listOf(
        Trigger(InputPrefs.CUSTOM_CLICK, R.string.trigger_click, "raw",
            listOf("raw", "right_click", "middle_click", "local_zoom", "keys", "none")),
        Trigger(InputPrefs.CUSTOM_DOUBLE_CLICK, R.string.trigger_double_click, "local_zoom",
            listOf("right_click", "middle_click", "local_zoom", "keys", "none")),
        Trigger(InputPrefs.CUSTOM_HOLD, R.string.trigger_hold, "raw",
            listOf("raw", "right_click", "middle_click", "local_zoom", "eraser", "keys", "none")),
        Trigger(InputPrefs.CUSTOM_HOLD_DRAG_CONTACT, R.string.trigger_hold_drag_contact, "raw",
            listOf("raw", "scroll", "pc_zoom", "none")),
        Trigger(InputPrefs.CUSTOM_HOLD_DRAG_HOVER, R.string.trigger_hold_drag_hover, "raw",
            listOf("raw", "scroll", "pc_zoom", "none")),
    )

    private class KeyEditorSpec(val prefKey: String, val title: Int, val triggerKey: String)

    private val keyEditorSpecs = listOf(
        KeyEditorSpec(InputPrefs.KEYS_CLICK, R.string.keys_title_click, InputPrefs.CUSTOM_CLICK),
        KeyEditorSpec(InputPrefs.KEYS_DOUBLE_CLICK, R.string.keys_title_double_click, InputPrefs.CUSTOM_DOUBLE_CLICK),
        KeyEditorSpec(InputPrefs.KEYS_HOLD, R.string.keys_title_hold, InputPrefs.CUSTOM_HOLD),
    )

    private val presets = listOf(
        R.string.keys_preset_ctrl_space to InputPrefs.KeyCombo(InputPrefs.MOD_CTRL, 0x2C),
        R.string.keys_preset_space to InputPrefs.KeyCombo(0, 0x2C),
        R.string.keys_preset_alt_space to InputPrefs.KeyCombo(InputPrefs.MOD_ALT, 0x2C),
        R.string.keys_preset_ctrl_z to InputPrefs.KeyCombo(InputPrefs.MOD_CTRL, 0x1D),
        R.string.keys_preset_ctrl_shift_z to InputPrefs.KeyCombo(InputPrefs.MOD_CTRL or InputPrefs.MOD_SHIFT, 0x1D),
    )

    /** Editor for one key combination. Every change is written to prefs at once; [sync] mirrors prefs into the widgets. */
    private inner class KeyEditor(val spec: KeyEditorSpec) {
        val view = LinearLayout(this@SettingsActivity).apply { orientation = LinearLayout.VERTICAL }
        private val current = TextView(this@SettingsActivity)
        private val presetSpinner = Spinner(this@SettingsActivity)
        private val keySpinner = Spinner(this@SettingsActivity)
        private val mods = listOf(
            InputPrefs.MOD_CTRL to R.string.keys_mod_ctrl,
            InputPrefs.MOD_SHIFT to R.string.keys_mod_shift,
            InputPrefs.MOD_ALT to R.string.keys_mod_alt,
            InputPrefs.MOD_META to R.string.keys_mod_meta,
        )
        private val boxes = mods.map { CheckBox(this@SettingsActivity).apply { setText(it.second) } }
        private var syncing = false

        private fun combo() = InputPrefs.keyCombo(this@SettingsActivity, spec.prefKey)

        /** Saves [c] unless it is already stored (spinner callbacks can also fire after programmatic updates). */
        private fun save(c: InputPrefs.KeyCombo) {
            if (c == combo()) return
            prefs.edit().putString(spec.prefKey, c.format()).apply()
            sync()
        }

        init {
            view.setPadding(dp(16), dp(4), 0, dp(8))
            view.addView(text(spec.title, 14f, bold = true))
            view.addView(current, matchWrap())
            presetSpinner.adapter = ArrayAdapter(
                this@SettingsActivity, android.R.layout.simple_spinner_dropdown_item,
                presets.map { getString(it.first) } + getString(R.string.keys_preset_custom),
            )
            presetSpinner.onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
                override fun onItemSelected(p: AdapterView<*>?, v: View?, pos: Int, id: Long) {
                    if (syncing || pos >= presets.size) return
                    save(presets[pos].second)
                }

                override fun onNothingSelected(p: AdapterView<*>?) {}
            }
            view.addView(presetSpinner, matchWrap())
            val row = LinearLayout(this@SettingsActivity).apply { orientation = LinearLayout.HORIZONTAL }
            boxes.forEachIndexed { i, box ->
                box.setOnCheckedChangeListener { _, on ->
                    if (syncing) return@setOnCheckedChangeListener
                    val bit = mods[i].first
                    val c = combo()
                    save(c.copy(mods = if (on) c.mods or bit else c.mods and bit.inv()))
                }
                row.addView(box, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
            }
            view.addView(row, matchWrap())
            keySpinner.adapter = ArrayAdapter(
                this@SettingsActivity, android.R.layout.simple_spinner_dropdown_item,
                InputPrefs.KEY_USAGES.map { InputPrefs.keyLabel(this@SettingsActivity, it) },
            )
            keySpinner.onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
                override fun onItemSelected(p: AdapterView<*>?, v: View?, pos: Int, id: Long) {
                    if (syncing) return
                    save(combo().copy(usage = InputPrefs.KEY_USAGES[pos]))
                }

                override fun onNothingSelected(p: AdapterView<*>?) {}
            }
            view.addView(keySpinner, matchWrap())
            view.addView(text(R.string.keys_note, 12f), matchWrap())
            sync()
        }

        fun sync() {
            val c = combo()
            syncing = true
            try {
                current.text = getString(R.string.keys_current, c.label(this@SettingsActivity))
                val preset = presets.indexOfFirst { it.second == c }
                presetSpinner.setSelection(if (preset >= 0) preset else presets.size)
                mods.forEachIndexed { i, m -> boxes[i].isChecked = c.mods and m.first != 0 }
                keySpinner.setSelection(InputPrefs.KEY_USAGES.indexOf(c.usage).coerceAtLeast(0))
            } finally {
                syncing = false
            }
        }
    }

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
        refreshDependents()
    }

    private fun refreshDependents() {
        val profile = prefs.getString(InputPrefs.PROFILE, "raw")
        updateCustomEnabled(profile == "custom")
        for (e in keyEditors) {
            val show = when (profile) {
                "graphics_tablet" -> e.spec.prefKey == InputPrefs.KEYS_HOLD
                "custom" -> prefs.getString(e.spec.triggerKey, null) == "keys"
                else -> false
            }
            e.view.visibility = if (show) View.VISIBLE else View.GONE
        }
        val holdKeys = profile == "custom" && prefs.getString(InputPrefs.CUSTOM_HOLD, "raw") == "keys"
        dragHint.visibility = if (holdKeys) View.VISIBLE else View.GONE
        if (holdKeys) for (s in dragSpinners) {
            s.isEnabled = false
            s.alpha = 0.4f
        }
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
            refreshDependents()
        }
        col.addView(group, matchWrap())
    }

    private fun addCustomSection(col: LinearLayout) {
        dragHint = text(R.string.keys_drag_unused, 12f)
        for (t in triggers) {
            val isDrag = t.key == InputPrefs.CUSTOM_HOLD_DRAG_CONTACT || t.key == InputPrefs.CUSTOM_HOLD_DRAG_HOVER
            if (t.key == InputPrefs.CUSTOM_HOLD_DRAG_CONTACT) col.addView(dragHint, matchWrap())
            val label = text(t.label).apply { setPadding(0, dp(8), 0, 0) }
            col.addView(label)
            val sp = Spinner(this)
            sp.adapter = ArrayAdapter(
                this, android.R.layout.simple_spinner_dropdown_item,
                t.actions.map { getString(actionLabels.getValue(it)) },
            )
            sp.setSelection(t.actions.indexOf(prefs.getString(t.key, t.default)).coerceAtLeast(0))
            sp.onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
                override fun onItemSelected(p: AdapterView<*>?, v: View?, pos: Int, id: Long) {
                    if (prefs.getString(t.key, t.default) == t.actions[pos]) return
                    prefs.edit().putString(t.key, t.actions[pos]).apply()
                    refreshDependents()
                }

                override fun onNothingSelected(p: AdapterView<*>?) {}
            }
            triggerSpinners += sp
            customViews += listOf(label, sp)
            if (isDrag) dragSpinners += sp
            col.addView(sp, matchWrap())
            keyEditorSpecs.firstOrNull { it.triggerKey == t.key }?.let {
                val ed = KeyEditor(it)
                keyEditors += ed
                col.addView(ed.view, matchWrap())
            }
        }
    }

    /** The Custom section is hidden with the preset profiles: its values are not what they do. */
    private fun updateCustomEnabled(enabled: Boolean) {
        for (v in customViews) v.visibility = if (enabled) View.VISIBLE else View.GONE
        for (s in triggerSpinners) {
            s.isEnabled = enabled
            s.alpha = 1f
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

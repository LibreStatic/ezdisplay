package com.librestatic.ezdisplay.display

import org.junit.Assert.assertEquals
import org.junit.Test

class PanelTypeTest {
    @Test fun oledNames() {
        listOf("ss_fhd_s6e3hc3_amoled", "AMOLED", "samsung oled panel", "s6e3fc3", "ea8076", "ams646yb01", "rm69299", "nt37801")
            .forEach { assertEquals(it, PanelType.Oled, classifyPanelName(it)) }
    }

    @Test fun lcdNames() {
        listOf("boe_nt36672_tft_lcd", "IPS panel", "lcd", "tianma_ili9881", "nt36525b")
            .forEach { assertEquals(it, PanelType.Lcd, classifyPanelName(it)) }
    }

    @Test fun oledWinsOverLcdWhenBothAppear() {
        assertEquals(PanelType.Oled, classifyPanelName("tft amoled"))
    }

    @Test fun unknownNames() {
        listOf("", "dsi_panel_cmd", "something else").forEach { assertEquals(it, PanelType.Unknown, classifyPanelName(it)) }
    }

    @Test fun modelTable() {
        mapOf(
            "SM-X910" to PanelType.Oled, "SM-X810" to PanelType.Oled, "SM-T970" to PanelType.Oled,
            "SM-S918B" to PanelType.Oled, "SM-G991B" to PanelType.Oled, "SM-N986B" to PanelType.Oled,
            "SM-F946B" to PanelType.Oled, "SM-F711B" to PanelType.Oled,
            "Pixel 8 Pro" to PanelType.Oled, "Pixel 6a" to PanelType.Oled,
            "Pixel Tablet" to PanelType.Lcd, "Pixel C" to PanelType.Lcd,
            "SM-T870" to PanelType.Unknown, "SM-X200" to PanelType.Unknown, "Nexus 7" to PanelType.Unknown, "" to PanelType.Unknown,
        ).forEach { (model, expected) -> assertEquals(model, expected, classifyModel(model)) }
    }

    private fun detector(
        files: Map<String, String> = emptyMap(),
        dirs: Map<String, List<String>> = emptyMap(),
        props: Map<String, String> = emptyMap(),
        model: String = "Generic",
    ) = PanelDetector({ files[it] }, { dirs[it] ?: emptyList() }, { props[it] }, model)

    @Test fun sysfsFileWins() {
        val d = detector(
            files = mapOf("/sys/class/lcd/panel/lcd_type" to "ss_amoled"),
            props = mapOf("ro.vendor.display.panel_name" to "ips"),
            model = "Pixel Tablet",
        )
        assertEquals(PanelType.Oled, d.detect(null))
    }

    @Test fun drmPanelNameIsListed() {
        val d = detector(
            dirs = mapOf("/sys/class/drm" to listOf("card0", "card0-DSI-1")),
            files = mapOf("/sys/class/drm/card0-DSI-1/panel_name" to "boe_nt36672_lcd"),
        )
        assertEquals(PanelType.Lcd, d.detect(null))
    }

    @Test fun propsAreUsedWhenSysfsHasNothing() {
        assertEquals(PanelType.Oled, detector(props = mapOf("vendor.display.panel_type" to "AMOLED")).detect(null))
    }

    @Test fun unknownSysfsFallsThroughToProps() {
        val d = detector(files = mapOf("/sys/class/lcd/panel/lcd_type" to "dsi_panel"), props = mapOf("persist.vendor.display.panel_name" to "tft"))
        assertEquals(PanelType.Lcd, d.detect(null))
    }

    @Test fun modelIsUsedBeforeLuminance() {
        assertEquals(PanelType.Lcd, detector(model = "Pixel Tablet").detect(0.001f))
        assertEquals(PanelType.Oled, detector(model = "SM-X910").detect(null))
    }

    @Test fun luminanceTieBreaker() {
        val d = detector()
        assertEquals(PanelType.Oled, d.detect(0.0005f))
        assertEquals(PanelType.Oled, d.detect(0.005f))
        assertEquals(PanelType.Unknown, d.detect(0f))
        assertEquals(PanelType.Unknown, d.detect(0.1f))
        assertEquals(PanelType.Unknown, d.detect(null))
    }

    @Test fun throwingReadersAreTolerated() {
        val d = PanelDetector({ error("denied") }, { error("denied") }, { error("denied") }, "SM-S918B")
        assertEquals(PanelType.Oled, d.detect(null))
    }
}

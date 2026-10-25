package com.librestatic.ezdisplay.wifi

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class WifiModelTest {
    private val fpA = "3f2a91c077de0b14" + "00".repeat(24)
    private val fpB = "aaaabbbbccccdddd" + "11".repeat(24)

    @Test fun pinsAreSixDigits() {
        assertTrue(isValidPin("123456"))
        assertTrue(isValidPin("000000"))
        listOf("", "12345", "1234567", "12345a", " 12345", "١٢٣٤٥٦").forEach { assertFalse(it, isValidPin(it)) }
    }

    @Test fun shortFingerprintGroupsTheFirstSixteenDigits() {
        assertEquals("3f2a-91c0-77de-0b14", shortFingerprint(fpA.uppercase()))
        assertTrue(fingerprintMatches(fpA, "3f2a-91c0-77de-0b14"))
        assertTrue(fingerprintMatches(fpA.uppercase(), "3F2A-91C0-77DE-0B14"))
        assertFalse(fingerprintMatches(fpA, "3f2a-91c0-77de-0b15"))
        // Too short a prefix identifies nothing.
        assertFalse(fingerprintMatches(fpA, ""))
        assertFalse(fingerprintMatches(fpA, "3f2a"))
    }

    @Test fun parsesAPairingLink() {
        val link = parsePairUri("ezd://pair?a=192.168.1.5:27183&fp=$fpA&pin=123456&n=Studio%20PC%20%C3%A9")
        assertNotNull(link)
        assertEquals("192.168.1.5:27183", link!!.addr)
        assertEquals(fpA, link.fingerprint)
        assertEquals("123456", link.pin)
        assertEquals("Studio PC é", link.name)
        assertEquals("3f2a-91c0-77de-0b14", link.shortFp)
    }

    @Test fun aPairingLinkSurvivesTheRoundTripToTheNativeSide() {
        val link = PairLink("[fe80::1]:27183", fpA, "000123", "A&B = C+D %é")
        assertEquals(link, parsePairUri(link.toUri()))
        // The host decodes %XX only, so no raw spaces or plus signs.
        assertFalse(link.toUri().contains(' '))
        assertEquals("A%26B%20%3D%20C%2BD%20%25%C3%A9", percentEncode(link.name))
    }

    @Test fun rejectsBadPairingLinks() {
        val ok = "ezd://pair?a=10.0.0.2:27183&fp=$fpA&pin=123456&n=x"
        assertNotNull(parsePairUri(ok))
        assertNotNull(parsePairUri("  $ok  "))
        listOf(
            null, "", "https://example.com", "ezd://pair", "ezd://pair?",
            "ezd://pair?a=10.0.0.2:27183&fp=$fpA&pin=12345",
            "ezd://pair?a=10.0.0.2:27183&fp=$fpA&pin=abcdef",
            "ezd://pair?a=10.0.0.2:27183&fp=abcd&pin=123456",
            "ezd://pair?a=10.0.0.2:27183&pin=123456",
            "ezd://pair?a=not-an-address&fp=$fpA&pin=123456",
            "ezd://pair?a=300.0.0.1:27183&fp=$fpA&pin=123456",
            "ezd://pair?fp=$fpA&pin=123456",
            "ezd://pair?a=10.0.0.2:27183&fp=$fpA&pin=123456&n=%zz",
            "ezd://pair?a=10.0.0.2:27183&fp=$fpA&pin=123456&broken",
        ).forEach { assertNull(it, parsePairUri(it)) }
    }

    @Test fun aLinkWithoutANameIsStillValid() {
        assertEquals("", parsePairUri("ezd://pair?a=10.0.0.2:27183&fp=$fpA&pin=123456")!!.name)
    }

    @Test fun manualAddressesGetTheDefaultPort() {
        assertEquals("192.168.1.5:27183", parseManualAddress("192.168.1.5"))
        assertEquals("192.168.1.5:4000", parseManualAddress(" 192.168.1.5:4000 "))
        assertEquals("[fe80::1]:27183", parseManualAddress("fe80::1"))
        assertEquals("[2001:db8::2]:5000", parseManualAddress("[2001:db8::2]:5000"))
        assertEquals("[2001:db8::2]:27183", parseManualAddress("[2001:db8::2]"))
        listOf("", "   ", "desk", "1.2.3", "300.1.1.1", "1.2.3.4:0", "1.2.3.4:70000", "1.2.3.4:x", "1.2.3.4:", "[::1", "::zz")
            .forEach { assertNull(it, parseManualAddress(it)) }
    }

    @Test fun parsesHostsJson() {
        val json = """[{"name":"desk","fingerprint":"$fpA","shortFp":"3f2a-91c0-77de-0b14","addr":"10.0.0.2:27183"},
            {"name":"laptop","fingerprint":"$fpB","shortFp":"aaaa-bbbb-cccc-dddd"}]"""
        val hosts = parseHostsJson(json)
        assertEquals(2, hosts.size)
        assertEquals(WifiHost("desk", fpA, "3f2a-91c0-77de-0b14", "10.0.0.2:27183"), hosts[0])
        assertNull(hosts[1].addr)
        assertEquals(emptyList<WifiHost>(), parseHostsJson("[]"))
        assertEquals(emptyList<WifiHost>(), parseHostsJson("nope"))
    }

    @Test fun parsesPairResults() {
        val ok = parsePairResultJson(
            """{"ok":true,"host":{"name":"desk","fingerprint":"$fpA","shortFp":"3f2a-91c0-77de-0b14"}}""",
        )
        assertTrue(ok.ok)
        assertEquals("desk", ok.host!!.name)
        val bad = parsePairResultJson("""{"ok":false,"error":"wrong_pin","message":"refused"}""")
        assertFalse(bad.ok)
        assertEquals("wrong_pin", bad.error)
        assertEquals("other", parsePairResultJson("garbage").error)
    }

    @Test fun choosesTheLiveHostThenTheLastOneThenAStoredAddress() {
        val a = WifiHost("a", fpA, shortFingerprint(fpA), "10.0.0.2:27183")
        val b = WifiHost("b", fpB, shortFingerprint(fpB), "10.0.0.3:27183")
        val noAddr = WifiHost("c", "ff".repeat(32), "ffff-ffff-ffff-ffff", null)
        val seenB = DiscoveredHost("b-svc", "b", "aaaa-bbbb-cccc-dddd", "192.168.1.9:27183")

        // Discovery beats everything and brings the fresh address.
        assertEquals(WifiChoice(b, "192.168.1.9:27183"), chooseWifiTarget(listOf(a, b), listOf(seenB), fpA))
        // Nothing discovered: the last used host with its stored address.
        assertEquals(WifiChoice(b, "10.0.0.3:27183"), chooseWifiTarget(listOf(a, b), emptyList(), fpB))
        assertEquals(WifiChoice(a, "10.0.0.2:27183"), chooseWifiTarget(listOf(a, b), emptyList(), null))
        // A host with no address is skipped; none at all means no Wi-Fi attempt.
        assertEquals(WifiChoice(a, "10.0.0.2:27183"), chooseWifiTarget(listOf(noAddr, a), emptyList(), noAddr.fingerprint))
        assertNull(chooseWifiTarget(listOf(noAddr), emptyList(), null))
        assertNull(chooseWifiTarget(emptyList(), listOf(seenB), null))
        // A discovered host nobody paired is never chosen.
        assertNull(chooseWifiTarget(listOf(noAddr), listOf(seenB), null))
    }

    @Test fun spoofedAnnouncementsAreRotatedOut() {
        val real = WifiHost("Real", fpA, shortFingerprint(fpA), "10.0.0.2:27183")
        val spoof = DiscoveredHost("evil", "Real", shortFingerprint(fpA), "192.168.1.66:27183")
        val genuine = DiscoveredHost("real", "Real", shortFingerprint(fpA), "192.168.1.9:27183")
        val seen = listOf(spoof, genuine)
        // The first announcement wins until it fails.
        assertEquals(WifiChoice(real, spoof.addr), chooseWifiTarget(listOf(real), seen, fpA))
        // Then the other discovered address, then the stored one.
        val avoid = mutableSetOf(fpA to spoof.addr)
        assertEquals(WifiChoice(real, genuine.addr), chooseWifiTarget(listOf(real), seen, fpA, avoid))
        avoid += fpA to genuine.addr
        assertEquals(WifiChoice(real, "10.0.0.2:27183"), chooseWifiTarget(listOf(real), seen, fpA, avoid))
        // Only avoided candidates left: still try one rather than never connecting.
        avoid += fpA to "10.0.0.2:27183"
        assertEquals(WifiChoice(real, spoof.addr), chooseWifiTarget(listOf(real), seen, fpA, avoid))
        // Another host is preferred to an avoided address of the first.
        val other = WifiHost("Other", fpB, shortFingerprint(fpB), "10.0.0.3:27183")
        assertEquals(WifiChoice(other, "10.0.0.3:27183"), chooseWifiTarget(listOf(real, other), emptyList(), fpA, setOf(fpA to "10.0.0.2:27183")))
    }

    @Test fun failuresAreRememberedForAWhile() {
        val f = WifiFailures(ttlMs = 60_000)
        f.record(fpA, "1.2.3.4:1", 1_000)
        assertEquals(setOf(fpA to "1.2.3.4:1"), f.active(30_000))
        assertTrue(f.active(61_000).isEmpty())
        f.record(fpA, "1.2.3.4:1", 100_000)
        f.clear(fpA, "1.2.3.4:1")
        assertTrue(f.active(100_001).isEmpty())
    }

    @Test fun localAddressesAreOnTheLocalNetwork() {
        listOf(
            "10.0.0.1", "10.255.0.1:1234", "172.16.0.1", "172.31.255.255", "192.168.1.5:27183", "169.254.1.1",
            "100.64.0.1", "100.127.1.1", "[fc00::1]:27183", "fd12:3456::1", "fe80::1", "[febf::1]:5",
        ).forEach { assertTrue(it, isLocalAddress(it)) }
        listOf(
            "8.8.8.8", "203.0.113.9:27183", "172.15.0.1", "172.32.0.1", "192.169.0.1", "100.63.0.1", "100.128.0.1",
            "127.0.0.1", "2001:db8::1", "[2001:db8::1]:27183", "fec0::1", "::1", "::ffff:192.168.1.5", "garbage", "",
        ).forEach { assertFalse(it, isLocalAddress(it)) }
    }

    @Test fun namesLoseControlAndFormatCharacters() {
        val spoof = "Of\u202Efice\u202C\u2066 PC\u2069\u200E\u200F\u061C\u0007\n\uFEFF"
        assertEquals("Office PC", cleanName(spoof))
        assertEquals("Studio PC", cleanName("  Studio PC  "))
        assertEquals(64, cleanName("x".repeat(100)).length)
        val link = parsePairUri("ezd://pair?a=10.0.0.2:27183&fp=$fpA&pin=123456&n=Of%E2%80%AEfice%E2%80%AC%E2%81%A6%20PC")
        assertEquals("Office PC", link!!.name)
    }
}

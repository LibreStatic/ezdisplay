## Befehlszeile
about = ezdisplay: Nutze dein Tablet als zweiten Bildschirm. Läuft im Infobereich der Taskleiste.
arg-settings = Öffnet das Einstellungsfenster.
arg-quit = Beendet das laufende ezdisplay und wartet, bis es gestoppt ist.
arg-autostart = Schaltet den Start mit der Sitzung ein oder aus und beendet sich dann.
usage-label = Aufruf:
heading-options = Optionen
help-flag = Hilfe anzeigen
version-flag = Version anzeigen

## Zustand des Dienstes
status-stopped = Angehalten
status-stopping = Wird angehalten…
status-waiting = Warte auf das Tablet
status-unauthorized = Bestätige das USB-Debugging auf dem Tablet
status-streaming = Übertragung an { $device } · { $width }×{ $height } mit { $hz } Hz
error-bind = Auf { $addr } kann nicht gelauscht werden: { $reason }
error-adb-not-found = adb wurde nicht gefunden
error-adb-reverse = adb reverse ist für { $serial } fehlgeschlagen: { $reason }
error-unsupported = Dieses System unterstützt noch keine virtuellen Bildschirme
error-source = Die Videoquelle kann nicht erstellt werden: { $reason }

## Menü des Infobereichs
menu-start = Starten
menu-stop = Anhalten
menu-scale = Skalierung
menu-scale-option = { $percent } %
menu-telemetry = Telemetrie-Overlay
menu-settings = Einstellungen…
menu-autostart = Mit der Sitzung starten
menu-logs = Protokollordner öffnen
menu-quit = Beenden

## Fenster: Bereiche
section-status = Status
section-display = Anzeige
section-input = Eingabe
section-connection = Verbindung
section-general = Allgemein
section-driver = Anzeigetreiber

## Fenster: Status
devices-heading = Geräte
stats-heading = Live-Statistik
stats-waiting = Warte auf die ersten Messwerte…
stat-device = Tablet
stat-mode = Modus
stat-encoder = Encoder
stat-fps = Bildrate
stat-bitrate = Bitrate
stat-e2e = Latenz von der Aufnahme bis zur Anzeige
stat-decode = Dekodierzeit
stat-transfer = Übertragungszeit
stat-input = Eingabelatenz
stat-drops = Verworfene Bilder
stat-rtt = Rundlaufzeit
mode-value = { $width }×{ $height } mit { $hz } Hz, { $codec }
encoder-value = { $encoder } mit { $mbps } Mbit/s
value-fps = { $fps } fps
value-mbps = { $mbps } Mbit/s
value-hz = { $hz } Hz
value-ms = { $ms } ms
value-ms-pair = { $p50 } ms (95. Perzentil: { $p95 } ms)
value-drops = { $dropped } vom Tablet, { $late ->
        [one] { $late } verspätete Bestätigung
       *[other] { $late } verspätete Bestätigungen
    }

## Fenster: Anzeige
display-hint = Änderungen gelten ab der nächsten Verbindung. Nutze „Jetzt anwenden“, um sofort neu zu verbinden.
setting-scale = Skalierung
setting-refresh = Höchste Bildwiederholrate
setting-bitrate = Bitrate
bitrate-auto = Automatisch
setting-encoder = Encoder
encoder-auto = Automatisch
setting-gpu = Grafikkarte
gpu-auto = Automatisch
gpu-prefer-dgpu = Dedizierte Grafikkarte bevorzugen
gpu-named = Nach Namen wählen
gpu-name-hint = Adaptername, wie von „ezd-host diag“ angezeigt
apply-now = Jetzt anwenden (verbindet neu)

## Fenster: Eingabe
input-enabled = Touch- und Stifteingaben an diesen Computer senden
setting-pen = Der Stift arbeitet als
pen-tablet = Grafiktablett (Druck und Neigung)
pen-mouse = Mauszeiger
telemetry = Telemetrie-Overlay auf dem Tablet anzeigen
input-permission-denied = Touch- und Stifteingaben brauchen die Erlaubnis, Eingabegeräte zu erstellen (/dev/uinput).
input-permission-missing = Touch- und Stifteingaben brauchen das Kernelmodul uinput, das nicht geladen ist (/dev/uinput fehlt). Beim Aktivieren der Eingabe wird es geladen und die Berechtigung eingerichtet.
input-notice = Touch- und Stifteingaben sind blockiert: ezdisplay darf keine Eingabegeräte erstellen.
input-enable = Eingabe aktivieren
input-enable-running = Warte auf die Autorisierung…
input-enable-done = Die Eingabe ist aktiviert.
input-enable-cancelled = Abgebrochen. Es wurde nichts geändert.
input-enable-no-agent = Es ist kein Authentifizierungs-Agent verfügbar. Installiere die udev-Regel von Hand, siehe README.
input-enable-still-denied = Die Regel ist installiert, der Zugriff wird aber weiterhin verweigert. Melde dich ab und wieder an und versuche es dann erneut.
input-enable-failed = Das Aktivieren der Eingabe ist fehlgeschlagen (Code { $code }).
input-enable-error = Das Aktivieren der Eingabe konnte nicht ausgeführt werden: { $reason }

## Fenster: Verbindung
adb-sdk = { $path } (Android SDK)
adb-bundled = { $path } (in ezdisplay enthalten)
devices-count =
    { $count ->
        [one] { $count } Gerät
       *[other] { $count } Geräte
    }
devices-none = Keine Geräte gefunden. Verbinde das Tablet per USB-Kabel und schalte das USB-Debugging ein.
samsung-driver = Samsung Android USB-Treiber
device-line = { $name } ({ $serial }): { $state }
device-ready = bereit
device-unauthorized = wartet darauf, dass du das USB-Debugging bestätigst
device-offline = offline
open-app = Die App auf dem Tablet öffnen, sobald es verbunden ist
use-adb = Per USB mit adb verbinden
advanced = Erweitert
setting-port = Port
port-hint = Das Tablet erreicht diesen Computer über diesen Port. Änderungen gelten, wenn keine Sitzung läuft.

## Fenster: Allgemein
start-on-launch = Übertragen, wenn ezdisplay startet
about-version = ezdisplay { $version }
licenses = Lizenzen

## Fenster: Anzeigetreiber
driver-state = Treiber
driver-installed = Installiert
driver-missing = Nicht installiert
driver-owner = Installiert von
driver-owner-ours = ezdisplay
driver-owner-other = einem anderen Programm (bleibt unverändert)
driver-owner-none = —
driver-folder = Treiberordner
driver-hint = Reparieren installiert den mit ezdisplay gelieferten Treiber für den virtuellen Bildschirm neu. Windows fragt zuerst um Erlaubnis.
driver-repair = Reparieren
repair-running = Wird repariert…
repair-cancelled = Abgebrochen. Es wurde nichts geändert.
repair-done = Der Treiber ist installiert.
repair-restart = Starte Windows neu, um die Installation des Treibers abzuschließen.
repair-failed = Die Reparatur ist fehlgeschlagen (Code { $code }).
repair-log = Details: { $path }
repair-error = Die Reparatur konnte nicht ausgeführt werden: { $reason }

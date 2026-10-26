## Riga di comando
about = ezdisplay: usa il tablet come secondo schermo. Gira nell'area di notifica.
arg-settings = Apre la finestra delle impostazioni.
arg-quit = Chiude l'ezdisplay in esecuzione e attende che si sia fermato.
arg-autostart = Attiva o disattiva l'avvio con la sessione, poi esce.
arg-elevation = Attiva o disattiva l'esecuzione come amministratore, poi esce.
usage-label = Uso:
heading-options = Opzioni
help-flag = Mostra l'aiuto
version-flag = Mostra la versione

## Stato del servizio
status-stopped = Fermo
status-stopping = Arresto in corso…
status-waiting = In attesa del tablet
status-unauthorized = Accetta il debug USB sul tablet
status-streaming = Trasmissione verso { $device } · { $width }×{ $height } a { $hz } Hz
error-bind = Impossibile mettersi in ascolto su { $addr }: { $reason }
error-adb-not-found = adb non trovato
error-adb-reverse = adb reverse non riuscito per { $serial }: { $reason }
error-unsupported = Questo sistema non supporta ancora gli schermi virtuali
error-source = Impossibile creare la sorgente video: { $reason }

## Menu dell'area di notifica
menu-start = Avvia
menu-stop = Ferma
menu-scale = Scala
menu-scale-option = { $percent }%
menu-telemetry = Overlay di telemetria
menu-settings = Impostazioni…
menu-autostart = Avvia con la sessione
menu-logs = Apri la cartella dei log
menu-quit = Esci

## Finestra: sezioni
section-status = Stato
section-display = Schermo
section-input = Input
section-connection = Connessione
section-general = Generale
section-driver = Driver dello schermo

## Finestra: stato
devices-heading = Dispositivi
stats-heading = Statistiche in tempo reale
stats-waiting = In attesa delle prime misurazioni…
stat-device = Tablet
stat-mode = Modalità
stat-encoder = Encoder
stat-fps = Frequenza dei fotogrammi
stat-bitrate = Bitrate
stat-e2e = Latenza dalla cattura allo schermo
stat-decode = Tempo di decodifica
stat-transfer = Tempo di trasferimento
stat-input = Latenza dell'input
stat-drops = Fotogrammi persi
stat-rtt = Andata e ritorno
mode-value = { $width }×{ $height } a { $hz } Hz, { $codec }
encoder-value = { $encoder } a { $mbps } Mbps
unit-fps = fps
unit-mbps = Mbps
value-hz = { $hz } Hz
value-ms = { $ms } ms
value-seconds = { $seconds } s
value-ms-pair = { $p50 } ms (95° percentile: { $p95 } ms)
value-drops = { $dropped } dal tablet, { $late ->
        [one] { $late } conferma in ritardo
       *[other] { $late } conferme in ritardo
    }

## Finestra: schermo
display-hint = Le modifiche valgono dalla prossima connessione. Usa «Applica ora» per riconnetterti subito.
setting-scale = Scala
setting-refresh = Frequenza di aggiornamento massima
setting-bitrate = Bitrate
bitrate-auto = Automatico
setting-encoder = Encoder
encoder-auto = Automatico
setting-gpu = Scheda grafica
gpu-auto = Automatica
gpu-prefer-dgpu = Preferisci la scheda grafica dedicata
gpu-named = Scegli per nome
gpu-name-hint = Nome dell'adattatore, come mostrato da «ezd-host diag»
apply-now = Applica ora (riconnette)
setting-hdr = HDR
setting-detach-grace = Rimuovi dopo
hdr-auto = Automatico
hdr-off = Disattivato
hdr-hint-linux = KDE non invia ancora l'HDR agli schermi virtuali; lo streaming resta in SDR.
stat-hdr = HDR
mode-value-hdr = { $width }×{ $height } a { $hz } Hz, { $codec } HDR10
hdr-reason-turned-off = L'HDR è disattivato nelle impostazioni; conversione in SDR
hdr-reason-old-client = L'app del tablet è troppo vecchia per l'HDR; aggiornala
hdr-reason-display-not-hdr = Lo schermo del tablet non supporta l'HDR10
hdr-reason-no-main10-decoder = Il tablet non può decodificare video HDR a 10 bit
hdr-reason-no-main10-encoder = Questa scheda grafica non può codificare HEVC a 10 bit; conversione in SDR

## Finestra: input
input-enabled = Invia l'input touch e della penna a questo computer
setting-pen = La penna funziona come
pen-tablet = Tavoletta grafica
pen-mouse = Puntatore del mouse
telemetry = Mostra l'overlay di telemetria sul tablet
input-permission-denied = L'input touch e della penna ha bisogno del permesso di creare dispositivi di input (/dev/uinput).
input-permission-missing = L'input touch e della penna ha bisogno del modulo kernel uinput, che non è caricato (manca /dev/uinput). Abilitando l'input viene caricato e il permesso viene configurato.
input-notice = L'input touch e della penna è bloccato: ezdisplay non ha il permesso di creare dispositivi di input.
input-enable = Abilita l'input
input-enable-running = In attesa dell'autorizzazione…
input-enable-done = L'input è abilitato.
input-enable-cancelled = Annullato. Non è cambiato nulla.
input-enable-no-agent = Nessun agente di autenticazione disponibile. Installa la regola udev a mano, vedi il README.
input-enable-still-denied = La regola è installata, ma l'accesso è ancora negato. Esci dalla sessione, rientra e riprova.
input-enable-failed = Abilitazione dell'input non riuscita (codice { $code }).
input-enable-error = Impossibile eseguire l'abilitazione dell'input: { $reason }

## Finestra: connessione
adb-sdk = { $path } (Android SDK)
adb-bundled = { $path } (incluso in ezdisplay)
devices-count =
    { $count ->
        [one] { $count } dispositivo
       *[other] { $count } dispositivi
    }
devices-none = Collega il tablet con un cavo USB e attiva il debug USB.
samsung-driver = Driver USB Android di Samsung
device-line = { $name } ({ $serial }): { $state }
device-ready = pronto
device-unauthorized = in attesa che accetti il debug USB
device-offline = offline
open-app = Apri l'app sul tablet quando si connette
use-adb = Connetti via USB con adb
advanced = Avanzate
setting-port = Porta
port-hint = Il tablet raggiunge questo computer su questa porta. Le modifiche valgono quando non c'è nessuna sessione in corso.

## Finestra: generale
start-on-launch = Trasmetti all'apertura di ezdisplay
setting-run-elevated = Controlla le finestre di amministratore (esegui con privilegi elevati)
run-elevated-hint = Permette alla penna e al tocco di raggiungere le finestre eseguite come amministratore, come Gestione attività. Vale dopo il riavvio di ezdisplay.
about-version = ezdisplay { $version }
licenses = Licenze

## Finestra: driver dello schermo
driver-state = Driver
driver-installed = Installato
driver-missing = Non installato
driver-owner = Installato da
driver-owner-ours = ezdisplay
driver-owner-other = un altro programma (non toccato)
driver-owner-none = —
driver-folder = Cartella del driver
driver-hint = Ripara reinstalla il driver dello schermo virtuale fornito con ezdisplay. Windows chiede prima il permesso.
driver-repair = Ripara
repair-running = Riparazione in corso…
repair-cancelled = Annullato. Non è cambiato nulla.
repair-done = Il driver è installato.
repair-restart = Riavvia Windows per completare l'installazione del driver.
repair-failed = La riparazione non è riuscita (codice { $code }).
repair-log = Dettagli: { $path }
repair-error = Impossibile eseguire la riparazione: { $reason }

## Window: cards and row descriptions
devices-empty = Nessun dispositivo rilevato
stats-empty = Le statistiche compaiono quando inizia lo streaming
card-stream = Streaming
card-measurements = Misurazioni
card-picture = Qualità dell'immagine
card-encoding = Codifica
card-virtual-monitor = Monitor virtuale
card-adb = Android Debug Bridge
card-startup = Avvio
card-appearance = Aspetto
card-diagnostics = Diagnostica
card-about = Informazioni
card-driver = Stato del driver
card-maintenance = Manutenzione
scale-description = Valori più bassi inviano meno pixel al tablet.
refresh-description = Limita la frequenza di aggiornamento offerta al tablet.
bitrate-description = Qualità video dello streaming. In automatico la sceglie ezdisplay.
encoder-description = L'encoder video che comprime lo streaming.
hdr-description = Invia HDR10 quando lo schermo del tablet lo supporta.
detach-grace-description = Quando il tablet va in sospensione, si spegne o si disconnette, il monitor virtuale viene rimosso dopo questo tempo e le sue finestre passano agli altri monitor.
gpu-description = La scheda grafica che cattura e codifica lo schermo.
input-description = Usa il tablet come touchscreen e penna su questo computer.
pen-description = Una tavoletta grafica aggiunge pressione e inclinazione.
telemetry-description = Mostra sul tablet valori di prestazione in tempo reale durante lo streaming.
use-adb-description = Trova i tablet con il debug USB attivo.
autostart-description = Avvia ezdisplay quando accedi.
start-on-launch-description = Si mette in attesa del tablet appena ezdisplay si apre.
system-accent = Usa il colore d'accento del sistema
system-accent-description = Usa il colore d'accento che hai scelto nelle impostazioni di sistema. Se disattivato, ezdisplay usa il proprio colore.
logs-title = Log
logs-description = Apre la cartella in cui ezdisplay salva i log.
about-description = Schermo USB open source per tablet Android.
repository = Codice sorgente
driver-reinstall = Reinstalla il driver

## Window: Wi-Fi
card-wifi = Wi-Fi
wifi-enable = Connetti tramite Wi-Fi
wifi-latency-warning = Il Wi-Fi aggiunge latenza e jitter; l'USB è il migliore per disegnare.
wifi-starts-with-stream = Il Wi-Fi si attiva quando inizia lo streaming.
wifi-address = Indirizzo
wifi-fingerprint = Impronta dell'host
wifi-pair = Associa un dispositivo
wifi-pair-cancel = Annulla
wifi-pair-hint = Sul tablet, inquadra questo codice QR o digita il PIN.
wifi-pin = PIN
wifi-qr-label = Codice QR di associazione
wifi-pair-expires = Si chiude tra { $seconds } s
wifi-notice-paired = Associato con { $device }.
wifi-notice-attempts = Associazione chiusa: troppi PIN errati.
wifi-notice-expired = Associazione chiusa: il tempo è scaduto.
wifi-paired-count =
    { $count ->
        [one] { $count } dispositivo associato
       *[other] { $count } dispositivi associati
    }
wifi-paired-none = Nessun dispositivo è ancora associato.
wifi-forget = Dimentica
error-wifi = Impossibile avviare il Wi-Fi: { $reason }

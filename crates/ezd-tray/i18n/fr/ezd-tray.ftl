## Ligne de commande
about = ezdisplay : utilisez votre tablette comme second écran. Fonctionne dans la zone de notification.
arg-settings = Ouvre la fenêtre des réglages.
arg-quit = Quitte l'instance d'ezdisplay en cours et attend qu'elle soit arrêtée.
arg-autostart = Active ou désactive le démarrage avec la session, puis quitte.
arg-elevation = Active ou désactive l'exécution en tant qu'administrateur, puis quitte.
usage-label = Utilisation :
heading-options = Options
help-flag = Affiche l'aide
version-flag = Affiche la version

## État du service
status-stopped = Arrêté
status-stopping = Arrêt en cours…
status-waiting = En attente de la tablette
status-unauthorized = Acceptez le débogage USB sur la tablette
status-streaming = Diffusion vers { $device } · { $width }×{ $height } à { $hz } Hz
error-bind = Impossible d'écouter sur { $addr } : { $reason }
error-adb-not-found = adb est introuvable
error-adb-reverse = Échec d'adb reverse pour { $serial } : { $reason }
usb-unavailable-hint = L'USB sans mode développeur est indisponible ({ $reason }) ; le débogage USB fonctionne toujours.
error-unsupported = Ce système ne prend pas encore en charge les écrans virtuels
error-source = Impossible de créer la source vidéo : { $reason }

## Menu de la zone de notification
menu-start = Démarrer
menu-stop = Arrêter
menu-scale = Échelle
menu-scale-option = { $percent } %
menu-telemetry = Incrustation de télémétrie
menu-settings = Réglages…
menu-autostart = Démarrer avec la session
menu-logs = Ouvrir le dossier des journaux
menu-quit = Quitter

## Fenêtre : sections
section-status = État
section-display = Affichage
section-input = Saisie
section-connection = Connexion
section-general = Général
section-driver = Pilote d'affichage

## Fenêtre : état
devices-heading = Appareils
stats-heading = Statistiques en direct
stats-waiting = En attente des premières mesures…
stat-device = Tablette
stat-connection = Connexion
transport-aoa = USB (accessoire)
transport-wifi = Wi-Fi
stat-mode = Mode
stat-encoder = Encodeur
stat-fps = Fréquence d'images
stat-bitrate = Débit
stat-e2e = Latence de la capture à l'écran
stat-decode = Temps de décodage
stat-transfer = Temps de transfert
stat-input = Latence de saisie
stat-drops = Images perdues
stat-rtt = Aller-retour
mode-value = { $width }×{ $height } à { $hz } Hz, { $codec }
encoder-value = { $encoder } à { $mbps } Mb/s
unit-fps = i/s
unit-mbps = Mb/s
value-hz = { $hz } Hz
value-ms = { $ms } ms
value-ms-pair = { $p50 } ms (95e centile : { $p95 } ms)
value-drops = { $dropped } par la tablette, { $late ->
        [one] { $late } accusé de réception tardif
       *[other] { $late } accusés de réception tardifs
    }

## Fenêtre : affichage
display-hint = Les modifications s'appliquent à la prochaine connexion. Utilisez « Appliquer maintenant » pour vous reconnecter tout de suite.
setting-scale = Échelle
setting-refresh = Fréquence de rafraîchissement maximale
setting-bitrate = Débit
bitrate-auto = Automatique
setting-encoder = Encodeur
encoder-auto = Automatique
setting-gpu = Carte graphique
gpu-auto = Automatique
gpu-prefer-dgpu = Préférer la carte graphique dédiée
gpu-named = Choisir par nom
gpu-name-hint = Nom de l'adaptateur, comme affiché par « ezd-host diag »
apply-now = Appliquer maintenant (reconnecte)
setting-hdr = HDR
hdr-auto = Automatique
hdr-off = Désactivé
hdr-hint-linux = KDE n'envoie pas encore le HDR aux écrans virtuels ; le flux reste en SDR.
stat-hdr = HDR
mode-value-hdr = { $width }×{ $height } à { $hz } Hz, { $codec } HDR10
hdr-reason-turned-off = Le HDR est désactivé dans les réglages ; conversion en SDR
hdr-reason-old-client = L'application de la tablette est trop ancienne pour le HDR ; mettez-la à jour
hdr-reason-display-not-hdr = L'écran de la tablette ne prend pas en charge le HDR10
hdr-reason-no-main10-decoder = La tablette ne peut pas décoder la vidéo HDR 10 bits
hdr-reason-no-main10-encoder = Cette carte graphique ne peut pas encoder du HEVC 10 bits ; conversion en SDR
setting-audio = Audio
audio-description = Diffuse le son de cet ordinateur vers la tablette. Automatique envoie l'audio non compressé quand la liaison le permet.
audio-auto = Automatique
audio-pcm = PCM (non compressé)
audio-opus = Opus (compressé)
audio-off = Désactivé
stat-audio = Audio
audio-value-none = Aucun
audio-value-pcm = PCM { $format }, { $khz } kHz, { $layout }
audio-value-opus = Opus, { $khz } kHz, { $layout }
audio-mono = mono
audio-stereo = stéréo
audio-format-s16 = 16 bits
audio-format-f32 = 32 bits flottant

## Fenêtre : saisie
input-enabled = Envoyer la saisie tactile et du stylet à cet ordinateur
setting-pen = Le stylet fonctionne comme
pen-tablet = Tablette graphique
pen-mouse = Pointeur de souris
telemetry = Afficher l'incrustation de télémétrie sur la tablette
input-permission-denied = La saisie tactile et du stylet a besoin de l'autorisation de créer des périphériques d'entrée (/dev/uinput).
input-permission-missing = La saisie tactile et du stylet a besoin du module noyau uinput, qui n'est pas chargé (/dev/uinput est absent). Activer la saisie le charge et configure l'autorisation.
input-notice = La saisie tactile et du stylet est bloquée : ezdisplay n'a pas l'autorisation de créer des périphériques d'entrée.
input-enable = Activer la saisie
input-enable-running = En attente de l'autorisation…
input-enable-done = La saisie est activée.
input-enable-cancelled = Annulé. Rien n'a été modifié.
input-enable-no-agent = Aucun agent d'authentification n'est disponible. Installez la règle udev à la main, voir le README.
input-enable-still-denied = La règle est installée, mais l'accès est toujours refusé. Fermez la session, reconnectez-vous, puis réessayez.
input-enable-failed = L'activation de la saisie a échoué (code { $code }).
input-enable-error = L'activation de la saisie n'a pas pu s'exécuter : { $reason }

## Fenêtre : connexion
adb-sdk = { $path } (Android SDK)
adb-bundled = { $path } (fourni avec ezdisplay)
devices-count =
    { $count ->
        [one] { $count } appareil
       *[other] { $count } appareils
    }
devices-none = Branchez la tablette avec un câble USB et activez le débogage USB.
samsung-driver = Pilote USB Android de Samsung
device-line = { $name } ({ $serial }) : { $state }
device-ready = prête
device-unauthorized = en attente de votre acceptation du débogage USB
device-offline = hors ligne
open-app = Ouvrir l'application sur la tablette à la connexion
use-adb = Se connecter en USB avec adb
use-aoa = USB sans mode développeur
advanced = Avancé
setting-port = Port
port-hint = La tablette joint cet ordinateur sur ce port. Les modifications s'appliquent quand aucune session n'est en cours.

## Fenêtre : général
start-on-launch = Diffuser à l'ouverture d'ezdisplay
setting-run-elevated = Contrôler les fenêtres d'administrateur (exécuter avec élévation)
run-elevated-hint = Permet au stylet et au toucher d'atteindre les fenêtres exécutées en tant qu'administrateur, comme le Gestionnaire des tâches. S'applique après le redémarrage d'ezdisplay.
about-version = ezdisplay { $version }
licenses = Licences

## Fenêtre : pilote d'affichage
driver-state = Pilote
driver-installed = Installé
driver-missing = Non installé
driver-owner = Installé par
driver-owner-ours = ezdisplay
driver-owner-other = un autre programme (laissé tel quel)
driver-owner-none = —
driver-folder = Dossier du pilote
driver-hint = Réparer réinstalle le pilote d'écran virtuel fourni avec ezdisplay. Windows demande d'abord l'autorisation.
driver-repair = Réparer
repair-running = Réparation en cours…
repair-cancelled = Annulé. Rien n'a été modifié.
repair-done = Le pilote est installé.
repair-restart = Redémarrez Windows pour terminer l'installation du pilote.
repair-failed = La réparation a échoué (code { $code }).
repair-log = Détails : { $path }
repair-error = La réparation n'a pas pu s'exécuter : { $reason }

## Window: cards and row descriptions
devices-empty = Aucun appareil détecté
stats-empty = Les statistiques apparaissent au début de la diffusion
card-stream = Flux
card-measurements = Mesures
card-picture = Qualité d'image
card-encoding = Encodage
card-adb = Android Debug Bridge
card-startup = Démarrage
card-appearance = Apparence
card-diagnostics = Diagnostic
card-about = À propos
card-driver = État du pilote
card-maintenance = Maintenance
scale-description = Des valeurs plus basses envoient moins de pixels à la tablette.
refresh-description = Limite la fréquence de rafraîchissement proposée à la tablette.
bitrate-description = Qualité vidéo du flux. En automatique, ezdisplay la choisit pour vous.
encoder-description = L'encodeur vidéo qui compresse le flux.
hdr-description = Envoie du HDR10 quand l'écran de la tablette le prend en charge.
gpu-description = La carte graphique qui capture et encode l'écran.
input-description = Utilisez la tablette comme écran tactile et comme stylet sur cet ordinateur.
pen-description = Une tablette graphique ajoute la pression et l'inclinaison.
telemetry-description = Affiche des mesures de performance en direct sur la tablette pendant la diffusion.
use-adb-description = Trouve les tablettes dont le débogage USB est activé.
use-aoa-description = Fait passer la tablette en mode accessoire dès que vous la branchez, sans débogage USB. adb reste disponible en secours.
autostart-description = Lance ezdisplay à l'ouverture de votre session.
start-on-launch-description = Se met en attente de votre tablette dès l'ouverture d'ezdisplay.
system-accent = Utiliser la couleur d'accentuation du système
system-accent-description = Reprend la couleur d'accentuation choisie dans les paramètres du système. Désactivé, ezdisplay utilise sa propre couleur.
logs-title = Journaux
logs-description = Ouvre le dossier où ezdisplay écrit ses journaux.
about-description = Écran USB open source pour tablettes Android.
repository = Code source
driver-reinstall = Réinstaller le pilote

## Window: Wi-Fi
card-wifi = Wi-Fi
wifi-enable = Se connecter par Wi-Fi
wifi-latency-warning = Le Wi-Fi ajoute de la latence et de la gigue ; l'USB est préférable pour dessiner.
wifi-starts-with-stream = Le Wi-Fi s'active quand la diffusion démarre.
wifi-address = Adresse
wifi-fingerprint = Empreinte de l'hôte
wifi-pair = Appairer un appareil
wifi-pair-cancel = Annuler
wifi-pair-hint = Sur la tablette, scannez ce QR code ou saisissez le code PIN.
wifi-pin = Code PIN
wifi-qr-label = QR code d'appairage
wifi-pair-expires = Se ferme dans { $seconds } s
wifi-notice-paired = Appairage réussi avec { $device }.
wifi-notice-attempts = L'appairage est fermé : trop de codes PIN incorrects.
wifi-notice-expired = L'appairage est fermé : le temps est écoulé.
wifi-paired-count =
    { $count ->
        [one] { $count } appareil appairé
       *[other] { $count } appareils appairés
    }
wifi-paired-none = Aucun appareil n'est encore appairé.
wifi-forget = Oublier
error-wifi = Impossible de démarrer le Wi-Fi : { $reason }

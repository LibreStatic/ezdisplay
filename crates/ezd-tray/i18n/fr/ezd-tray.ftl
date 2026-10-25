## Ligne de commande
about = ezdisplay : utilisez votre tablette comme second écran. Fonctionne dans la zone de notification.
arg-settings = Ouvre la fenêtre des réglages.
arg-quit = Quitte l'instance d'ezdisplay en cours et attend qu'elle soit arrêtée.
arg-autostart = Active ou désactive le démarrage avec la session, puis quitte.
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
value-fps = { $fps } i/s
value-mbps = { $mbps } Mb/s
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

## Fenêtre : saisie
input-enabled = Envoyer la saisie tactile et du stylet à cet ordinateur
setting-pen = Le stylet fonctionne comme
pen-tablet = Tablette graphique (pression et inclinaison)
pen-mouse = Pointeur de souris
telemetry = Afficher l'incrustation de télémétrie sur la tablette

## Fenêtre : connexion
adb-sdk = { $path } (Android SDK)
adb-bundled = { $path } (fourni avec ezdisplay)
devices-count =
    { $count ->
        [one] { $count } appareil
       *[other] { $count } appareils
    }
devices-none = Aucun appareil. Branchez la tablette avec un câble USB et activez le débogage USB.
samsung-driver = Pilote USB Android de Samsung
device-line = { $name } ({ $serial }) : { $state }
device-ready = prête
device-unauthorized = en attente de votre acceptation du débogage USB
device-offline = hors ligne
open-app = Ouvrir l'application sur la tablette à la connexion
use-adb = Se connecter en USB avec adb
advanced = Avancé
setting-port = Port
port-hint = La tablette joint cet ordinateur sur ce port. Les modifications s'appliquent quand aucune session n'est en cours.

## Fenêtre : général
start-on-launch = Diffuser à l'ouverture d'ezdisplay
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

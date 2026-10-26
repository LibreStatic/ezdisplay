## Línea de comandos
about = ezdisplay: usá tu tablet como segunda pantalla. Se ejecuta en la bandeja del sistema.
arg-settings = Abre la ventana de ajustes.
arg-quit = Cierra el ezdisplay en ejecución y espera a que se detenga.
arg-autostart = Activa o desactiva el inicio con la sesión y termina.
arg-elevation = Activa o desactiva la ejecución como administrador y termina.
usage-label = Uso:
heading-options = Opciones
help-flag = Muestra la ayuda
version-flag = Muestra la versión

## Estado del servicio
status-stopped = Detenido
status-stopping = Deteniendo…
status-waiting = Esperando a la tablet
status-unauthorized = Aceptá la depuración USB en la tablet
status-streaming = Transmitiendo a { $device } · { $width }×{ $height } a { $hz } Hz
error-bind = No se puede escuchar en { $addr }: { $reason }
error-adb-not-found = No se encontró adb
error-adb-reverse = Falló adb reverse para { $serial }: { $reason }
usb-unavailable-hint = USB sin modo desarrollador no está disponible ({ $reason }); la depuración USB sigue funcionando.
error-unsupported = Este sistema todavía no admite pantallas virtuales
error-source = No se puede crear la fuente de video: { $reason }

## Menú de la bandeja
menu-start = Iniciar
menu-stop = Detener
menu-scale = Escala
menu-scale-option = { $percent }%
menu-telemetry = Superposición de telemetría
menu-settings = Ajustes…
menu-autostart = Iniciar con la sesión
menu-logs = Abrir la carpeta de registros
menu-quit = Salir

## Ventana: secciones
section-status = Estado
section-display = Pantalla
section-input = Entrada
section-connection = Conexión
section-general = General
section-driver = Controlador de pantalla

## Ventana: estado
devices-heading = Dispositivos
stats-heading = Estadísticas en vivo
stats-waiting = Esperando las primeras mediciones…
stat-device = Tablet
stat-connection = Conexión
transport-aoa = USB (accesorio)
transport-wifi = Wi-Fi
stat-mode = Modo
stat-encoder = Codificador
stat-fps = Cuadros por segundo
stat-bitrate = Tasa de bits
stat-e2e = Latencia de captura a pantalla
stat-decode = Tiempo de decodificación
stat-transfer = Tiempo de transferencia
stat-input = Latencia de entrada
stat-drops = Cuadros descartados
stat-rtt = Ida y vuelta
mode-value = { $width }×{ $height } a { $hz } Hz, { $codec }
encoder-value = { $encoder } a { $mbps } Mbps
unit-fps = fps
unit-mbps = Mbps
value-hz = { $hz } Hz
value-ms = { $ms } ms
value-ms-pair = { $p50 } ms (percentil 95: { $p95 } ms)
value-drops = { $dropped } por la tablet, { $late ->
        [one] { $late } confirmación tardía
       *[other] { $late } confirmaciones tardías
    }

## Ventana: pantalla
display-hint = Los cambios se aplican desde la próxima conexión. Usá «Aplicar ahora» para reconectar enseguida.
setting-scale = Escala
setting-refresh = Frecuencia de actualización máxima
setting-bitrate = Tasa de bits
bitrate-auto = Automática
setting-encoder = Codificador
encoder-auto = Automático
setting-gpu = Placa de video
gpu-auto = Automática
gpu-prefer-dgpu = Preferir la placa de video dedicada
gpu-named = Elegir por nombre
gpu-name-hint = Nombre del adaptador, como lo muestra «ezd-host diag»
apply-now = Aplicar ahora (reconecta)
setting-hdr = HDR
hdr-auto = Automático
hdr-off = Apagado
hdr-hint-linux = KDE todavía no envía HDR a las pantallas virtuales; la transmisión sigue en SDR.
stat-hdr = HDR
mode-value-hdr = { $width }×{ $height } a { $hz } Hz, { $codec } HDR10
hdr-reason-turned-off = El HDR está apagado en los ajustes; se convierte a SDR
hdr-reason-old-client = La app de la tablet es muy vieja para HDR; actualizala
hdr-reason-display-not-hdr = La pantalla de la tablet no admite HDR10
hdr-reason-no-main10-decoder = La tablet no puede decodificar video HDR de 10 bits
hdr-reason-no-main10-encoder = Esta placa de video no puede codificar HEVC de 10 bits; se convierte a SDR
setting-audio = Audio
audio-description = Transmite el sonido de esta computadora a la tablet. Automático envía audio sin comprimir cuando el enlace lo permite.
audio-auto = Automático
audio-pcm = PCM (sin comprimir)
audio-opus = Opus (comprimido)
audio-off = Apagado
stat-audio = Audio
audio-value-none = Ninguno
audio-value-pcm = PCM { $format }, { $khz } kHz, { $layout }
audio-value-opus = Opus, { $khz } kHz, { $layout }
audio-mono = mono
audio-stereo = estéreo
audio-format-s16 = 16 bits
audio-format-f32 = 32 bits flotante

## Ventana: entrada
input-enabled = Enviar la entrada táctil y del lápiz a esta computadora
setting-pen = El lápiz actúa como
pen-tablet = Tableta gráfica
pen-mouse = Puntero de mouse
telemetry = Mostrar la superposición de telemetría en la tablet
input-permission-denied = La entrada táctil y del lápiz necesita permiso para crear dispositivos de entrada (/dev/uinput).
input-permission-missing = La entrada táctil y del lápiz necesita el módulo uinput del kernel, que no está cargado (falta /dev/uinput). Al habilitar la entrada se carga y se configura el permiso.
input-notice = La entrada táctil y del lápiz está bloqueada: ezdisplay no tiene permiso para crear dispositivos de entrada.
input-enable = Habilitar la entrada
input-enable-running = Esperando la autorización…
input-enable-done = La entrada está habilitada.
input-enable-cancelled = Cancelado. No se cambió nada.
input-enable-no-agent = No hay ningún agente de autenticación disponible. Instalá la regla de udev a mano, mirá el README.
input-enable-still-denied = La regla está instalada, pero el acceso sigue denegado. Cerrá la sesión, volvé a entrar y probá de nuevo.
input-enable-failed = No se pudo habilitar la entrada (código { $code }).
input-enable-error = No se pudo ejecutar la habilitación de la entrada: { $reason }

## Ventana: conexión
adb-sdk = { $path } (Android SDK)
adb-bundled = { $path } (incluido con ezdisplay)
devices-count =
    { $count ->
        [one] { $count } dispositivo
       *[other] { $count } dispositivos
    }
devices-none = Conectá la tablet con un cable USB y activá la depuración USB.
samsung-driver = Controlador USB de Android de Samsung
device-line = { $name } ({ $serial }): { $state }
device-ready = lista
device-unauthorized = esperando que aceptes la depuración USB
device-offline = sin conexión
open-app = Abrir la app en la tablet al conectarla
use-adb = Conectar por USB con adb
use-aoa = USB sin modo desarrollador
advanced = Avanzado
setting-port = Puerto
port-hint = La tablet llega a esta computadora por este puerto. Los cambios se aplican cuando no hay una sesión en curso.

## Ventana: general
start-on-launch = Transmitir al abrir ezdisplay
setting-run-elevated = Controlar ventanas de administrador (ejecutar con privilegios elevados)
run-elevated-hint = Permite que el lápiz y el toque lleguen a ventanas que se ejecutan como administrador, como el Administrador de tareas. Se aplica al reiniciar ezdisplay.
about-version = ezdisplay { $version }
licenses = Licencias

## Ventana: controlador de pantalla
driver-state = Controlador
driver-installed = Instalado
driver-missing = No instalado
driver-owner = Instalado por
driver-owner-ours = ezdisplay
driver-owner-other = otro programa (no se toca)
driver-owner-none = —
driver-folder = Carpeta del controlador
driver-hint = Reparar vuelve a instalar el controlador de pantalla virtual que incluye ezdisplay. Windows pide permiso antes.
driver-repair = Reparar
repair-running = Reparando…
repair-cancelled = Cancelado. No se cambió nada.
repair-done = El controlador está instalado.
repair-restart = Reiniciá Windows para terminar de instalar el controlador.
repair-failed = La reparación falló (código { $code }).
repair-log = Detalles: { $path }
repair-error = No se pudo ejecutar la reparación: { $reason }

## Window: cards and row descriptions
devices-empty = No se detectaron dispositivos
stats-empty = Las estadísticas aparecen cuando empieza la transmisión
card-stream = Transmisión
card-measurements = Mediciones
card-picture = Calidad de imagen
card-encoding = Codificación
card-adb = Android Debug Bridge
card-startup = Inicio
card-appearance = Apariencia
card-diagnostics = Diagnóstico
card-about = Acerca de
card-driver = Estado del controlador
card-maintenance = Mantenimiento
scale-description = Los valores más bajos envían menos píxeles a la tablet.
refresh-description = Limita la frecuencia de actualización que se ofrece a la tablet.
bitrate-description = Calidad del video de la transmisión. En automático la elige por vos.
encoder-description = El codificador de video que comprime la transmisión.
hdr-description = Envía HDR10 cuando la pantalla de la tablet lo admite.
gpu-description = La placa de video que captura y codifica la pantalla.
input-description = Usá la tablet como pantalla táctil y lápiz en esta computadora.
pen-description = Una tableta gráfica agrega presión e inclinación.
telemetry-description = Muestra mediciones de rendimiento en vivo en la tablet mientras transmitís.
use-adb-description = Encuentra las tablets que tienen la depuración USB activada.
use-aoa-description = Pasa la tablet al modo accesorio al conectarla, así no hace falta la depuración USB. adb queda como alternativa.
autostart-description = Inicia ezdisplay cuando iniciás sesión.
start-on-launch-description = Empieza a esperar tu tablet apenas se abre ezdisplay.
system-accent = Usar el color de acento del sistema
system-accent-description = Usa el color de acento que elegiste en la configuración del sistema. Si está apagado, ezdisplay usa su propio color.
logs-title = Registros
logs-description = Abre la carpeta donde ezdisplay guarda sus registros.
about-description = Pantalla USB de código abierto para tablets Android.
repository = Código fuente
driver-reinstall = Reinstalar el controlador

## Window: Wi-Fi
card-wifi = Wi-Fi
wifi-enable = Conectar por Wi-Fi
wifi-latency-warning = El Wi-Fi agrega latencia y jitter; el USB es lo mejor para dibujar.
wifi-starts-with-stream = El Wi-Fi se activa cuando empieza la transmisión.
wifi-address = Dirección
wifi-fingerprint = Huella del host
wifi-pair = Vincular un dispositivo
wifi-pair-cancel = Cancelar
wifi-pair-hint = En la tablet, escaneá este código QR o escribí el PIN.
wifi-pin = PIN
wifi-qr-label = Código QR de vinculación
wifi-pair-expires = Se cierra en { $seconds } s
wifi-notice-paired = Vinculada con { $device }.
wifi-notice-attempts = La vinculación se cerró: demasiados PIN incorrectos.
wifi-notice-expired = La vinculación se cerró: se acabó el tiempo.
wifi-paired-count =
    { $count ->
        [one] { $count } dispositivo vinculado
       *[other] { $count } dispositivos vinculados
    }
wifi-paired-none = Todavía no hay dispositivos vinculados.
wifi-forget = Olvidar
error-wifi = No se puede iniciar el Wi-Fi: { $reason }

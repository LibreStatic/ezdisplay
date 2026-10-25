## Línea de comandos
about = ezdisplay: usá tu tablet como segunda pantalla. Se ejecuta en la bandeja del sistema.
arg-settings = Abre la ventana de ajustes.
arg-quit = Cierra el ezdisplay en ejecución y espera a que se detenga.
arg-autostart = Activa o desactiva el inicio con la sesión y termina.
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
value-fps = { $fps } fps
value-mbps = { $mbps } Mbps
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

## Ventana: entrada
input-enabled = Enviar la entrada táctil y del lápiz a esta computadora
setting-pen = El lápiz actúa como
pen-tablet = Tableta gráfica (presión e inclinación)
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
devices-none = No hay dispositivos. Conectá la tablet con un cable USB y activá la depuración USB.
samsung-driver = Controlador USB de Android de Samsung
device-line = { $name } ({ $serial }): { $state }
device-ready = lista
device-unauthorized = esperando que aceptes la depuración USB
device-offline = sin conexión
open-app = Abrir la app en la tablet al conectarla
use-adb = Conectar por USB con adb
advanced = Avanzado
setting-port = Puerto
port-hint = La tablet llega a esta computadora por este puerto. Los cambios se aplican cuando no hay una sesión en curso.

## Ventana: general
start-on-launch = Transmitir al abrir ezdisplay
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

## Command line
about = ezdisplay: use your tablet as a second display. Runs in the system tray.
arg-settings = Open the settings window.
arg-quit = Quit the running ezdisplay, waiting until it has stopped.
arg-autostart = Turn starting with the session on or off, then exit.
usage-label = Usage:
heading-options = Options
help-flag = Print help
version-flag = Print version

## State of the service
status-stopped = Stopped
status-stopping = Stopping…
status-waiting = Waiting for the tablet
status-unauthorized = Accept USB debugging on the tablet
status-streaming = Streaming to { $device } · { $width }×{ $height } at { $hz } Hz
error-bind = Cannot listen on { $addr }: { $reason }
error-adb-not-found = adb was not found
error-adb-reverse = adb reverse failed for { $serial }: { $reason }
error-unsupported = This system has no virtual display support yet
error-source = Cannot create the video source: { $reason }

## Tray menu
menu-start = Start
menu-stop = Stop
menu-scale = Scale
menu-scale-option = { $percent }%
menu-telemetry = Telemetry overlay
menu-settings = Settings…
menu-autostart = Start with the session
menu-logs = Open logs folder
menu-quit = Quit

## Window: sections
section-status = Status
section-display = Display
section-input = Input
section-connection = Connection
section-general = General
section-driver = Display driver

## Window: status
devices-heading = Devices
stats-heading = Live statistics
stats-waiting = Waiting for the first measurements…
stat-device = Tablet
stat-mode = Mode
stat-encoder = Encoder
stat-fps = Frame rate
stat-bitrate = Bitrate
stat-e2e = Capture-to-screen latency
stat-decode = Decode time
stat-transfer = Transfer time
stat-input = Input latency
stat-drops = Dropped frames
stat-rtt = Round trip
mode-value = { $width }×{ $height } at { $hz } Hz, { $codec }
encoder-value = { $encoder } at { $mbps } Mbps
value-fps = { $fps } fps
value-mbps = { $mbps } Mbps
value-hz = { $hz } Hz
value-ms = { $ms } ms
value-ms-pair = { $p50 } ms (95th percentile: { $p95 } ms)
value-drops = { $dropped } by the tablet, { $late ->
        [one] { $late } late acknowledgement
       *[other] { $late } late acknowledgements
    }

## Window: display
display-hint = Changes apply from the next connection. Use "Apply now" to reconnect right away.
setting-scale = Scale
setting-refresh = Highest refresh rate
setting-bitrate = Bitrate
bitrate-auto = Automatic
setting-encoder = Encoder
encoder-auto = Automatic
setting-gpu = Graphics card
gpu-auto = Automatic
gpu-prefer-dgpu = Prefer the discrete graphics card
gpu-named = Choose by name
gpu-name-hint = Adapter name, as shown by "ezd-host diag"
apply-now = Apply now (reconnects)

## Window: input
input-enabled = Send touch and pen input to this computer
setting-pen = Pen acts as
pen-tablet = Graphics tablet (pressure and tilt)
pen-mouse = Mouse pointer
telemetry = Show the telemetry overlay on the tablet

## Window: connection
adb-sdk = { $path } (Android SDK)
adb-bundled = { $path } (bundled with ezdisplay)
devices-count =
    { $count ->
        [one] { $count } device
       *[other] { $count } devices
    }
devices-none = No devices listed. Connect the tablet with a USB cable and turn on USB debugging.
samsung-driver = Samsung Android USB driver
device-line = { $name } ({ $serial }): { $state }
device-ready = ready
device-unauthorized = waiting for you to accept USB debugging
device-offline = offline
open-app = Open the app on the tablet when it is connected
use-adb = Connect over USB with adb
advanced = Advanced
setting-port = Port
port-hint = The tablet reaches this computer on this port. Changes apply when no session is running.

## Window: general
start-on-launch = Stream when ezdisplay opens
about-version = ezdisplay { $version }
licenses = Licenses

## Window: display driver
driver-state = Driver
driver-installed = Installed
driver-missing = Not installed
driver-owner = Installed by
driver-owner-ours = ezdisplay
driver-owner-other = another program (left untouched)
driver-owner-none = —
driver-folder = Driver folder
driver-hint = Repair reinstalls the virtual display driver that ships with ezdisplay. Windows asks for permission first.
driver-repair = Repair
repair-running = Repairing…
repair-cancelled = Cancelled. Nothing was changed.
repair-done = The driver is installed.
repair-restart = Restart Windows to finish installing the driver.
repair-failed = The repair failed (code { $code }).
repair-log = Details: { $path }
repair-error = The repair could not run: { $reason }

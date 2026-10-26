## Command line
about = ezdisplay: use your tablet as a second display. Runs in the system tray.
arg-settings = Open the settings window.
arg-quit = Quit the running ezdisplay, waiting until it has stopped.
arg-autostart = Turn starting with the session on or off, then exit.
arg-elevation = Turn running as administrator on or off, then exit.
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
usb-unavailable-hint = USB without developer mode is unavailable ({ $reason }); USB debugging still works.
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
stat-connection = Connection
transport-aoa = USB (accessory)
transport-wifi = Wi-Fi
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
unit-fps = fps
unit-mbps = Mbps
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
setting-hdr = HDR
hdr-auto = Automatic
hdr-off = Off
hdr-hint-linux = KDE doesn't send HDR to virtual displays yet; the stream stays SDR.
stat-hdr = HDR
mode-value-hdr = { $width }×{ $height } at { $hz } Hz, { $codec } HDR10
hdr-reason-turned-off = HDR is off in the settings; converting to SDR
hdr-reason-old-client = The tablet app is too old for HDR; update it
hdr-reason-display-not-hdr = The tablet's screen doesn't support HDR10
hdr-reason-no-main10-decoder = The tablet can't decode 10-bit HDR video
hdr-reason-no-main10-encoder = This graphics card can't encode 10-bit HEVC; converting to SDR
setting-audio = Audio
audio-description = Streams this computer's sound to the tablet. Automatic sends uncompressed audio when the link allows it.
audio-auto = Automatic
audio-pcm = PCM (uncompressed)
audio-opus = Opus (compressed)
audio-off = Off
stat-audio = Audio
audio-value-none = None
audio-value-pcm = PCM { $format }, { $khz } kHz, { $layout }
audio-value-opus = Opus, { $khz } kHz, { $layout }
audio-mono = mono
audio-stereo = stereo
audio-format-s16 = 16-bit
audio-format-f32 = 32-bit float

## Window: input
input-enabled = Send touch and pen input to this computer
setting-pen = Pen acts as
pen-tablet = Graphics tablet
pen-mouse = Mouse pointer
telemetry = Show the telemetry overlay on the tablet
input-permission-denied = Touch and pen need permission to create input devices (/dev/uinput).
input-permission-missing = Touch and pen need the uinput kernel module, which is not loaded (/dev/uinput is missing). Enabling input loads it and sets up the permission.
input-notice = Touch and pen are blocked: ezdisplay has no permission to create input devices.
input-enable = Enable input
input-enable-running = Waiting for authorization…
input-enable-done = Input is enabled.
input-enable-cancelled = Cancelled. Nothing was changed.
input-enable-no-agent = No authentication agent is available. Install the udev rule by hand, see the README.
input-enable-still-denied = The rule is installed, but access is still denied. Log out and back in, then try again.
input-enable-failed = Enabling input failed (code { $code }).
input-enable-error = Enabling input could not run: { $reason }

## Window: connection
adb-sdk = { $path } (Android SDK)
adb-bundled = { $path } (bundled with ezdisplay)
devices-count =
    { $count ->
        [one] { $count } device
       *[other] { $count } devices
    }
devices-none = Connect the tablet with a USB cable and turn on USB debugging.
samsung-driver = Samsung Android USB driver
device-line = { $name } ({ $serial }): { $state }
device-ready = ready
device-unauthorized = waiting for you to accept USB debugging
device-offline = offline
open-app = Open the app on the tablet when it is connected
use-adb = Connect over USB with adb
use-aoa = USB without developer mode
advanced = Advanced
setting-port = Port
port-hint = The tablet reaches this computer on this port. Changes apply when no session is running.

## Window: general
start-on-launch = Stream when ezdisplay opens
setting-run-elevated = Control administrator windows (run elevated)
run-elevated-hint = Lets pen and touch reach windows that run as administrator, such as Task Manager. Applies after restarting ezdisplay.
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

## Window: cards and row descriptions
devices-empty = No devices detected
stats-empty = Statistics appear when streaming starts
card-stream = Stream
card-measurements = Measurements
card-picture = Picture quality
card-encoding = Encoding
card-adb = Android Debug Bridge
card-startup = Startup
card-appearance = Appearance
card-diagnostics = Diagnostics
card-about = About
card-driver = Driver status
card-maintenance = Maintenance
scale-description = Lower values send fewer pixels to the tablet.
refresh-description = Limits the refresh rate offered to the tablet.
bitrate-description = Video quality of the stream. Automatic picks it for you.
encoder-description = The video encoder that compresses the stream.
hdr-description = Sends HDR10 when the tablet's screen supports it.
gpu-description = The graphics card that captures and encodes the screen.
input-description = Use the tablet as a touch screen and a pen on this computer.
pen-description = A graphics tablet adds pressure and tilt.
telemetry-description = Shows live performance numbers on the tablet while streaming.
use-adb-description = Finds tablets that have USB debugging turned on.
use-aoa-description = Switches the tablet to accessory mode when you plug it in, so USB debugging is not needed. adb stays as the fallback.
autostart-description = Starts ezdisplay when you sign in.
start-on-launch-description = Starts waiting for your tablet as soon as ezdisplay opens.
system-accent = Use the system accent color
system-accent-description = Matches the accent color picked in your system settings. When off, ezdisplay uses its own color.
logs-title = Logs
logs-description = Open the folder where ezdisplay writes its logs.
about-description = Open-source USB display for Android tablets.
repository = Source code
driver-reinstall = Reinstall the driver

## Window: Wi-Fi
card-wifi = Wi-Fi
wifi-enable = Connect over Wi-Fi
wifi-latency-warning = Wi-Fi adds latency and jitter; USB is best for drawing.
wifi-starts-with-stream = Wi-Fi turns on when streaming starts.
wifi-address = Address
wifi-fingerprint = Host fingerprint
wifi-pair = Pair a device
wifi-pair-cancel = Cancel
wifi-pair-hint = On the tablet, scan this QR code or type the PIN.
wifi-pin = PIN
wifi-qr-label = Pairing QR code
wifi-pair-expires = Closes in { $seconds } s
wifi-notice-paired = Paired with { $device }.
wifi-notice-attempts = Pairing closed: too many wrong PINs.
wifi-notice-expired = Pairing closed: time ran out.
wifi-paired-count =
    { $count ->
        [one] { $count } paired device
       *[other] { $count } paired devices
    }
wifi-paired-none = No devices are paired yet.
wifi-forget = Forget
error-wifi = Wi-Fi cannot start: { $reason }

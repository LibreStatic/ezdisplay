# Testing the Windows host

This guide covers testing the Windows host on a real machine and reporting back what is needed to fix
anything that fails. The host creates a virtual monitor with the Virtual Display Driver (VDD),
captures it with Desktop Duplication and encodes it on the GPU. The installer bundles the driver,
`adb` and the tray app, so nothing else has to be installed by hand.

Test it in both laptop configurations (the internal panel hangs off the iGPU or the dGPU depending on
the MUX mode) and, if possible, in a third pass that forces the GPU.

## Requirements

- Windows 10 22H2 (build 19045) or Windows 11, with up-to-date GPU drivers.
- An Android tablet with USB debugging enabled.
- Administrator rights for the installer.

## Steps

1. **Download the installer.** On GitHub, *Actions* tab, latest green *CI* run, artifact
   `ezdisplay-windows-installer`. It contains `ezdisplay-setup-<version>-x64.exe`. The artifact
   `ezdisplay-windows-x86_64` (portable executables, `ezd-fake-client.exe` and the FFmpeg DLLs) is
   only needed for the tests without a tablet below.

2. **Run the installer.** The installer is not signed yet:
   - SmartScreen shows "Windows protected your PC": click *More info*, then *Run anyway*.
   - Smart App Control (Windows 11) blocks unsigned programs and cannot be bypassed per program.
     If it is *On* (Windows Security, *App & browser control*, *Smart App Control settings*),
     neither the installer nor ezdisplay can run.

   Keep the "Install the virtual display driver" task checked. Setup downloads FFmpeg (about
   90 MB), then Windows asks once "Would you like to install this device software?" for the
   driver: click *Install*. At the end ezdisplay starts in the notification area.

   Do not install VDD with `winget`: that package only installs VDD's control app, not the driver.

3. **Full diagnostics.** In PowerShell:

   ```powershell
   $ezd = "$env:ProgramFiles\ezdisplay"
   & "$ezd\ezd-host.exe" diag > diag.json
   ```

   It changes nothing on the system. It includes the Windows version and DPI, the adapters (LUID,
   VRAM, performance/power order), the outputs of each adapter, the GPU decision, the VDD state, an
   encoder test per adapter (1920x1080, one frame) and a Desktop Duplication test on the virtual
   monitor.

4. **VDD state:**

   ```powershell
   & "$ezd\ezd-host.exe" vdd status > vdd-status.json
   ```

   After a clean install `owned` is `true` and `driver_dir` points to `$ezd\driver`. If the driver
   is missing, use *Repair* in the *Display driver* section of the ezdisplay settings window.

5. **Only with a VDD installed separately** (before ezdisplay; the installer leaves it untouched):
   prepare it once in an administrator console:

   ```powershell
   & "$ezd\ezd-host.exe" vdd setup
   ```

   It grants write access to `vdd_settings.xml` only, in the VDD configuration folder
   (`C:\VirtualDisplayDriver` or the `VDDPATH` registry value), so later runs do not need
   administrator rights. Useful options: `--gpu "NAME"` to assign the virtual monitor to a GPU, and
   `--mode 2560x1600@120` (repeatable) to offer modes in advance. `vdd revert` undoes the
   permission. With the bundled driver this is already done.

6. **Install the app on the tablet** (once per version). From the *Android* workflow in *Actions*,
   artifact `ezdisplay-android-apks`, with the `adb` that ships with ezdisplay:

   ```powershell
   & "$ezd\adb\adb.exe" install -r app-full-debug.apk
   ```

   If `adb devices` does not list the tablet, install the
   [Samsung Android USB driver](https://developer.samsung.com/android-usb-driver) (other brands
   publish their own), reconnect the cable and accept the USB debugging prompt on the tablet.

7. **Stream.** Connect the tablet over USB. The tray app opens the ezdisplay app on the tablet and
   starts streaming; the tray icon and its menu show the state, and *Settings* shows live statistics.
   For more detailed logs, or to pick options by hand, quit the tray app from its menu and run the
   host in a console:

   ```powershell
   & "$ezd\ezd-host.exe" run --source virtual --adb
   ```

   To pick the GPU: `--adapter "NVIDIA"` (name or LUID `0x...`) or `--prefer-dgpu`. To capture an
   existing monitor: `--output \\.\DISPLAY3`. With `$env:RUST_LOG="debug"` the log is more
   detailed. The tray app writes its logs to `%LOCALAPPDATA%\ezdisplay\logs`.

## Installer checks

- **Reinstall** over the existing install: it must not download FFmpeg again.
- **Autostart:** sign out and back in; ezdisplay must start in the notification area (unless the
  "Start ezdisplay when you sign in" task was unchecked).
- **Uninstall** from *Settings*, *Apps*: afterwards Device Manager (*Display adapters*, with
  *View*, *Show hidden devices*) must not list "Virtual Display Driver", and
  `C:\Program Files\ezdisplay` must be gone.

## What to test in each MUX mode

Repeat steps 3, 4 and 7 once per mode:

- **Hybrid / Optimus mode (the internal panel uses the iGPU):** the policy picks the iGPU (the one
  driving the main display). Expected: encoder `h264_qsv` (Intel) and `adapter_mismatch: false` in
  `vdd_output`.
- **dGPU / discrete MUX mode (the internal panel uses the dGPU):** the policy picks the dGPU.
  Expected: `h264_nvenc` (NVIDIA) or `h264_amf` (AMD).
- **Forcing the dGPU in hybrid mode:** `--prefer-dgpu`. Expected: the host assigns the VDD to the
  dGPU (SETGPU) and video keeps flowing.

In each case check that the virtual monitor appears to the right of the main one, that the image
reaches the tablet in the requested mode, that the pointer moves without lag, and that video recovers
on its own after changing the resolution or locking/unlocking the session.

## Input (pen and touch)

The host injects the tablet's pen and touch with Windows synthetic pointer devices (Windows 10 1809
or later), and clicks, scrolling and keys with `SendInput`.

1. **Diagnostics.** In `diag.json`, the `input` section must say `"pen": "ok"` and `"touch": "ok"`.
   If not, copy the error (HRESULT) as is.
2. **Without a tablet.** From the unzipped `ezdisplay-windows-x86_64` artifact, with
   `.\ezd-host.exe run --source virtual` in one console (quit the tray app first), run
   `.\ezd-fake-client.exe` in another with `--input-script drag`, `touch2` and `scroll` (one at a
   time; see `.\ezd-fake-client.exe --help` for the other options) and watch the cursor and the ink in
   Paint or Whiteboard on the virtual monitor. `drag` draws a stroke, `touch2` does two fingers,
   `scroll` scrolls and zooms with Ctrl, `keys` holds Ctrl+Space while drawing a stroke.
3. **Pressure.** With the tablet and a pen: in Paint or Whiteboard the stroke width must follow the
   pressure; the eraser (if the pen has one) must erase.
4. **Touch.** In Edge: dragging with one finger scrolls the page, pinching with two fingers zooms.
   A still finger must not lift on its own (the host refreshes it every 100 ms; if it lifts, note it).
5. **Pen button.** Side button held + tap = right click (context menu).
6. **Keyboard shortcuts ("Graphics tablet" profile).** In the tablet app pick the "Graphics tablet"
   profile. In Krita (Settings > Configure Krita > Tablet settings) choose Windows Ink so pressure
   works. Holding the pen button and dragging with the tip must zoom (default shortcut: Ctrl+Space).
   Without a tablet, `.\ezd-fake-client.exe --input-script keys` plays the same thing. On the test
   page (`tools/input-test`) the keys show up as `keydown` / `keyup`, with Ctrl held during the
   stroke. Note whether any key stays "stuck" after releasing the button or ending the session.
7. **Elevated windows.** With Task Manager open (elevated), Windows blocks input to it unless the
   host also runs as administrator. Note whether the `ezd-host run` log shows the UIPI warning and
   whether pen/touch have no effect there.
8. Note the `note` field of the input state shown on the tablet and any `input:` line in the log
   (with `RUST_LOG=debug`).

## What to send back

- `diag.json` and `vdd-status.json` from each MUX mode (say which one).
- The console output (stderr) of `ezd-host run ...` from start until the problem; if something fails,
  repeat with `RUST_LOG=debug`.
- A description of what you saw: was the monitor created? did video arrive? roughly how much latency?
  did the pointer behave? was there flicker or a black screen?
- Laptop model, GPU(s) and driver version (they appear in `diag.json`).

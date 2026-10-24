# Phase 0 — checklist on the Acer Nitro 5 (Windows)

Goal: confirm with data why the current second-screen app stutters (which GPU its display ends up on
and whether there are cross-GPU copies) and record a latency baseline to compare ezdisplay against.
Then test whether the Virtual Display Driver (VDD) stays on the GPU we choose.

Run parts A and B **in both MUX modes** (Discrete and Optimus/Hybrid; the switch is made in
NitroSense and requires a reboot). Fill in the table at the end and send it back together with the
screenshots.

## 0. Baseline data (once)

- Windows: `winver` → version and build.
- Exact Nitro model (label underneath or `wmic csproduct get name`).
- Device Manager → Display adapters: **exact** name of both GPUs and each driver version.
- Main monitor: the laptop panel or an external one? If external, over HDMI, USB-C or Mini DP?
- Tablet: exact model, Android version and native resolution (Settings → About tablet, or
  `adb shell wm size`).
- Current second-screen app: version and settings (resolution, fps, quality, USB/Wi-Fi). Cable used.

## A. Current second-screen app (baseline)

1. **Which GPU is the current app's display connected to?** Settings → System → Display → Advanced
   display → pick the current app's display → note "Connected to: …". Repeat for the main monitor.
2. **Engines of each GPU.** Task Manager → Performance → GPU 0 and GPU 1. On each one switch two of
   the small graphs (click the graph title) to **Copy** and **Video Encode**. Note the approximate %
   of each engine on each GPU in these cases:
   - a. Idle, with the tablet showing the desktop.
   - b. Moving the cursor back and forth between the main monitor and the tablet.
   - c. Dragging a window (for example, the browser) between the two displays.
   - d. A YouTube video playing on the tablet.

   Take a screenshot of Task Manager in case c.
3. **Which GPU the process uses.** Task Manager → Details → right-click the headers → Select columns →
   "GPU engine". Note what it shows for the current app's processes.
4. **Latency.** Open the repo's `tools/latency/index.html` in **two** browser windows: one maximized
   on the main monitor and one maximized on the tablet. In the tablet window, the line at the top left
   shows the refresh rate it really reaches (note it).
   - With a phone in slow motion (240 fps if possible), film both displays together for about
     10 seconds.
   - Then film the cursor crossing from one display to the other, and a window being dragged between
     the two.
   - Send the videos, or measure yourself: pause on a frame and subtract the two clocks
     (tablet − main). Take about 10 readings on different frames.

## B. Virtual Display Driver (proof of concept)

> It installs a signed display driver (MIT). Uninstall it from "Add or remove programs" or with
> `winget uninstall`.

1. In an administrator terminal: `winget install --id=VirtualDrivers.Virtual-Display-Driver -e`
2. If the key `HKLM\SOFTWARE\MikeTheTech\VirtualDisplayDriver` exists in regedit, note its values:
   the driver reads it **before** the XML.
3. Edit `C:\VirtualDisplayDriver\vdd_settings.xml`:
   - In `<gpu><friendlyname>` put the exact name of the GPU under test, copied from Device Manager.
     First the NVIDIA one, then the Intel/AMD one.
   - In `<resolutions>` add the tablet's native resolution. The refresh rates in `<global>` (60–240)
     are added to every resolution.

     ```xml
     <resolution>
         <width>2560</width>
         <height>1600</height>
         <refresh_rate>120</refresh_rate>
     </resolution>
     ```
4. Restart the driver: Device Manager → Display adapters → Virtual Display Driver → Disable and
   Enable.
5. Settings → Display: set the virtual monitor to **Extend**, with the tablet's resolution and
   **120 Hz**. In Advanced display, note which GPU it says "Connected to".
6. Repeat step A.2 case c, dragging a window to the virtual monitor. It is not visible, but windows
   can be moved with `Win+Shift+←/→`. Note the Copy % of each GPU.
7. Change `friendlyname` to the other GPU, restart the driver (step 4) and repeat 5–6.

## Table to fill in

| MUX mode | Test | Display connected to | Copy GPU0 / GPU1 (%) | Video Encode GPU0 / GPU1 (%) | Latency tablet − main (ms) | Real refresh on the tablet |
|---|---|---|---|---|---|---|
| Discrete | Current app idle | | | | | |
| Discrete | Current app cursor crossing | | | | | |
| Discrete | Current app dragging window | | | | | |
| Discrete | Current app video | | | | | |
| Discrete | VDD → NVIDIA, dragging | | | | — | — |
| Discrete | VDD → iGPU, dragging | | | | — | — |
| Hybrid | Current app idle | | | | | |
| Hybrid | Current app cursor crossing | | | | | |
| Hybrid | Current app dragging window | | | | | |
| Hybrid | Current app video | | | | | |
| Hybrid | VDD → NVIDIA, dragging | | | | — | — |
| Hybrid | VDD → iGPU, dragging | | | | — | — |

Free-form notes: anything that feels different, driver errors, whether some resolution or refresh rate
is missing, etc.

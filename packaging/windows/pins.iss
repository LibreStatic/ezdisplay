; Pinned third-party downloads of the Windows installer.
;
; Single source of truth: ezdisplay.iss includes this file, and CI reads the same defines with a
; regex (one `#define Name "value"` per line) to fetch and verify the files it bundles. Bump a pin
; by changing its URL and SHA-256 together.

; FFmpeg (BtbN monthly build, GPL, shared). Not bundled: the installer downloads it and keeps only
; bin\*.dll. CI links against the headers and import libraries of the same build.
#define FfmpegUrl "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-09-30-13-08/ffmpeg-n8.1.3-9-g29e619e767-win64-gpl-shared-8.1.zip"
#define FfmpegSha256 "dfe81c3aa0a546ee81980b1b824486dd0fe0a1d452f00cb55135379a74be7bcf"
#define FfmpegDir "ffmpeg-n8.1.3-9-g29e619e767-win64-gpl-shared-8.1"

; Virtual Display Driver 25.7.23 (MIT). The archive's VirtualDisplayDriver\ folder holds the signed
; driver package, bundled unchanged.
#define VddUrl "https://github.com/VirtualDrivers/Virtual-Display-Driver/releases/download/25.7.23/VirtualDisplayDriver-x86.Driver.Only.zip"
#define VddSha256 "e24210692b442b39af763536330ce78b423f19342b7a7792c26de3944e418b3a"

; nefcon 1.21.0 (MIT): creates the driver's root device node. Bundled: x64\nefconw.exe.
#define NefconUrl "https://github.com/nefarius/nefcon/releases/download/v1.21.0/nefcon_v1.21.0.zip"
#define NefconSha256 "4c8e68f6b663598d77b7c3f76459b16fad00994e53ea8af0bd106a805dd5c18b"

; Android SDK Platform-Tools r37.0.1. Bundled: adb.exe, AdbWinApi.dll, AdbWinUsbApi.dll, NOTICE.txt.
#define PlatformToolsUrl "https://dl.google.com/android/repository/platform-tools_r37.0.1-win.zip"
#define PlatformToolsSha256 "45f4d63113e895ebde0c90f194099a4676b6ac653bd28d54314a9e022bbc1a99"

; UsbDk 1.0.22 (Apache-2.0, https://github.com/daynix/UsbDk): the filter driver that lets libusb open
; devices Windows keeps for itself (MTP), needed for USB without developer mode (AOA). Optional
; installer task: the installer downloads the MSI and runs it only when the task is selected.
#define UsbDkUrl "https://github.com/daynix/UsbDk/releases/download/v1.00-22/UsbDk_1.0.22_x64.msi"
#define UsbDkSha256 "91f6f695e1e13c656024e6d3b55620bf08d8835ef05ee0496935ba6bb62466a5"

# Windows installer

`ezdisplay.iss` is an [Inno Setup](https://jrsoftware.org/isinfo.php) 6.7 script. The installer:

- installs `ezdisplay.exe` (tray app) and `ezd-host.exe` to `Program Files\ezdisplay`;
- bundles the signed Virtual Display Driver package, `nefconw.exe` and `adb` (Platform-Tools);
- downloads FFmpeg (GPL shared build) during setup, verifies its SHA-256 and keeps only the DLLs;
- optionally installs the virtual display driver (`ezd-host.exe vdd install`) and registers
  autostart for the signing-in user (`ezdisplay.exe --autostart on`);
- on uninstall stops the tray, removes the autostart entry, stops adb and removes the driver (only
  if ezdisplay installed it) before deleting any file.

Third-party downloads are pinned in `pins.iss` (URL + SHA-256), which both the installer and CI read.

## Build locally (Windows)

```powershell
cargo build --workspace --release
pwsh packaging\windows\stage.ps1 -OutDir staging -CacheDir $env:TEMP\ezdisplay-pins
& "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" /DAppVersion=0.1.0 /DStagingDir=$PWD\staging packaging\windows\ezdisplay.iss
```

The installer lands in `packaging\windows\output\ezdisplay-setup-<version>-x64.exe`.
`stage.ps1 -FfmpegOnly` only fetches the pinned FFmpeg build (headers and import libraries) and sets
`FFMPEG_DIR` for the Rust build in CI.

## Install options

- `/FFMPEGZIP=<path>`: use a local copy of the pinned FFmpeg archive instead of downloading it
  (offline installs). Its SHA-256 is verified.
- Silent install: `ezdisplay-setup-<version>-x64.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /MERGETASKS="!driver"`.
  A silent install cannot install the driver unattended: Windows shows its own "Would you like to
  install this device software?" prompt, and ezdisplay does not import any certificate into
  TrustedPublisher. Skip the `driver` task and repair the driver later from ezdisplay's settings
  ("Display driver", "Repair").
- Reinstalling or upgrading skips the FFmpeg download when `ffmpeg.sha256` in the install folder
  matches the pin.

A driver uninstall that needs a reboot (exit code 3010) is not reported by the uninstaller.

## Signing

The installer is not signed yet. SmartScreen shows "Run anyway" and Smart App Control blocks unsigned
installers. A commented-out `SignTool=` line in `ezdisplay.iss` is the place to enable signing once a
SignPath Foundation certificate exists.

## Bump a pin

Change the URL and the SHA-256 of an entry in `pins.iss` together (for FFmpeg also `FfmpegDir`, the
top-level folder of the archive). Compute the hash with `Get-FileHash <file> -Algorithm SHA256`
(lowercase it) and update the matching license file in `licenses\` if the license text changed. CI
verifies every download against the pin and runs the installer end to end.

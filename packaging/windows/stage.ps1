# Downloads and verifies the pinned third-party archives (pins.iss) and lays out the staging folder
# consumed by ezdisplay.iss (/DStagingDir=...). PowerShell 7.
#
#   stage.ps1 -OutDir staging            # full staging (needs target\release\{ezdisplay,ezd-host}.exe)
#   stage.ps1 -FfmpegOnly                # only fetch FFmpeg (headers + import libs) for the build
param(
    [string]$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path,
    [string]$OutDir = (Join-Path $PSScriptRoot 'staging'),
    [string]$CacheDir = (Join-Path ([IO.Path]::GetTempPath()) 'ezdisplay-pins'),
    # Only download, verify and extract FFmpeg. Exports FFMPEG_DIR and its bin folder to the
    # GitHub Actions environment when running there.
    [switch]$FfmpegOnly
)

$ErrorActionPreference = 'Stop'
$pinsFile = Join-Path $PSScriptRoot 'pins.iss'

# One `#define Name "value"` per line.
$pins = @{}
foreach ($line in Get-Content $pinsFile) {
    if ($line -match '^#define\s+(\w+)\s+"([^"]*)"') { $pins[$Matches[1]] = $Matches[2] }
}
foreach ($key in 'FfmpegUrl', 'FfmpegSha256', 'FfmpegDir', 'VddUrl', 'VddSha256', 'NefconUrl',
    'NefconSha256', 'PlatformToolsUrl', 'PlatformToolsSha256') {
    if (-not $pins.ContainsKey($key)) { throw "pins.iss does not define $key" }
}

New-Item -ItemType Directory -Force $CacheDir | Out-Null

# Downloads $Url into the cache (skipped when a file with the right hash is already there),
# verifies SHA-256 and returns the file path.
function Get-Pinned([string]$Url, [string]$Sha256) {
    $file = Join-Path $CacheDir ($Sha256.Substring(0, 8) + '-' + ($Url -split '/')[-1])
    if (Test-Path $file) {
        if ((Get-FileHash $file -Algorithm SHA256).Hash -eq $Sha256) { return $file }
        Remove-Item $file -Force
    }
    Write-Host "Downloading $Url"
    curl.exe -L --fail --retry 3 -o $file $Url
    if ($LASTEXITCODE -ne 0) { throw "download failed ($LASTEXITCODE): $Url" }
    $actual = (Get-FileHash $file -Algorithm SHA256).Hash
    if ($actual -ne $Sha256) {
        Remove-Item $file -Force
        throw "SHA-256 mismatch for ${Url}: expected $Sha256, got $actual"
    }
    return $file
}

# Extracts an archive into a fresh folder under the cache and returns that folder.
function Expand-Pinned([string]$Zip, [string]$Name) {
    $dest = Join-Path $CacheDir "x-$Name"
    if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
    Expand-Archive -Path $Zip -DestinationPath $dest
    return $dest
}

function Copy-Required([string]$Source, [string]$Destination) {
    if (-not (Test-Path $Source)) { throw "missing file: $Source" }
    New-Item -ItemType Directory -Force (Split-Path $Destination) | Out-Null
    Copy-Item $Source $Destination -Force
}

if ($FfmpegOnly) {
    $zip = Get-Pinned $pins.FfmpegUrl $pins.FfmpegSha256
    $root = Expand-Pinned $zip 'ffmpeg'
    $dir = Join-Path $root $pins.FfmpegDir
    if (-not (Test-Path (Join-Path $dir 'include\libavcodec\avcodec.h'))) {
        throw 'unexpected FFmpeg archive layout'
    }
    Write-Host "FFMPEG_DIR=$dir"
    if ($env:GITHUB_ENV) { "FFMPEG_DIR=$dir" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8 }
    if ($env:GITHUB_PATH) { Join-Path $dir 'bin' | Out-File -FilePath $env:GITHUB_PATH -Append -Encoding utf8 }
    return
}

# Full staging.
$release = Join-Path $RepoRoot 'target\release'
if (Test-Path $OutDir) { Remove-Item $OutDir -Recurse -Force }
New-Item -ItemType Directory -Force $OutDir | Out-Null

Copy-Required (Join-Path $release 'ezdisplay.exe') (Join-Path $OutDir 'ezdisplay.exe')
Copy-Required (Join-Path $release 'ezd-host.exe') (Join-Path $OutDir 'ezd-host.exe')

# Virtual display driver: the signed package, unchanged.
$vdd = Expand-Pinned (Get-Pinned $pins.VddUrl $pins.VddSha256) 'vdd'
foreach ($name in 'mttvdd.cat', 'MttVDD.dll', 'MttVDD.inf', 'vdd_settings.xml') {
    Copy-Required (Join-Path $vdd "VirtualDisplayDriver\$name") (Join-Path $OutDir "driver\$name")
}

# nefcon creates the driver's root device node.
$nefcon = Expand-Pinned (Get-Pinned $pins.NefconUrl $pins.NefconSha256) 'nefcon'
Copy-Required (Join-Path $nefcon 'x64\nefconw.exe') (Join-Path $OutDir 'driver\nefconw.exe')

# adb.
$pt = Expand-Pinned (Get-Pinned $pins.PlatformToolsUrl $pins.PlatformToolsSha256) 'platform-tools'
foreach ($name in 'adb.exe', 'AdbWinApi.dll', 'AdbWinUsbApi.dll', 'NOTICE.txt') {
    Copy-Required (Join-Path $pt "platform-tools\$name") (Join-Path $OutDir "adb\$name")
}

# Licenses (FFmpeg itself is downloaded at install time).
$lic = Join-Path $OutDir 'licenses'
$packaging = $PSScriptRoot
Copy-Required (Join-Path $RepoRoot 'LICENSE-MIT') (Join-Path $lic 'LICENSE-MIT')
Copy-Required (Join-Path $RepoRoot 'LICENSE-APACHE') (Join-Path $lic 'LICENSE-APACHE')
Copy-Required (Join-Path $packaging 'licenses\VirtualDisplayDriver-LICENSE.txt') (Join-Path $lic 'VirtualDisplayDriver-LICENSE.txt')
Copy-Required (Join-Path $packaging 'licenses\nefcon-LICENSE.txt') (Join-Path $lic 'nefcon-LICENSE.txt')
Copy-Required (Join-Path $packaging 'licenses\FFmpeg-NOTICE.txt') (Join-Path $lic 'FFmpeg-NOTICE.txt')
Copy-Required (Join-Path $pt 'platform-tools\NOTICE.txt') (Join-Path $lic 'adb-NOTICE.txt')

Write-Host "Staging folder ready: $OutDir"

# Fetch the pinned FFmpeg build that ships inside Pirosman Studio (Windows x64).
#
# Pinned: FFmpeg 9.0.2 "essentials" static build by gyan.dev (GPL v3), from the
# versioned GitHub release of GyanD/codexffmpeg. The SHA-256 below equals the
# digest GitHub publishes for that asset. The archive is cached in .cache\ffmpeg
# and only ffmpeg.exe, ffprobe.exe and the license/readme are extracted into
# src-tauri\ffmpeg\, which Tauri bundles into the installer (see
# src-tauri\tauri.windows.conf.json) and package-windows.ps1 copies into the
# portable zip. The build fails if the checksum, version or required features
# do not match. Nothing is downloaded at runtime.
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts/fetch-ffmpeg.ps1 [-Force]
# -TestExpectedSha256 exists only to prove that a checksum mismatch fails the build.
param([switch]$Force, [string]$TestExpectedSha256)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$Version   = "9.0.2"
$Archive   = "ffmpeg-9.0.2-essentials_build.zip"
$Url       = "https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/$Archive"
$Sha256    = "60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba"
$VersionId = "9.0.2-essentials_build-www.gyan.dev"   # first line of `ffmpeg -version`
if ($TestExpectedSha256) { $Sha256 = $TestExpectedSha256.ToLower() }
$Prefix    = "ffmpeg-9.0.2-essentials_build/"

# Everything Pirosman Studio asks FFmpeg for: one list shared with fetch-ffmpeg.sh, kept
# complete by a render test (render_filters_are_all_required_of_the_pinned_ffmpeg).
$req = Get-Content (Join-Path $PSScriptRoot "ffmpeg-requirements.txt")
$Encoders = @($req | Where-Object { $_ -match '^encoder ' } | ForEach-Object { $_.Split(" ")[1] })
$Filters  = @($req | Where-Object { $_ -match '^filter ' } | ForEach-Object { $_.Split(" ")[1] })

$root  = Split-Path $PSScriptRoot -Parent
$cache = Join-Path $root ".cache\ffmpeg"
$out   = Join-Path $root "src-tauri\ffmpeg"
$zip   = Join-Path $cache $Archive
$manifestPath = Join-Path $out "manifest.json"
$notice = Join-Path $root "packaging\FFMPEG_NOTICE.txt"

function Fail($msg) { Write-Host "fetch-ffmpeg: ERROR: $msg" -ForegroundColor Red; exit 1 }
function Sha($p) { (Get-FileHash $p -Algorithm SHA256).Hash.ToLower() }

# The shipped notices must describe the pinned build (update them when bumping FFmpeg).
if (-not $TestExpectedSha256) {
    foreach ($doc in $notice, (Join-Path $root "THIRD_PARTY_NOTICES.md")) {
        $text = Get-Content $doc -Raw
        if (-not $text.Contains($Url) -or -not $text.Contains($Sha256)) { Fail "$doc does not name the pinned archive URL and SHA-256" }
    }
}

# Up to date? (same pinned archive, both executables present with recorded sizes)
if (-not $Force -and (Test-Path $manifestPath)) {
    try {
        $m = Get-Content $manifestPath -Raw | ConvertFrom-Json
        $ok = $m.archive_sha256 -eq $Sha256
        foreach ($f in $m.files.PSObject.Properties) {
            $p = Join-Path $out $f.Name
            if (-not (Test-Path $p) -or (Get-Item $p).Length -ne $f.Value.size) { $ok = $false }
        }
        if ($ok) {
            # Refresh the notice so an edited packaging\FFMPEG_NOTICE.txt always ships.
            Copy-Item $notice (Join-Path $out "NOTICE.txt") -Force
            Write-Host "fetch-ffmpeg: FFmpeg $VersionId already in src-tauri\ffmpeg"; exit 0
        }
    } catch { }
}

New-Item -ItemType Directory -Force $cache | Out-Null
if (-not (Test-Path $zip) -or (Sha $zip) -ne $Sha256) {
    Write-Host "fetch-ffmpeg: downloading $Url"
    $tmp = "$zip.partial"
    try { Invoke-WebRequest $Url -OutFile $tmp -UseBasicParsing } catch { Fail "download failed: $($_.Exception.Message)" }
    Move-Item $tmp $zip -Force
}
$got = Sha $zip
if ($got -ne $Sha256) {
    Remove-Item $zip -Force
    Fail "SHA-256 mismatch for $Archive. Expected $Sha256, got $got. The download was deleted; do not bundle it."
}
Write-Host "fetch-ffmpeg: SHA-256 verified ($Sha256)"

# Extract only what ships.
$stage = Join-Path $cache "stage"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory $stage | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem
$z = [IO.Compression.ZipFile]::OpenRead($zip)
try {
    $want = @{ "bin/ffmpeg.exe" = "ffmpeg.exe"; "bin/ffprobe.exe" = "ffprobe.exe"; "LICENSE" = "LICENSE.txt"; "README.txt" = "BUILD-README.txt" }
    foreach ($k in $want.Keys) {
        $e = $z.GetEntry($Prefix + $k)
        if (-not $e) { Fail "archive lacks $Prefix$k" }
        [IO.Compression.ZipFileExtensions]::ExtractToFile($e, (Join-Path $stage $want[$k]), $true)
    }
} finally { $z.Dispose() }

# Verify the binaries actually are the pinned build and can do what the app needs.
$ff = Join-Path $stage "ffmpeg.exe"
$fp = Join-Path $stage "ffprobe.exe"
$v1 = (& $ff -hide_banner -version | Select-Object -First 1)
$v2 = (& $fp -hide_banner -version | Select-Object -First 1)
if ($v1 -notlike "*$VersionId*") { Fail "ffmpeg.exe reports '$v1', expected $VersionId" }
if ($v2 -notlike "*$VersionId*") { Fail "ffprobe.exe reports '$v2', expected $VersionId" }
$enc = (& $ff -hide_banner -encoders) -join "`n"
$fil = (& $ff -hide_banner -filters) -join "`n"
$missing = @()
foreach ($e in $Encoders) { if ($enc -notmatch "\s$([regex]::Escape($e))\s") { $missing += "encoder $e" } }
foreach ($f in $Filters)  { if ($fil -notmatch "\s$([regex]::Escape($f))\s") { $missing += "filter $f" } }
if ($missing.Count) { Fail "pinned FFmpeg lacks: $($missing -join ', ')" }
Write-Host "fetch-ffmpeg: $($Encoders.Count) encoders and $($Filters.Count) filters present"

# Publish into src-tauri\ffmpeg (replacing any previous version entirely).
if (Test-Path $out) { Remove-Item $out -Recurse -Force }
New-Item -ItemType Directory $out | Out-Null
Copy-Item (Join-Path $stage "*") $out
Copy-Item $notice (Join-Path $out "NOTICE.txt")
$files = [ordered]@{}
foreach ($n in "ffmpeg.exe", "ffprobe.exe") {
    $p = Join-Path $out $n
    $files[$n] = [ordered]@{ size = (Get-Item $p).Length; sha256 = (Sha $p) }
}
$manifest = [ordered]@{
    version        = $VersionId
    source_url     = $Url
    archive_sha256 = $Sha256
    license        = "GPL-3.0-or-later (see LICENSE.txt and NOTICE.txt)"
    files          = $files
}
# UTF-8 without BOM, read by crates/pirosman-core/src/media.rs.
[IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 4))
Remove-Item $stage -Recurse -Force
Write-Host "fetch-ffmpeg: bundled FFmpeg $VersionId ready in src-tauri\ffmpeg"

# Build the Windows release and package it two ways into dist-release\:
#   1. Pirosman-Studio_<ver>_x64-setup.exe    NSIS installer: app, bundled FFmpeg
#      (ffmpeg\), installs WebView2 if missing
#   2. Pirosman-Studio_<ver>_x64-portable.zip exe + ffmpeg\ + ofx\
#      + README.txt (packaging\PORTABLE_README.txt) + license and notice files
# The WebView2 loader is linked statically on MSVC (webview2-com-sys uses
# WebView2LoaderStatic.lib). Only a windows-gnu build links WebView2Loader.dll
# dynamically; then tauri-build copies it to target\release and both packages
# must carry it next to the exe.
# Both contain the same pinned FFmpeg fetched and checksum-verified by
# scripts/fetch-ffmpeg.ps1 (run automatically by `tauri build` on Windows).
#
# Usage (from the repo root):  powershell -ExecutionPolicy Bypass -File scripts/package-windows.ps1
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
function Fail($msg) { Write-Host "package-windows: ERROR: $msg" -ForegroundColor Red; exit 1 }
. (Join-Path $PSScriptRoot "packaging-helpers.ps1")

$ver = (Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json).version

# Keep the build machine's paths out of the shipped binaries: rustc records
# source paths (e.g. panic locations) of generated files under the repo's
# target\ and of every dependency under the Cargo registry. The flags reach
# `tauri build` and its beforeBuildCommand (build-ofx.ps1), which inherit this
# environment. Existing RUSTFLAGS / CARGO_ENCODED_RUSTFLAGS are kept.
$cargoHome = [IO.Path]::GetFullPath($(if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }))
$sep = [string][char]0x1f
$flags = @(if ($env:CARGO_ENCODED_RUSTFLAGS) { $env:CARGO_ENCODED_RUSTFLAGS -split $sep } elseif ($env:RUSTFLAGS) { $env:RUSTFLAGS -split '\s+' | Where-Object { $_ } })
$env:CARGO_ENCODED_RUSTFLAGS = ($flags + "--remap-path-prefix=$root=/pirosman" + "--remap-path-prefix=$cargoHome=/cargo") -join $sep

npx tauri build
if ($LASTEXITCODE -ne 0) { Fail "tauri build failed" }
# `tauri build` targets the Rust host toolchain.
$gnu = [bool]((rustc -vV) -match '^host: .*-windows-gnu$')

$rel = Join-Path $root "target\release"
$ff  = Join-Path $root "src-tauri\ffmpeg"
$out = Join-Path $root "dist-release"
$need = "ffmpeg.exe", "ffprobe.exe", "manifest.json", "LICENSE.txt", "NOTICE.txt", "BUILD-README.txt"
foreach ($n in $need) { if (-not (Test-Path (Join-Path $ff $n))) { Fail "src-tauri\ffmpeg\$n missing; run npm run fetch-ffmpeg" } }

# The executables we compile and ship must not contain local paths.
$shipped = (Join-Path $rel "pirosman-studio.exe"), (Join-Path $root "src-tauri\ofx\pirosman-ofxhost.exe"), (Join-Path $root "src-tauri\ofx\Plugins\Pirosman Samples.ofx.bundle\Contents\Win64\Pirosman Samples.ofx")
$leaks = Find-PathLeaks -Path $shipped -Forbidden $root, $cargoHome, $env:USERPROFILE
if ($leaks) { $leaks | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }; Fail "shipped binaries contain local paths; check the --remap-path-prefix flags" }
Write-Host "package-windows: no local paths in $($shipped.Count) shipped binaries"

# The installer must actually carry the FFmpeg files (inspect the generated NSIS script).
$nsi = Get-Content (Join-Path $rel "nsis\x64\installer.nsi") -Raw
foreach ($n in $need) { if ($nsi -notmatch [regex]::Escape("ffmpeg\$n")) { Fail "installer script does not include ffmpeg\$n" } }
if ($gnu -and $nsi -notmatch "WebView2Loader\.dll") { Fail "installer script does not include WebView2Loader.dll" }
if ($nsi -notmatch "pirosman-ofxhost\.exe") { Fail "installer script does not include the OpenFX host" }
$docs = "THIRD_PARTY_NOTICES.md", "LICENSE-MIT", "LICENSE-APACHE"
foreach ($n in $docs) { if ($nsi -notmatch [regex]::Escape($n)) { Fail "installer script does not include $n" } }

New-Item -ItemType Directory -Force $out | Out-Null
Get-ChildItem $out -Filter "Pirosman-Studio_*" | Remove-Item -Force
$setup = Get-ChildItem (Join-Path $rel "bundle\nsis") -Filter "*_${ver}_x64-setup.exe" -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1
if (-not $setup) { Fail "no NSIS installer for version $ver (*_${ver}_x64-setup.exe) in target\release\bundle\nsis; check the tauri build output above" }
Copy-Item $setup.FullName (Join-Path $out "Pirosman-Studio_${ver}_x64-setup.exe") -Force

$stage = Join-Path $out "Pirosman Studio"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory $stage | Out-Null
Copy-Item (Join-Path $rel "pirosman-studio.exe") (Join-Path $stage "Pirosman Studio.exe")
# The windows-gnu build links WebView2Loader.dll dynamically: it must sit next to the exe.
if ($gnu) { Copy-Item (Join-Path $rel "WebView2Loader.dll") $stage }
# Same layout as the installer: <app folder>\ffmpeg\ is where the app looks first.
New-Item -ItemType Directory (Join-Path $stage "ffmpeg") | Out-Null
foreach ($n in $need) { Copy-Item (Join-Path $ff $n) (Join-Path $stage "ffmpeg\$n") }
# OpenFX: the out-of-process host and the bundled sample plugins (<app folder>\ofx\).
Copy-Item (Join-Path $root "src-tauri\ofx") (Join-Path $stage "ofx") -Recurse
Copy-Item (Join-Path $root "packaging\PORTABLE_README.txt") (Join-Path $stage "README.txt")
foreach ($n in $docs) { Copy-Item (Join-Path $root $n) (Join-Path $stage $n) }

$zip = Join-Path $out "Pirosman-Studio_${ver}_x64-portable.zip"
try { New-PortableZip -Stage $stage -Zip $zip } catch { Fail "portable zip: $($_.Exception.Message)" }
Remove-Item $stage -Recurse -Force

# Verify the zip carries everything the app needs.
$names = (Get-ZipEntries $zip).Name
$expect = @("Pirosman Studio/Pirosman Studio.exe", "Pirosman Studio/ofx/pirosman-ofxhost.exe", "Pirosman Studio/ofx/Plugins/Pirosman Samples.ofx.bundle/Contents/Win64/Pirosman Samples.ofx", "Pirosman Studio/README.txt") + ($docs | ForEach-Object { "Pirosman Studio/$_" }) + ($need | ForEach-Object { "Pirosman Studio/ffmpeg/$_" })
if ($gnu) { $expect += "Pirosman Studio/WebView2Loader.dll" }
foreach ($e in $expect) { if ($names -notcontains $e) { Fail "portable zip lacks $e" } }

Write-Host "package-windows: installer and portable zip verified (app, ffmpeg\: $($need -join ', ')$(if ($gnu) { ', WebView2Loader.dll' }))"
Get-ChildItem $out -File | Select-Object Name, @{ n = "MB"; e = { [math]::Round($_.Length / 1MB, 1) } }

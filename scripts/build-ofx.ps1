# Build the OpenFX host and the bundled "Pirosman Samples" plugins into src-tauri\ofx\,
# which the installer and the portable zip ship as <app>\ofx\:
#   ofx\pirosman-ofxhost.exe                                   out-of-process plugin host
#   ofx\Plugins\Pirosman Samples.ofx.bundle\Contents\Win64\... sample OpenFX effects
# Run automatically before `tauri build` / `tauri dev` on Windows.
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
cargo build --release -p pirosman-ofxhost -p pirosman-ofx-samples
if ($LASTEXITCODE -ne 0) { Write-Host "build-ofx: cargo build failed" -ForegroundColor Red; exit 1 }
$out = Join-Path $root "src-tauri\ofx"
$bin = Join-Path $out "Plugins\Pirosman Samples.ofx.bundle\Contents\Win64"
New-Item -ItemType Directory -Force $bin | Out-Null
Copy-Item (Join-Path $root "target\release\pirosman-ofxhost.exe") (Join-Path $out "pirosman-ofxhost.exe") -Force
Copy-Item (Join-Path $root "target\release\pirosman_ofx_samples.dll") (Join-Path $bin "Pirosman Samples.ofx") -Force
Write-Host "build-ofx: OpenFX host and sample plugins ready in src-tauri\ofx"

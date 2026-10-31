#!/usr/bin/env bash
# Build the OpenFX host and the bundled "Pirosman Samples" plugins into src-tauri/ofx/,
# which the AppImage and .deb ship as <resources>/ofx/:
#   ofx/pirosman-ofxhost                                           out-of-process plugin host
#   ofx/Plugins/Pirosman Samples.ofx.bundle/Contents/Linux-x86-64/... sample OpenFX effects
# Run automatically before `tauri build` / `tauri dev` on Linux
# (Windows uses scripts/build-ofx.ps1; `npm run build:ofx` picks the right one).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
[[ "$(uname -s)" == Linux ]] || { echo "build-ofx: ERROR: this script is for Linux (Windows uses scripts/build-ofx.ps1; macOS is not supported yet)" >&2; exit 1; }
# The OpenFX architecture folder (crates/pirosman-core/src/ofx.rs arch_dir).
case "$(uname -m)" in
  x86_64) arch="Linux-x86-64" ;;
  *) echo "build-ofx: ERROR: no OpenFX architecture folder for $(uname -m)" >&2; exit 1 ;;
esac
cargo build --release -p pirosman-ofxhost -p pirosman-ofx-samples || { echo "build-ofx: cargo build failed" >&2; exit 1; }
out="$root/src-tauri/ofx"
bin="$out/Plugins/Pirosman Samples.ofx.bundle/Contents/$arch"
mkdir -p "$bin"
install -m 755 target/release/pirosman-ofxhost "$out/pirosman-ofxhost"
install -m 755 target/release/libpirosman_ofx_samples.so "$bin/Pirosman Samples.ofx"
echo "build-ofx: OpenFX host and sample plugins ready in src-tauri/ofx"

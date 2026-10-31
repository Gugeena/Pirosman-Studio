#!/usr/bin/env bash
# Build the Linux release and copy it into dist-release/:
#   1. Pirosman-Studio_<ver>_amd64.AppImage  self-contained (FFmpeg, OpenFX host, GStreamer)
#   2. Pirosman-Studio_<ver>_amd64.deb       Debian/Ubuntu package
# Both carry the same pinned FFmpeg fetched and checksum-verified by
# scripts/fetch-ffmpeg.sh (run automatically by `tauri build` on Linux).
# Each package is then unpacked into target/linux-check/{deb,appimage} and checked.
#
# Usage (from the repo root): bash scripts/package-linux.sh
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
fail() { echo "package-linux: ERROR: $*" >&2; exit 1; }
[[ "$(uname -m)" == x86_64 ]] || fail "only x86_64 is packaged so far"
ver="$(node -p "require('./src-tauri/tauri.conf.json').version")"

# Keep the build machine's paths out of the shipped binaries (as package-windows.ps1 does).
# CARGO_ENCODED_RUSTFLAGS (0x1f-separated) keeps paths with spaces intact and wins over
# RUSTFLAGS; flags already set in either are kept.
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
sep=$'\x1f'
flags=()
if [[ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]]; then
  IFS="$sep" read -r -a flags <<<"$CARGO_ENCODED_RUSTFLAGS"
elif [[ -n "${RUSTFLAGS:-}" ]]; then
  read -r -a flags <<<"$RUSTFLAGS"
fi
flags+=("--remap-path-prefix=$root=/pirosman" "--remap-path-prefix=$cargo_home=/cargo")
CARGO_ENCODED_RUSTFLAGS="$(IFS="$sep"; echo "${flags[*]}")"
export CARGO_ENCODED_RUSTFLAGS
unset RUSTFLAGS
npx tauri build

bundle="$root/target/release/bundle"
appimage="$(ls -t "$bundle"/appimage/*_"$ver"_amd64.AppImage 2>/dev/null | head -n1)" || true
deb="$(ls -t "$bundle"/deb/*_"$ver"_amd64.deb 2>/dev/null | head -n1)" || true
[[ -n "$appimage" ]] || fail "no AppImage for $ver in $bundle/appimage"
[[ -n "$deb" ]] || fail "no .deb for $ver in $bundle/deb"

# Unpack both and check what ships.
check="$root/target/linux-check"
rm -rf "$check"; mkdir -p "$check/deb"
dpkg-deb -x "$deb" "$check/deb"
( cd "$check" && chmod +x "$appimage" && "$appimage" --appimage-extract >/dev/null && mv squashfs-root appimage )
manifest_version() { sed -n 's/.*"version": "\(.*\)".*/\1/p' "$1/manifest.json"; }
for pkg in deb appimage; do
  res="$(dirname "$(find "$check/$pkg" -path '*/ffmpeg/manifest.json' | head -n1)")/.."
  [[ -d "$res/ffmpeg" ]] || fail "$pkg: no ffmpeg/ resource folder"
  for f in ffmpeg/ffmpeg ffmpeg/ffprobe ofx/pirosman-ofxhost; do
    [[ -x "$res/$f" ]] || fail "$pkg: $f missing or not executable"
  done
  for f in ffmpeg/manifest.json ffmpeg/LICENSE.txt ffmpeg/NOTICE.txt ffmpeg/BUILD-README.txt THIRD_PARTY_NOTICES.md LICENSE-MIT LICENSE-APACHE \
           "ofx/Plugins/Pirosman Samples.ofx.bundle/Contents/Linux-x86-64/Pirosman Samples.ofx"; do
    [[ -f "$res/$f" ]] || fail "$pkg: $f missing"
  done
  "$res/ffmpeg/ffmpeg" -hide_banner -version | head -n1 | grep -qF "$(manifest_version "$res/ffmpeg")" || fail "$pkg: bundled ffmpeg is not the pinned build"
  # The app checks these sizes everywhere except inside an AppImage, whose bundler
  # (linuxdeploy) rewrites the RPATH of every ELF file (crates/pirosman-core/src/media.rs).
  if [[ $pkg == deb ]]; then
    for n in ffmpeg ffprobe; do
      want="$(grep -A1 "\"$n\": {" "$res/ffmpeg/manifest.json" | sed -n 's/.*"size": \([0-9]*\).*/\1/p')"
      [[ "$(stat -c %s "$res/ffmpeg/$n")" == "$want" ]] || fail "$pkg: ffmpeg/$n is not the size recorded in manifest.json"
    done
  fi
  [[ -z "$(find "$check/$pkg" -iname '*.exe' -o -iname '*.dll' -o -iname '*.ps1')" ]] || fail "$pkg: contains Windows files"
  app="$(find "$check/$pkg" -path '*/bin/pirosman-studio' -type f | head -n1)"
  [[ -x "$app" ]] || fail "$pkg: no executable pirosman-studio"
  for p in "$root" "$cargo_home" "$HOME"; do
    if grep -qaF "$p/" "$app" "$res/ofx/pirosman-ofxhost"; then fail "$pkg: shipped binaries contain the local path $p"; fi
  done
  echo "package-linux: $pkg verified ($(realpath --relative-to="$check/$pkg" "$res"))"
done

out="$root/dist-release"
mkdir -p "$out"
rm -f "$out"/Pirosman-Studio_*_amd64.AppImage "$out"/Pirosman-Studio_*_amd64.deb
cp "$appimage" "$out/Pirosman-Studio_${ver}_amd64.AppImage"
cp "$deb" "$out/Pirosman-Studio_${ver}_amd64.deb"
chmod +x "$out/Pirosman-Studio_${ver}_amd64.AppImage"
ls -l "$out"/Pirosman-Studio_*_amd64.*

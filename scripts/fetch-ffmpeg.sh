#!/usr/bin/env bash
# Fetch the pinned FFmpeg build that ships inside Pirosman Studio on Linux.
# (Windows uses scripts/fetch-ffmpeg.ps1; `npm run fetch-ffmpeg` picks the right one.)
#
# Pinned per CPU architecture: a GPL static build by BtbN/FFmpeg-Builds from the
# FFmpeg 9.0 release branch (9.0.2 plus fixes), taken from a dated monthly
# release. The SHA-256 equals the digest GitHub publishes for that asset. The
# archive is cached in .cache/ffmpeg and only ffmpeg, ffprobe and the license
# are extracted into src-tauri/ffmpeg/, which Tauri bundles into the AppImage and
# .deb (see src-tauri/tauri.linux.conf.json). The build fails if the checksum,
# version or required features do not match. Nothing is downloaded at runtime.
#
# Usage: bash scripts/fetch-ffmpeg.sh [--force]
set -euo pipefail

[[ "$(uname -s)" == Linux ]] || { echo "fetch-ffmpeg: ERROR: this script is for Linux (Windows uses scripts/fetch-ffmpeg.ps1; macOS is not supported yet)" >&2; exit 1; }
arch="$(uname -m)"
case "$arch" in
  x86_64)
    Tag="autobuild-2026-09-30-13-08"
    Archive="ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0.tar.xz"
    Sha256="68ee646831adaae2495618346f3bba94ff207ff83bbd34d643e7004730d66269"
    VersionId="n9.0.2-17-g2a571b6068-20260930" ;;   # in the first line of `ffmpeg -version`
  # aarch64: the same release has ffmpeg-n9.0.2-17-g2a571b6068-linuxarm64-gpl-9.0.tar.xz;
  # add it here (and to the notices) once an ARM64 build is tested.
  *) echo "fetch-ffmpeg: ERROR: no pinned FFmpeg for $arch (supported: x86_64)" >&2; exit 1 ;;
esac
Url="https://github.com/BtbN/FFmpeg-Builds/releases/download/$Tag/$Archive"
Prefix="${Archive%.tar.xz}"

root="$(cd "$(dirname "$0")/.." && pwd)"
# Everything Pirosman Studio asks FFmpeg for: one list shared with fetch-ffmpeg.ps1, kept
# complete by a render test (render_filters_are_all_required_of_the_pinned_ffmpeg).
Encoders="$(awk '$1 == "encoder" { print $2 }' "$root/scripts/ffmpeg-requirements.txt")"
Filters="$(awk '$1 == "filter" { print $2 }' "$root/scripts/ffmpeg-requirements.txt")"
cache="$root/.cache/ffmpeg"
out="$root/src-tauri/ffmpeg"
tarball="$cache/$Archive"
manifest="$out/manifest.json"
notice="$root/packaging/FFMPEG_NOTICE-linux.txt"
fail() { echo "fetch-ffmpeg: ERROR: $*" >&2; exit 1; }
sha() { sha256sum "$1" | cut -d' ' -f1; }

# The shipped notices must describe the pinned build (update them when bumping FFmpeg).
for doc in "$notice" "$root/THIRD_PARTY_NOTICES.md"; do
  grep -qF "$Url" "$doc" && grep -qF "$Sha256" "$doc" || fail "$doc does not name the pinned archive URL and SHA-256"
done

# Up to date? (same pinned archive, both executables present with recorded sizes)
if [[ "${1:-}" != "--force" && -f "$manifest" ]] && grep -qF "\"archive_sha256\": \"$Sha256\"" "$manifest"; then
  ok=1
  for n in ffmpeg ffprobe; do
    size="$(grep -A1 "\"$n\": {" "$manifest" | sed -n 's/.*"size": \([0-9]*\).*/\1/p')"
    [[ -x "$out/$n" && "$(stat -c %s "$out/$n")" == "$size" ]] || ok=0
  done
  if [[ $ok == 1 ]]; then
    install -m 644 "$notice" "$out/NOTICE.txt"   # an edited notice always ships
    echo "fetch-ffmpeg: FFmpeg $VersionId already in src-tauri/ffmpeg"; exit 0
  fi
fi

mkdir -p "$cache"
if [[ ! -f "$tarball" || "$(sha "$tarball")" != "$Sha256" ]]; then
  echo "fetch-ffmpeg: downloading $Url"
  curl -fsSL --retry 3 -o "$tarball.partial" "$Url" || fail "download failed"
  mv "$tarball.partial" "$tarball"
fi
got="$(sha "$tarball")"
if [[ "$got" != "$Sha256" ]]; then
  rm -f "$tarball"
  fail "SHA-256 mismatch for $Archive. Expected $Sha256, got $got. The download was deleted; do not bundle it."
fi
echo "fetch-ffmpeg: SHA-256 verified ($Sha256)"

# Extract only what ships.
stage="$cache/stage"
rm -rf "$stage"; mkdir -p "$stage"
tar -xJf "$tarball" -C "$stage" "$Prefix/bin/ffmpeg" "$Prefix/bin/ffprobe" "$Prefix/LICENSE.txt" || fail "archive lacks bin/ffmpeg, bin/ffprobe or LICENSE.txt"
mv "$stage/$Prefix/bin/ffmpeg" "$stage/$Prefix/bin/ffprobe" "$stage/$Prefix/LICENSE.txt" "$stage/"
rm -rf "${stage:?}/$Prefix"
chmod 755 "$stage/ffmpeg" "$stage/ffprobe"

# Verify the binaries actually are the pinned build and can do what the app needs.
v1="$("$stage/ffmpeg" -hide_banner -version | head -n1)"
v2="$("$stage/ffprobe" -hide_banner -version | head -n1)"
[[ "$v1" == *"$VersionId"* ]] || fail "ffmpeg reports '$v1', expected $VersionId"
[[ "$v2" == *"$VersionId"* ]] || fail "ffprobe reports '$v2', expected $VersionId"
enc="$("$stage/ffmpeg" -hide_banner -encoders)"
fil="$("$stage/ffmpeg" -hide_banner -filters)"
missing=""
for e in $Encoders; do grep -qE "\s$e\s" <<<"$enc" || missing="$missing encoder:$e"; done
for f in $Filters;  do grep -qE "\s$f\s" <<<"$fil" || missing="$missing filter:$f"; done
[[ -z "$missing" ]] || fail "pinned FFmpeg lacks:$missing"
echo "fetch-ffmpeg: $(wc -w <<<"$Encoders") encoders and $(wc -w <<<"$Filters") filters present"
# The builder publishes no readme in the archive: record its configuration instead.
"$stage/ffmpeg" -hide_banner -buildconf > "$stage/BUILD-README.txt"

# Publish into src-tauri/ffmpeg (replacing any previous version entirely).
rm -rf "$out"; mkdir -p "$out"
cp -p "$stage"/* "$out/"
install -m 644 "$notice" "$out/NOTICE.txt"
entry() { printf '    "%s": {\n      "size": %s,\n      "sha256": "%s"\n    }' "$1" "$(stat -c %s "$out/$1")" "$(sha "$out/$1")"; }
# Read by crates/pirosman-core/src/media.rs (validate_bundled).
cat > "$manifest" <<EOF
{
  "version": "$VersionId",
  "source_url": "$Url",
  "archive_sha256": "$Sha256",
  "license": "GPL-3.0-or-later (see LICENSE.txt and NOTICE.txt)",
  "files": {
$(entry ffmpeg),
$(entry ffprobe)
  }
}
EOF
rm -rf "$stage"
echo "fetch-ffmpeg: bundled FFmpeg $VersionId ready in src-tauri/ffmpeg"

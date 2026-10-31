#!/usr/bin/env bash
# Launch a packaged Pirosman Studio on a virtual display and check that it stays
# up and runs the FFmpeg it ships (the start page asks for the FFmpeg status),
# never a system FFmpeg or a Windows tool. Needs xvfb-run and strace.
#
# Usage: bash scripts/smoke-linux.sh <path to pirosman-studio or the AppImage>
set -euo pipefail
app="$1"
log="$(mktemp -d)"
fail() { echo "smoke-linux: ERROR: $*" >&2; tail -n 40 "$log/out" >&2 || true; exit 1; }
# A virtual display has no GPU: render in software so WebKit draws the page. And use
# it even inside a desktop session (WSLg sets WAYLAND_DISPLAY, which GTK would prefer).
export WEBKIT_DISABLE_COMPOSITING_MODE=1 WEBKIT_DISABLE_DMABUF_RENDERER=1 LIBGL_ALWAYS_SOFTWARE=1 GDK_BACKEND=x11
unset WAYLAND_DISPLAY
set +e
xvfb-run -a strace -f -qq -e trace=execve -o "$log/trace" timeout 30 "$app" >"$log/out" 2>&1
code=$?
set -e
# `timeout` ends a healthy app with 124; anything else means it quit or crashed.
[[ $code == 124 ]] || fail "the app exited by itself (code $code)"
grep -q "panicked" "$log/out" && fail "the app panicked"
ff="$(grep -o 'execve("[^"]*/ffmpeg/ffmpeg"' "$log/trace" | head -n1 | cut -d'"' -f2)" || true
[[ -n "$ff" ]] || fail "the app never ran its bundled FFmpeg"
grep -qE 'execve\("(/usr)?/bin/(ffmpeg|ffprobe)"' "$log/trace" && fail "the app ran a system FFmpeg"
grep -qiE 'execve\("[^"]*(powershell|pwsh|cmd\.exe|\.exe")' "$log/trace" && fail "the app tried to run a Windows program"
echo "smoke-linux: $app stayed up and ran its bundled FFmpeg: $ff"

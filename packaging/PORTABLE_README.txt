PIROSMAN STUDIO (portable, Windows 10/11 64-bit)
================================================

Extract the whole zip and run "Pirosman Studio.exe". Keep everything together:

  Pirosman Studio.exe
  ffmpeg\                  FFmpeg 9.0.2 (ffmpeg.exe, ffprobe.exe, licenses)
  ofx\                      OpenFX plugin host and the bundled sample plugins
  README.txt                this file
  LICENSE-MIT, LICENSE-APACHE, THIRD_PARTY_NOTICES.md

FFmpeg is included. You do not need to install FFmpeg, winget, or change PATH.
Help > FFmpeg status... shows which FFmpeg is in use.

Prefer the installer (Pirosman-Studio_..._x64-setup.exe) if you have it: it
also installs Microsoft Edge WebView2 when it is missing.


1. "Windows protected your PC"
   The app is not code-signed. Click "More info", then "Run anyway".

2. Microsoft Edge WebView2 Runtime (draws the interface)
   It is already on Windows 11 and on updated Windows 10. If it is missing,
   Pirosman Studio says so at start-up and offers Microsoft's free download.
   No internet on that PC? Download the "Evergreen Standalone Installer" on
   another PC from https://developer.microsoft.com/microsoft-edge/webview2/

3. If the app says FFmpeg could not be loaded
   The ffmpeg folder was removed or damaged (for example by antivirus or a
   partial extraction). Extract the zip again. Advanced users can point the
   PIROSMAN_FFMPEG_DIR environment variable at another FFmpeg build.

4. Sharing projects
   A .pirosman file points to media on the computer that made it. To send a
   project with its media and history, use Revisions > Exchange > Export
   bundle with "include media" ticked; the other person uses Import bundle.

5. Live Sync between PCs
   Both PCs must be on the same network. When hosting, allow Pirosman Studio
   through Windows Firewall when Windows asks. The connection is not
   encrypted: use it only on a network you trust.

Requirements: Windows 10 (1809 or later) or Windows 11, 64-bit.
Not supported: macOS, Linux or 32-bit Windows.
Windows on ARM may run it under x64 emulation but has not been tested.

Licenses
  Pirosman Studio: MIT OR Apache-2.0 (LICENSE-MIT, LICENSE-APACHE).
  Bundled FFmpeg: GNU GPL v3 (ffmpeg\LICENSE.txt). Build details and source
  links: ffmpeg\NOTICE.txt. All third-party notices: THIRD_PARTY_NOTICES.md.

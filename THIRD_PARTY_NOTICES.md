# Third-party notices

Pirosman Studio is licensed MIT OR Apache-2.0 (see `LICENSE-MIT` and `LICENSE-APACHE`). The Windows installer and the portable zip ship those two files, this file, and the third-party programs listed below. The third-party programs keep their own licenses.

## FFmpeg (bundled, GPL v3)

- **What:** `ffmpeg.exe` and `ffprobe.exe`, unmodified, from the FFmpeg 9.0.2 "essentials" static Windows x64 build by [gyan.dev](https://www.gyan.dev/ffmpeg/builds/).
- **Archive:** <https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip>
- **SHA-256:** `60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba`
  - This equals the digest GitHub publishes for the release asset.
  - It is enforced by `scripts/fetch-ffmpeg.ps1`.
- **Installed to:** the `ffmpeg\` folder next to `pirosman-studio.exe` (installer) or `Pirosman Studio.exe` (portable). That folder also holds:
  - `LICENSE.txt`, the GPL v3 text;
  - `BUILD-README.txt`, the builder's configuration and library list;
  - `NOTICE.txt`, a short notice with source links (from `packaging/FFMPEG_NOTICE.txt`);
  - `manifest.json`, the pinned version, sizes and hashes.
- **License:** the build is configured with `--enable-gpl --enable-version3` and includes GPL libraries such as libx264 and libx265, so the binaries are under the GNU GPL v3.
- **How it is used:** Pirosman Studio runs FFmpeg as a separate process. It does not link against FFmpeg.
- **Source code:**
  - FFmpeg 9.0.2: <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz>
  - Commit named in the builder's `BUILD-README.txt`: <https://github.com/FFmpeg/FFmpeg/commit/946fcce07b>
  - Third-party libraries: `BUILD-README.txt` lists the builder's external libraries and versions. The binaries also statically include libraries that list does not name; version strings embedded in `ffmpeg.exe` indicate OpenSSL 3.6.x, GnuTLS 3.8.13, nettle 4.0, GMP 6.3.0, libsodium 1.0.22, libunibreak 5.1, pixman 0.46.4, expat 2.8.4 and zlib 1.3.1, plus bzip2, brotli, libtasn1 and libiconv (versions not recorded).
- FFmpeg is a trademark of Fabrice Bellard. Pirosman Studio is not affiliated with the FFmpeg project or gyan.dev.

## Microsoft Edge WebView2

- **WebView2 loader:** the release (MSVC) build links Microsoft's WebView2 loader statically into the executable (`WebView2LoaderStatic.lib` from the WebView2 SDK, via the `webview2-com-sys` crate), so no separate `WebView2Loader.dll` ships. The loader is under Microsoft's WebView2 SDK license.
- **WebView2 Runtime:** not shipped. The installer runs Microsoft's Evergreen bootstrapper, `MicrosoftEdgeWebview2Setup.exe`, when the runtime is missing. That download is subject to Microsoft's terms.

## Rust and npm dependencies

The application is built from many open-source crates and npm packages, including Tauri, React, serde and tungstenite, under MIT, Apache-2.0 and similar licenses. See `Cargo.lock` and `package-lock.json` for the exact list.

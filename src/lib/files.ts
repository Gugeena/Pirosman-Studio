// URLs for local files in the WebView.
//
// Pictures the app generates into its cache arrive as `pcache:<relative path>` and are
// served by the app's own `pcache` scheme (src-tauri/src/lib.rs), which never depends on
// the asset-protocol scope. Everything else (media, proxies) goes through the asset protocol,
// or on Linux the media server below.
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { homeDir } from "@tauri-apps/api/path";
import { save, type SaveDialogOptions } from "@tauri-apps/plugin-dialog";

const WINDOWS = typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);

// Linux: WebKitGTK's media player refuses custom URI schemes, so local files come
// from the app's loopback media server instead (src-tauri/src/media_server.rs).
let mediaBase: string | null = null;
// Linux: an AppImage runs inside its read-only image (WebKit needs that working directory),
// so a bare file name in a save dialog or an export path would land there. Use home instead.
let home: string | null = null;
/** Call before rendering; returns a problem to show the user, if any. */
export async function initFiles(): Promise<string | null> {
  if (!WINDOWS) home = await homeDir().catch(() => null);
  try {
    mediaBase = await invoke<string | null>("media_base_url");
    return null;
  } catch (e) {
    return String(e);
  }
}

/** `name` in the home folder on Linux when it has no folder of its own; unchanged otherwise. */
export const inHome = (name: string) => (home && !/[\\/]/.test(name) ? `${home.replace(/\/$/, "")}/${name}` : name);

/** Save dialog that suggests `name` (a file name: path separators become "_"), in the home folder on Linux. */
export const saveDialog = (name: string, opts: Omit<SaveDialogOptions, "defaultPath">) =>
  save({ ...opts, defaultPath: inHome(name.replace(/[\\/]/g, "_")) });

export function fileSrc(p: string): string {
  if (p.startsWith("pcache:")) {
    const rel = p.slice("pcache:".length).split("/").map(encodeURIComponent).join("/");
    return WINDOWS ? `http://pcache.localhost/${rel}` : `pcache://localhost/${rel}`;
  }
  return mediaBase ? `${mediaBase}/${encodeURIComponent(p)}` : convertFileSrc(p);
}

/** `url("…")` for a CSS background, quoted so any URL is safe. */
export const cssUrl = (p: string) => `url("${fileSrc(p).replace(/"/g, "%22")}")`;

// URLs for local files in the WebView.
//
// Pictures the app generates into its cache arrive as `pcache:<relative path>` and are
// served by the app's own `pcache` scheme (src-tauri/src/lib.rs), which never depends on
// the asset-protocol scope. Everything else (media, proxies) goes through the asset protocol.
import { convertFileSrc } from "@tauri-apps/api/core";

const WINDOWS = typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);

export function fileSrc(p: string): string {
  if (p.startsWith("pcache:")) {
    const rel = p.slice("pcache:".length).split("/").map(encodeURIComponent).join("/");
    return WINDOWS ? `http://pcache.localhost/${rel}` : `pcache://localhost/${rel}`;
  }
  return convertFileSrc(p);
}

/** `url("…")` for a CSS background, quoted so any URL is safe. */
export const cssUrl = (p: string) => `url("${fileSrc(p).replace(/"/g, "%22")}")`;

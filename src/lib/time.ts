import type { Rational } from "./api";

export const fps = (r: Rational) => r.num / r.den;
export const nominal = (r: Rational) => Math.max(1, Math.round(r.num / r.den));

/** Non-drop-frame timecode HH:MM:SS:FF, matching crates/pirosman-core/src/time.rs. */
export function timecode(frames: number, r: Rational): string {
  const n = nominal(r);
  const neg = frames < 0;
  const f = Math.abs(Math.round(frames));
  const ff = f % n;
  const s = Math.floor(f / n);
  const pad = (x: number) => String(x).padStart(2, "0");
  return `${neg ? "-" : ""}${pad(Math.floor(s / 3600))}:${pad(Math.floor(s / 60) % 60)}:${pad(s % 60)}:${pad(ff)}`;
}

export const framesToSeconds = (frames: number, r: Rational) => (frames * r.den) / r.num;
export const secondsToFrames = (s: number, r: Rational) => Math.round((s * r.num) / r.den);

export function shortDuration(frames: number, r: Rational): string {
  const s = framesToSeconds(frames, r);
  if (s < 60) return `${s.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  return `${m}:${String(Math.floor(s % 60)).padStart(2, "0")}`;
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const u = ["KB", "MB", "GB", "TB"];
  let i = -1;
  let v = n;
  do { v /= 1024; i++; } while (v >= 1024 && i < u.length - 1);
  return `${v.toFixed(v < 10 ? 1 : 0)} ${u[i]}`;
}

export function relTime(iso: string): string {
  const d = new Date(iso).getTime();
  const s = Math.round((Date.now() - d) / 1000);
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return new Date(iso).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

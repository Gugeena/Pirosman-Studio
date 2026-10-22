// Clip timing helpers mirroring crates/pirosman-core/src/model.rs.
import type { Clip, Project, TitleClip, Track } from "./api";

export const speedOf = (c: Clip) => c.speed ?? 1;

/** Timeline frames occupied: source frames / speed, rounded (min 1). */
export function clipDur(c: Clip): number {
  const s = speedOf(c);
  const len = c.source_out - c.source_in;
  return s === 1 ? len : Math.max(1, Math.round(len / s));
}
export const clipEnd = (c: Clip) => c.start + clipDur(c);
export const titleEnd = (t: TitleClip) => t.start + t.duration;

/** Source frame shown at timeline `frame` (inside the clip), honouring speed and reverse. */
export function sourceFrameAt(c: Clip, frame: number): number {
  const off = (frame - c.start) * speedOf(c);
  return c.reversed ? c.source_out - 1 - off : c.source_in + off;
}

/** 0..1 visibility from fades and dip-to-black halves at `frame` (live preview approximation). */
export function fadeOpacity(t: Track, c: Clip, frame: number): number {
  const d = clipDur(c);
  const k = frame - c.start;
  let o = 1;
  const tin = c.transition_in;
  const next = t.clips.find((x) => x.id !== c.id && x.start === clipEnd(c));
  const tout = next?.transition_in;
  const fin = tin ? (tin.kind === "dip_to_black" ? tin.duration_frames - Math.floor(tin.duration_frames / 2) : Math.floor(tin.duration_frames / 2)) : c.fade_in_frames ?? 0;
  const fout = tout ? (tout.kind === "dip_to_black" ? Math.floor(tout.duration_frames / 2) : 0) : c.fade_out_frames ?? 0;
  if (fin > 0 && k < fin) o = Math.min(o, (k + 1) / (fin + 1));
  if (fout > 0 && k >= d - fout) o = Math.min(o, (d - k) / (fout + 1));
  return Math.max(0, Math.min(1, o));
}

export function timelineEnd(p: Project | undefined): number {
  if (!p) return 0;
  let m = 0;
  for (const t of p.tracks) {
    for (const c of t.clips) m = Math.max(m, clipEnd(c));
    for (const x of t.titles ?? []) m = Math.max(m, titleEnd(x));
  }
  return m;
}

export function findTitle(p: Project | undefined, id: string | null | undefined): { track: Track; title: TitleClip } | null {
  if (!p || !id) return null;
  for (const t of p.tracks) {
    const x = (t.titles ?? []).find((x) => x.id === id);
    if (x) return { track: t, title: x };
  }
  return null;
}

export function fmtSpeed(c: Clip): string | null {
  const s = speedOf(c);
  if (s === 1 && !c.reversed) return null;
  const n = s === 1 ? "" : `${Number.isInteger(s) ? s.toFixed(1) : String(s)}x`;
  return c.reversed ? `${n}${n ? " " : ""}REV` : n;
}

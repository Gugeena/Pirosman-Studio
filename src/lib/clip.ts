// Clip timing helpers mirroring crates/pirosman-core/src/model.rs.
import type { Clip, ClipPatch, Keyframe, Project, TitleClip, Track, Transform, TransitionKind } from "./api";

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

const STILL: Transform = { scale_x: 1, scale_y: 1, position_x: 0, position_y: 0, rotation_deg: 0 };

/** Transform and opacity at timeline `frame`: keyframes interpolate by source frame, as render.rs `keyed` does. */
export function animatedAt(c: Clip, frame: number): { tf: Transform; opacity: number } {
  const k = c.keyframes ?? [];
  if (!k.length) return { tf: c.transform ?? STILL, opacity: c.opacity ?? 1 };
  const s = sourceFrameAt(c, frame);
  const i = k.findIndex((x) => x.frame > s);
  if (i === 0) return { tf: k[0].transform, opacity: k[0].opacity };
  if (i < 0) return { tf: k[k.length - 1].transform, opacity: k[k.length - 1].opacity };
  const a = k[i - 1], b = k[i], u = (s - a.frame) / (b.frame - a.frame);
  const l = (x: number, y: number) => x + (y - x) * u;
  const ta = a.transform, tb = b.transform;
  return {
    tf: { scale_x: l(ta.scale_x, tb.scale_x), scale_y: l(ta.scale_y, tb.scale_y), position_x: l(ta.position_x, tb.position_x), position_y: l(ta.position_y, tb.position_y), rotation_deg: l(ta.rotation_deg, tb.rotation_deg) },
    opacity: l(a.opacity, b.opacity),
  };
}

/** The clip's keyframes with one set at the source frame under timeline `frame` (added or replaced). */
export function withKeyframe(c: Clip, frame: number, tf: Transform, opacity: number): Keyframe[] {
  const f = Math.round(sourceFrameAt(c, frame));
  return [...(c.keyframes ?? []).filter((k) => k.frame !== f), { frame: f, transform: tf, opacity }].sort((a, b) => a.frame - b.frame);
}

/** Patch setting the layer's transform/opacity at `frame`: a keyframe there when the clip is animated. */
export function layerPatch(c: Clip, frame: number, p: { transform?: Transform; opacity?: number }): ClipPatch {
  if (!c.keyframes?.length) return p;
  const cur = animatedAt(c, frame);
  return { keyframes: withKeyframe(c, frame, p.transform ?? cur.tf, p.opacity ?? cur.opacity) };
}

/** Timeline frame where source frame `f` of the clip plays. */
export const timelineFrameOf = (c: Clip, f: number) => c.start + (c.reversed ? c.source_out - 1 - f : f - c.source_in) / speedOf(c);

/** Every transition kind with its label, in menu order (mirrors `TransitionKind` in pirosman-core). */
export const TRANSITIONS: Record<TransitionKind, string> = {
  cross_dissolve: "Cross dissolve", wipe: "Edge wipe", slide: "Slide", push: "Push", dip_to_black: "Dip to black", dip_to_white: "Dip to white",
};
/** Overlapping kinds play both clips across the cut (and need handles); dips go through a colour. */
export const overlaps = (k: TransitionKind) => k !== "dip_to_black" && k !== "dip_to_white";

/** The clip that ends where `c` starts on the same track (the outgoing side of its transition). */
export const prevClip = (t: Track, c: Clip) => t.clips.find((x) => x.id !== c.id && clipEnd(x) === c.start);
export const nextClip = (t: Track, c: Clip) => t.clips.find((x) => x.id !== c.id && x.start === clipEnd(c));

/** Frames an overlapping transition adds before the clip's start and after its end (render.rs `handles`). */
export function handlesOf(t: Track, c: Clip): [number, number] {
  const tin = c.transition_in, tout = nextClip(t, c)?.transition_in;
  return [
    tin && overlaps(tin.kind) ? Math.floor(tin.duration_frames / 2) : 0,
    tout && overlaps(tout.kind) ? tout.duration_frames - Math.floor(tout.duration_frames / 2) : 0,
  ];
}

/** 0..1 visibility from fades and dip halves at `frame` (live preview approximation; also drives audio gain). */
export function fadeOpacity(t: Track, c: Clip, frame: number): number {
  const d = clipDur(c);
  const k = frame - c.start;
  let o = 1;
  const tin = c.transition_in;
  const tout = nextClip(t, c)?.transition_in;
  const fin = tin ? (!overlaps(tin.kind) ? tin.duration_frames - Math.floor(tin.duration_frames / 2) : Math.floor(tin.duration_frames / 2)) : c.fade_in_frames ?? 0;
  const fout = tout ? (!overlaps(tout.kind) ? Math.floor(tout.duration_frames / 2) : 0) : c.fade_out_frames ?? 0;
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

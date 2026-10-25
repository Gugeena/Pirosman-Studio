import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { fileSrc } from "../lib/files";
import {
  ArrowLeftRight, Camera, FastForward, Rewind, ChevronFirst, ChevronLast, ChevronLeft, ChevronRight, Clapperboard, Columns2, CornerDownRight, ExternalLink, Loader2, Pause, Play,
  Puzzle, RectangleHorizontal, Replace, StepBack, StepForward, X,
} from "lucide-react";
import { api, DEFAULT_TEXT_STYLE, type Clip, type ColorParams, type Crop, type MediaInfo, type MediaRef, type Project, type TitleClip, type Track, type Transform } from "../lib/api";
import { animatedAt, clipDur, clipEnd, fadeOpacity, handlesOf, layerPatch, nextClip, overlaps, sourceFrameAt, speedOf, titleEnd } from "../lib/clip";
import { attempt, infoMap, mediaMap, projectDuration, run, useUI } from "../lib/store";
import { framesToSeconds, secondsToFrames, timecode } from "../lib/time";
import { dbToLinear, dropStripsExcept, resumeAudio, setStrip, strip } from "../lib/audioEngine";
import { grabStillFlow } from "../lib/actions";
import { fpsLabel } from "../lib/presets";

/** Linear coefficients matching the export's FFmpeg `colorlevels` mapping. */
function levels(c: ColorParams) {
  const imin = Math.max(0, -c.lift), omin = Math.max(0, c.lift);
  const imax = c.gain > 1 ? 1 / c.gain : 1, omax = c.gain < 1 ? c.gain : 1;
  const k = (omax - omin) / (imax - imin);
  return { slope: k, intercept: omin - imin * k };
}

/** SVG filter approximating the export grade (levels → contrast → gamma → saturation). */
export function GradeFilter({ id, c }: { id: string; c: ColorParams }) {
  const l = levels(c);
  const funcs = (attrs: Record<string, number | string>) => [<feFuncR key="r" {...attrs} />, <feFuncG key="g" {...attrs} />, <feFuncB key="b" {...attrs} />];
  return (
    <svg width="0" height="0" style={{ position: "absolute" }} aria-hidden="true">
      <filter id={id} colorInterpolationFilters="sRGB">
        <feComponentTransfer>{funcs({ type: "linear", slope: l.slope, intercept: l.intercept })}</feComponentTransfer>
        <feComponentTransfer>{funcs({ type: "linear", slope: c.contrast, intercept: 0.5 - 0.5 * c.contrast })}</feComponentTransfer>
        <feComponentTransfer>{funcs({ type: "gamma", amplitude: 1, exponent: 1 / c.gamma, offset: 0 })}</feComponentTransfer>
        <feColorMatrix type="saturate" values={String(Math.min(c.saturation, 3))} />
      </filter>
    </svg>
  );
}

const ratio = (a: string) => { const [w, h] = a.split("/").map(Number); return String(w / h || 16 / 9); };
const neutral = (c: ColorParams) => c.lift === 0 && c.gamma === 1 && c.gain === 1 && c.contrast === 1 && c.saturation === 1;

const IDENT: Transform = { scale_x: 1, scale_y: 1, position_x: 0, position_y: 0, rotation_deg: 0 };
const NO_CROP: Crop = { left: 0, right: 0, top: 0, bottom: 0 };

/** `slot` alternates per clip so both sides of a transition get their own element (A/B roll). */
type Item = { kind: "clip"; track: Track; clip: Clip; media: MediaRef; slot: number } | { kind: "title"; track: Track; title: TitleClip };
type ClipItem = Extract<Item, { kind: "clip" }>;
type TitleItem = Extract<Item, { kind: "title" }>;
const itemId = (it: Item) => (it.kind === "clip" ? it.clip.id : it.title.id);

/** Everything visible at `frame`, bottom to top, in the export's order (per track: its clips, then its titles). */
function itemsAt(p: Project, media: Map<string, MediaRef>, frame: number): Item[] {
  const out: Item[] = [];
  for (const t of p.tracks.filter((t) => t.kind === "video" && !t.muted)) {
    // Across an overlapping transition both clips play (incoming on top), as in the export.
    t.clips.forEach((c, i) => {
      const [lead, tail] = handlesOf(t, c);
      const m = media.get(c.media_id);
      if (m && c.start - lead <= frame && frame < clipEnd(c) + tail) out.push({ kind: "clip", track: t, clip: c, media: m, slot: i % 2 });
    });
    for (const x of t.titles ?? []) if (x.start <= frame && frame < titleEnd(x)) out.push({ kind: "title", track: t, title: x });
  }
  return out;
}

/** The cropped picture fitted into the frame like the export (`force_original_aspect_ratio=decrease`), as frame fractions. */
function fitBox(c: Clip, m: MediaRef, W: number, H: number) {
  const k = c.crop ?? NO_CROP;
  const mw = (m.width || W) * (1 - k.left - k.right), mh = (m.height || H) * (1 - k.top - k.bottom);
  const f = Math.min(W / mw, H / mh);
  return { w: (mw * f) / W, h: (mh * f) / H };
}

/** Offset by a transform's position (fractions of the frame); `ar` is height / width. */
const offset = (tf: Transform, ar: number) => `translate(${tf.position_x * 100}cqw, ${tf.position_y * ar * 100}cqw)`;

/** CSS approximations of the clip's effects (export renders the real filters). */
function vfxLook(c: Clip, gradeId: string | null): { filter?: string; overlays: JSX.Element[] } {
  const overlays: JSX.Element[] = [];
  const filters: string[] = [];
  if (gradeId) filters.push(`url(#${gradeId})`);
  for (const fx of (c.vfx ?? []).filter((f) => f.enabled && f.amount > 0)) {
    const a = fx.amount;
    if (fx.kind === "gaussian_blur") filters.push(`blur(${(0.5 + a * 24.5) * 0.35}px)`);
    if (fx.kind === "glow") filters.push(`brightness(${1 + a * 0.18}) drop-shadow(0 0 ${4 + a * 14}px rgba(255,170,110,${a * 0.6}))`);
    if (fx.kind === "chromatic_aberration") filters.push(`drop-shadow(${a * 3}px 0 0 rgba(255,0,0,.45)) drop-shadow(${-a * 3}px 0 0 rgba(0,90,255,.45))`);
    if (fx.kind === "vignette") overlays.push(<div key={fx.id} className="fx-vignette" style={{ opacity: 0.25 + a * 0.75 }} />);
    if (fx.kind === "film_grain") overlays.push(<div key={fx.id} className="fx-grain" style={{ opacity: a * 0.55 }} />);
  }
  return { filter: filters.length ? filters.join(" ") : undefined, overlays };
}

/** How a clip's fades and transitions show at `frame`, mirroring render.rs (dx and wipe are frame fractions). */
function transitionLook(t: Track, c: Clip, frame: number) {
  let opacity = 1, dx = 0, wipe = 1;
  let dip: { color: string; amount: number } | null = null;
  const tin = c.transition_in, tout = nextClip(t, c)?.transition_in;
  const [lead] = handlesOf(t, c);
  const color = (k: string) => (k === "dip_to_white" ? "#fff" : "#000");
  const k = frame - c.start, dur = clipDur(c);
  if (tin && overlaps(tin.kind)) {
    const d = tin.duration_frames, n = frame - (c.start - lead);
    if (n < d) {
      if (tin.kind === "wipe") wipe = (n + 1) / (d + 1);
      else if (tin.kind === "slide" || tin.kind === "push") dx = 1 - n / d;
      else opacity = n / d;
    }
  } else if (tin) {
    const n = tin.duration_frames - Math.floor(tin.duration_frames / 2);
    if (k < n) dip = { color: color(tin.kind), amount: 1 - k / n };
  } else if ((c.fade_in_frames ?? 0) > 0 && k < c.fade_in_frames!) opacity = (k + 1) / (c.fade_in_frames! + 1);
  if (tout && overlaps(tout.kind)) {
    const n = frame - (clipEnd(c) - Math.floor(tout.duration_frames / 2));
    if (tout.kind === "push" && n >= 0) dx = -Math.min(1, n / tout.duration_frames);
  } else if (tout) {
    const n = Math.floor(tout.duration_frames / 2), j = k - (dur - n);
    if (j >= 0) dip = { color: color(tout.kind), amount: Math.min(1, (j + 1) / n) };
  } else if ((c.fade_out_frames ?? 0) > 0 && k >= dur - c.fade_out_frames!) opacity = (dur - k) / (c.fade_out_frames! + 1);
  return { opacity: Math.max(0, Math.min(1, opacity)), dx, wipe, dip };
}

type Sync = (el: HTMLMediaElement, src: string | null, seconds: number, rateMul: number, reversed: boolean) => void;

/** One video track's picture: crop and fit as the export does, then transform, opacity and fades. */
function LayerView({ it, tf, info, frame, playing, shuttle, rate, W, H, grade, sync }: {
  it: ClipItem; tf: Transform; info?: MediaInfo; frame: number; playing: boolean; shuttle: number;
  rate: Project["settings"]["frame_rate"]; W: number; H: number; grade: ColorParams | null; sync: Sync;
}) {
  const vref = useRef<HTMLVideoElement>(null);
  const { clip: c, media: m, track: t } = it;
  const still = m.kind === "image";
  const tr = transitionLook(t, c, frame);
  useEffect(() => {
    const el = vref.current;
    if (!el) return;
    el.muted = true;
    sync(el, info?.preview ?? null, framesToSeconds(sourceFrameAt(c, frame), rate), speedOf(c) * Math.abs(shuttle), !!c.reversed || shuttle < 0);
  }, [frame, playing, c.id, info?.preview, shuttle]);
  const b = fitBox(c, m, W, H);
  const k = c.crop ?? NO_CROP;
  const cw = 1 - k.left - k.right, ch = 1 - k.top - k.bottom;
  const gradeId = grade && !neutral(grade) ? `grade-${c.id}` : null;
  const look = vfxLook(c, gradeId);
  const inner: React.CSSProperties = { width: `${100 / cw}%`, height: `${100 / ch}%`, left: `${(-k.left / cw) * 100}%`, top: `${(-k.top / ch) * 100}%` };
  return (
    <div
      className="layer"
      style={{
        left: `${(0.5 - b.w / 2) * 100}%`, top: `${(0.5 - b.h / 2) * 100}%`, width: `${b.w * 100}%`, height: `${b.h * 100}%`,
        opacity: animatedAt(c, frame).opacity * tr.opacity, filter: look.filter,
        clipPath: tr.wipe < 1 ? `inset(0 ${(1 - tr.wipe) * 100}% 0 0)` : undefined,
        transform: `translateX(${tr.dx * 100}cqw) ${offset(tf, H / W)} rotate(${tf.rotation_deg}deg) scale(${tf.scale_x}, ${tf.scale_y})`,
      }}
    >
      {gradeId && grade && <GradeFilter id={gradeId} c={grade} />}
      {still ? <img src={fileSrc(m.path)} alt="" style={inner} /> : <video ref={vref} preload="auto" crossOrigin="anonymous" style={{ ...inner, visibility: info?.preview ? "visible" : "hidden" }} />}
      {look.overlays}
      {tr.dip && <div className="dip" style={{ background: tr.dip.color, opacity: tr.dip.amount }} />}
    </div>
  );
}

/** A title drawn like the export's `drawtext` (font, outline, shadow, box, placement). */
function titleStyle(x: TitleClip, frame: number, W: number, ar: number, tf: Transform | null): React.CSSProperties {
  const s = { ...DEFAULT_TEXT_STYLE, ...x.style };
  const k = frame - x.start;
  let o = s.opacity;
  if ((x.fade_in_frames ?? 0) > 0 && k < x.fade_in_frames!) o *= (k + 1) / (x.fade_in_frames! + 1);
  if ((x.fade_out_frames ?? 0) > 0 && k >= x.duration - x.fade_out_frames!) o *= (x.duration - k) / (x.fade_out_frames! + 1);
  const scale = tf ? tf.scale_x : 1;
  const base: React.CSSProperties = {
    opacity: o, color: x.color_hex, fontSize: `${((x.font_size * scale) / W) * 100}cqw`,
    fontFamily: `"${s.font || "Segoe UI"}", system-ui, sans-serif`, fontWeight: s.bold ? 700 : 400, textAlign: s.align, lineHeight: 1 + s.line_spacing,
    textShadow: s.shadow ? "2px 2px 0 rgba(0,0,0,.55)" : "none",
    // drawtext's border sits outside the glyph: a centred stroke twice as wide, painted under the fill.
    WebkitTextStroke: s.outline_px ? `${((2 * s.outline_px * scale) / W) * 100}cqw ${s.outline_hex}` : undefined, paintOrder: "stroke fill",
    background: s.background ?? undefined, padding: s.background ? "0.25em" : undefined,
  };
  if (tf) return { ...base, left: "50%", top: "50%", transform: `translate(-50%, -50%) ${offset(tf, ar)} rotate(${tf.rotation_deg}deg)` };
  return {
    ...base,
    ...(x.position === "top" ? { top: "8%", left: "50%", transform: "translateX(-50%)" }
      : x.position === "center" ? { top: "50%", left: "50%", transform: "translate(-50%,-50%)" }
      : x.position === "bottom" ? { bottom: "8%", left: "50%", transform: "translateX(-50%)" }
      : { top: "72%", left: "7%" }),
  };
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const round = (v: number, d: number) => Math.round(v * 10 ** d) / 10 ** d;
const norm180 = (a: number) => { const r = ((((a + 180) % 360) + 360) % 360) - 180; return r === -180 ? 180 : r; };

/** Pixel geometry of an item in the frame: centre, unrotated size, rotation. */
interface Geom { cx: number; cy: number; w: number; h: number; rot: number }

type Drag = { id: string; kind: "clip" | "title"; mode: "move" | "rotate" | "scale" | "scale-x" | "scale-y"; x0: number; y0: number; tf0: Transform; g: Geom };

/**
 * The Program viewer's live composite and its on-screen controls: click to select,
 * drag to move, corner handles scale, edge handles stretch clips, the round handle rotates.
 * Shift snaps rotation to 15°; position snaps to the centre lines, scale to 100%.
 */
function ProgramCanvas({ project, media, infos, frame, playing, shuttle, grade, sync }: {
  project: Project; media: Map<string, MediaRef>; infos: Map<string, MediaInfo>; frame: number; playing: boolean; shuttle: number;
  grade: { clipId: string; color: ColorParams } | undefined; sync: Sync;
}) {
  const selection = useUI((s) => s.selection);
  const live = useUI((s) => s.liveTransform);
  const set = useUI((s) => s.set);
  const hostRef = useRef<HTMLDivElement>(null);
  const titleEls = useRef(new Map<string, HTMLDivElement>());
  const [drag, setDrag] = useState<Drag | null>(null);
  const [snap, setSnap] = useState({ x: false, y: false });
  const [editing, setEditing] = useState<string | null>(null);
  const [box, setBox] = useState<Geom | null>(null);
  const { width: W, height: H, frame_rate: rate } = project.settings;
  const ar = H / W;
  const items = itemsAt(project, media, frame);
  const tfOf = (it: Item): Transform | null => {
    if (live && live.id === itemId(it)) return live.tf;
    return it.kind === "clip" ? animatedAt(it.clip, frame).tf : it.title.transform ?? null;
  };
  const selId = selection[selection.length - 1];
  const sel = items.find((it) => itemId(it) === selId);

  const geomOf = (it: Item): Geom | null => {
    const host = hostRef.current;
    if (!host) return null;
    const fw = host.clientWidth, fh = host.clientHeight;
    const tf = tfOf(it);
    if (it.kind === "clip") {
      const b = fitBox(it.clip, it.media, W, H);
      const t = tf ?? IDENT;
      return { cx: (0.5 + t.position_x) * fw, cy: (0.5 + t.position_y) * fh, w: b.w * t.scale_x * fw, h: b.h * t.scale_y * fh, rot: t.rotation_deg };
    }
    const el = titleEls.current.get(it.title.id);
    if (!el) return null;
    if (tf) return { cx: (0.5 + tf.position_x) * fw, cy: (0.5 + tf.position_y) * fh, w: el.offsetWidth, h: el.offsetHeight, rot: tf.rotation_deg };
    const r = el.getBoundingClientRect(), hr = host.getBoundingClientRect();
    return { cx: r.left - hr.left + r.width / 2, cy: r.top - hr.top + r.height / 2, w: r.width, h: r.height, rot: 0 };
  };
  // Titles need their laid-out size, so the selection box is measured after render.
  useLayoutEffect(() => {
    const g = sel ? geomOf(sel) : null;
    const same = (a: Geom | null, b: Geom | null) => a === b || (!!a && !!b && Math.abs(a.cx - b.cx) + Math.abs(a.cy - b.cy) + Math.abs(a.w - b.w) + Math.abs(a.h - b.h) + Math.abs(a.rot - b.rot) < 0.5);
    if (!same(g, box)) setBox(g);
  });
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const ro = new ResizeObserver(() => setBox(null));
    ro.observe(host);
    return () => ro.disconnect();
  }, []);

  const hitTest = (clientX: number, clientY: number): Item | undefined => {
    const hr = hostRef.current!.getBoundingClientRect();
    for (const it of [...items].reverse()) {
      const g = geomOf(it);
      if (!g) continue;
      const a = (-g.rot * Math.PI) / 180, dx = clientX - hr.left - g.cx, dy = clientY - hr.top - g.cy;
      const lx = dx * Math.cos(a) - dy * Math.sin(a), ly = dx * Math.sin(a) + dy * Math.cos(a);
      if (Math.abs(lx) <= g.w / 2 && Math.abs(ly) <= g.h / 2) return it;
    }
    return undefined;
  };

  const begin = (e: React.PointerEvent, it: Item, mode: Drag["mode"]) => {
    if (e.button !== 0 || it.track.locked) return;
    const g = geomOf(it);
    const host = hostRef.current;
    if (!g || !host) return;
    // Handles sit on top of the box: stop here so the outer handlers don't restart the drag as a move.
    e.stopPropagation();
    e.preventDefault();
    host.setPointerCapture(e.pointerId);
    set({ activeMonitor: "timeline", selection: [itemId(it)] });
    // A preset-placed title starts free placement exactly where it is now.
    const tf0 = tfOf(it) ?? { ...IDENT, position_x: g.cx / host.clientWidth - 0.5, position_y: g.cy / host.clientHeight - 0.5 };
    setDrag({ id: itemId(it), kind: it.kind, mode, x0: e.clientX, y0: e.clientY, tf0, g });
  };
  const onDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    const it = hitTest(e.clientX, e.clientY);
    set({ selection: it ? [itemId(it)] : [] });
    if (it) begin(e, it, "move");
  };
  const onMove = (e: React.PointerEvent) => {
    if (!drag) return;
    const host = hostRef.current!;
    const hr = host.getBoundingClientRect();
    const fw = host.clientWidth, fh = host.clientHeight;
    const t = { ...drag.tf0 };
    const c = { x: hr.left + drag.g.cx, y: hr.top + drag.g.cy };
    const local = (x: number, y: number) => {
      const a = (-drag.g.rot * Math.PI) / 180, dx = x - c.x, dy = y - c.y;
      return { x: dx * Math.cos(a) - dy * Math.sin(a), y: dx * Math.sin(a) + dy * Math.cos(a) };
    };
    let sx = false, sy = false;
    if (drag.mode === "move") {
      t.position_x = drag.tf0.position_x + (e.clientX - drag.x0) / fw;
      t.position_y = drag.tf0.position_y + (e.clientY - drag.y0) / fh;
      if (Math.abs(t.position_x) * fw < 8) { t.position_x = 0; sx = true; }
      if (Math.abs(t.position_y) * fh < 8) { t.position_y = 0; sy = true; }
    } else if (drag.mode === "rotate") {
      const a0 = Math.atan2(drag.y0 - c.y, drag.x0 - c.x), a = Math.atan2(e.clientY - c.y, e.clientX - c.x);
      let r = drag.tf0.rotation_deg + ((a - a0) * 180) / Math.PI;
      if (e.shiftKey) r = Math.round(r / 15) * 15;
      t.rotation_deg = norm180(r);
    } else {
      const p0 = local(drag.x0, drag.y0), p = local(e.clientX, e.clientY);
      const ratio = (n: number, d: number) => (Math.abs(d) < 1 ? 1 : Math.abs(n) / Math.abs(d));
      if (drag.mode === "scale") {
        const k = Math.hypot(p.x, p.y) / Math.max(1, Math.hypot(p0.x, p0.y));
        t.scale_x = drag.tf0.scale_x * k;
        t.scale_y = drag.tf0.scale_y * k;
        if (drag.tf0.scale_x === drag.tf0.scale_y && Math.abs(t.scale_x - 1) < 0.03) t.scale_x = t.scale_y = 1;
      } else if (drag.mode === "scale-x") t.scale_x = drag.tf0.scale_x * ratio(p.x, p0.x);
      else t.scale_y = drag.tf0.scale_y * ratio(p.y, p0.y);
    }
    const tf: Transform = {
      scale_x: round(clamp(t.scale_x, 0.05, 8), 3), scale_y: round(clamp(t.scale_y, 0.05, 8), 3),
      position_x: round(clamp(t.position_x, -2, 2), 4), position_y: round(clamp(t.position_y, -2, 2), 4), rotation_deg: round(t.rotation_deg, 1),
    };
    if (drag.kind === "title") tf.scale_y = tf.scale_x;
    setSnap({ x: sx, y: sy });
    set({ liveTransform: { id: drag.id, tf } });
  };
  const onUp = () => {
    if (!drag) return;
    const tf = useUI.getState().liveTransform?.tf;
    setDrag(null);
    setSnap({ x: false, y: false });
    if (tf && JSON.stringify(tf) !== JSON.stringify(drag.tf0)) {
      const it = items.find((x) => itemId(x) === drag.id);
      const req = it?.kind === "clip" ? api.updateClip(drag.id, layerPatch(it.clip, frame, { transform: tf })) : api.updateTitle(drag.id, { transform: tf });
      void run(req).finally(() => set({ liveTransform: null }));
    } else set({ liveTransform: null });
  };
  const onDouble = (e: React.MouseEvent) => {
    const it = hitTest(e.clientX, e.clientY);
    if (it?.kind === "title" && !it.track.locked) setEditing(it.title.id);
  };

  const editTitle = editing ? items.find((it): it is TitleItem => it.kind === "title" && it.title.id === editing) : undefined;
  return (
    <div className="canvas" ref={hostRef} onPointerDown={onDown} onPointerMove={onMove} onPointerUp={onUp} onDoubleClick={onDouble}>
      {items.map((it) =>
        it.kind === "clip" ? (
          <LayerView key={`${it.track.id}:${it.slot}`} it={it} tf={tfOf(it) ?? IDENT} info={infos.get(it.media.id)} frame={frame} playing={playing} shuttle={shuttle} rate={rate} W={W} H={H}
            grade={grade && grade.clipId === it.clip.id ? grade.color : it.clip.color} sync={sync} />
        ) : (
          <div key={it.title.id} className="ttl" ref={(el) => { if (el) titleEls.current.set(it.title.id, el); else titleEls.current.delete(it.title.id); }}
            style={{ ...titleStyle(it.title, frame, W, ar, tfOf(it)), visibility: editing === it.title.id ? "hidden" : undefined }}>{it.title.text}</div>
        ),
      )}
      {snap.x && <div className="guide v" />}
      {snap.y && <div className="guide h" />}
      {sel && box && !editTitle && (
        <div className={`gizmo ${sel.track.locked ? "locked" : ""} ${box.cy - box.h / 2 < 24 ? "rot-below" : ""}`} style={{ left: box.cx - box.w / 2, top: box.cy - box.h / 2, width: box.w, height: box.h, transform: `rotate(${box.rot}deg)` }}
          onPointerDown={(e) => begin(e, sel, "move")} title={sel.track.locked ? `${sel.track.name} is locked` : sel.kind === "title" ? "Drag to move · double-click to edit the text" : "Drag to move"}>
          {!sel.track.locked && (
            <>
              {(["nw", "ne", "se", "sw"] as const).map((k) => <span key={k} className={`h ${k}`} onPointerDown={(e) => begin(e, sel, "scale")} />)}
              {sel.kind === "clip" && ([["n", "scale-y"], ["s", "scale-y"], ["e", "scale-x"], ["w", "scale-x"]] as const).map(([k, m]) => <span key={k} className={`h ${k}`} onPointerDown={(e) => begin(e, sel, m)} />)}
              <span className="rot" title="Rotate (Shift: 15° steps)" onPointerDown={(e) => begin(e, sel, "rotate")} />
            </>
          )}
        </div>
      )}
      {editTitle && box && (
        <textarea
          className="ttl-edit" autoFocus defaultValue={editTitle.title.text} aria-label="Title text"
          style={{ ...titleStyle(editTitle.title, frame, W, ar, null), left: box.cx - box.w / 2, top: box.cy - box.h / 2, minWidth: box.w, minHeight: box.h, transform: undefined, opacity: 1, bottom: undefined }}
          onPointerDown={(e) => e.stopPropagation()}
          onKeyDown={(e) => { if (e.key === "Escape") setEditing(null); if (e.key === "Enter" && e.ctrlKey) (e.target as HTMLTextAreaElement).blur(); }}
          onBlur={(e) => { const v = e.target.value; setEditing(null); if (v.trim() && v !== editTitle.title.text) run(api.updateTitle(editTitle.title.id, { text: v })); }}
        />
      )}
    </div>
  );
}

function Transport(props: { frame: number; total: number; playing: boolean; rate: Project["settings"]["frame_rate"]; onSeek: (f: number) => void; onToggle: () => void; extra?: React.ReactNode }) {
  const { frame, total, rate, onSeek } = props;
  const step = Math.round(rate.num / rate.den);
  return (
    <div className="transport">
      <span className="tc num">{timecode(frame, rate)}</span>
      <span className="spacer" />
      <button className="btn icon ghost opt" title="Go to start (Home)" onClick={() => onSeek(0)}><ChevronFirst /></button>
      <button className="btn icon ghost opt" title="Back 1 second (Shift+Left)" onClick={() => onSeek(Math.max(0, frame - step))}><StepBack /></button>
      <button className="btn icon ghost" title="Previous frame (Left)" onClick={() => onSeek(Math.max(0, frame - 1))}><ChevronLeft /></button>
      <button className="btn icon" title="Play / pause (Space). Shuttle: J reverse, K stop, L forward; press J or L again for 2× and 4×" onClick={() => { resumeAudio(); props.onToggle(); }} aria-pressed={props.playing}>{props.playing ? <Pause /> : <Play />}</button>
      <button className="btn icon ghost" title="Next frame (Right)" onClick={() => onSeek(Math.min(total, frame + 1))}><ChevronRight /></button>
      <button className="btn icon ghost opt" title="Forward 1 second (Shift+Right)" onClick={() => onSeek(Math.min(total, frame + step))}><StepForward /></button>
      <button className="btn icon ghost opt" title="Go to end (End)" onClick={() => onSeek(total)}><ChevronLast /></button>
      <span className="spacer" />
      {props.extra}
      <span className="tc dim num opt" title="Duration">{timecode(total, rate)}</span>
    </div>
  );
}

function Scrub({ frame, total, onSeek, markIn, markOut }: { frame: number; total: number; onSeek: (f: number) => void; markIn?: number | null; markOut?: number | null }) {
  const ref = useRef<HTMLDivElement>(null);
  const at = (x: number) => {
    const r = ref.current!.getBoundingClientRect();
    return Math.round(Math.max(0, Math.min(1, (x - r.left) / r.width)) * total);
  };
  const pct = (f: number) => `${total ? (f / total) * 100 : 0}%`;
  return (
    <div
      className="scrub" ref={ref} role="slider" aria-label="Scrub" aria-valuemin={0} aria-valuemax={total} aria-valuenow={frame} tabIndex={-1}
      onPointerDown={(e) => {
        (e.target as HTMLElement).setPointerCapture(e.pointerId);
        onSeek(at(e.clientX));
        const el = e.currentTarget;
        const mv = (ev: PointerEvent) => onSeek(at(ev.clientX));
        const up = () => { el.removeEventListener("pointermove", mv); el.removeEventListener("pointerup", up); };
        el.addEventListener("pointermove", mv);
        el.addEventListener("pointerup", up);
      }}
    >
      <div className="fill" style={{ width: pct(frame) }} />
      {(markIn != null || markOut != null) && <div className="mark-range" style={{ left: pct(markIn ?? 0), width: `calc(${pct((markOut ?? total) - (markIn ?? 0))})` }} />}
      <div className="head" style={{ left: pct(frame) }} />
    </div>
  );
}

export interface MonitorChrome {
  onSwap?: () => void;
  onToggleDual?: () => void;
  dual?: boolean;
  onFloat?: () => void;
  floating?: boolean;
  /** Close this viewer (the source viewer); it comes back when media is opened in it. */
  onClose?: () => void;
}

function MonitorButtons({ c }: { c?: MonitorChrome }) {
  if (!c) return null;
  return (
    <span className="mon-tools">
      {c.onSwap && <button className="btn icon small ghost" title="Swap source and timeline viewers" onClick={c.onSwap}><ArrowLeftRight /></button>}
      {c.onToggleDual && <button className="btn icon small ghost" title={c.dual ? "Single viewer (close the source viewer)" : "Show the source viewer"} onClick={c.onToggleDual}>{c.dual ? <RectangleHorizontal /> : <Columns2 />}</button>}
      {c.onFloat && <button className="btn icon small ghost" title={c.floating ? "Dock viewer" : "Float viewer in its own window"} onClick={c.onFloat}><ExternalLink /></button>}
    </span>
  );
}

// ------------------------------------------------------------- source monitor

export function SourceMonitor({ chrome }: { chrome?: MonitorChrome }) {
  const view = useUI((s) => s.view);
  const id = useUI((s) => s.sourceMediaId);
  const markIn = useUI((s) => s.markIn);
  const markOut = useUI((s) => s.markOut);
  const set = useUI((s) => s.set);
  const playhead = useUI((s) => s.playhead);
  const m = view?.project.media.find((x) => x.id === id) ?? null;
  const info = id ? infoMap(view).get(id) : undefined;
  const rate = view?.project.settings.frame_rate ?? { num: 25, den: 1 };
  const total = m?.duration_frames ?? 0;
  const vref = useRef<HTMLVideoElement>(null);
  const [frame, setFrame] = useState(0);
  const [playing, setPlaying] = useState(false);

  useEffect(() => { setFrame(0); setPlaying(false); }, [id]);
  useEffect(() => {
    const v = vref.current;
    if (!v) return;
    const tick = () => setFrame(secondsToFrames(v.currentTime, rate));
    const onEnd = () => setPlaying(false);
    v.addEventListener("timeupdate", tick);
    v.addEventListener("ended", onEnd);
    return () => { v.removeEventListener("timeupdate", tick); v.removeEventListener("ended", onEnd); };
  }, [info?.preview, rate]);

  // Source-viewer commands arrive as events from the global key handler and menus.
  useEffect(() => {
    const h = (e: Event) => {
      const k = (e as CustomEvent<string>).detail;
      if (k === "in") set({ markIn: frame });
      if (k === "out") set({ markOut: Math.max(frame, (markIn ?? 0) + 1) });
      if (k === "insert") edit("insert");
      if (k === "overwrite" || k === "place") edit("overwrite");
    };
    window.addEventListener("pirosman-source", h);
    return () => window.removeEventListener("pirosman-source", h);
  });

  const seek = (f: number) => {
    setFrame(f);
    if (vref.current) vref.current.currentTime = framesToSeconds(f, rate);
  };
  const toggle = () => {
    const v = vref.current;
    if (!v) return;
    if (v.paused) { void v.play(); setPlaying(true); } else { v.pause(); setPlaying(false); }
  };

  function edit(kind: "insert" | "overwrite") {
    if (!view) return;
    if (!m) { useUI.getState().toast("error", "Select media in the pool first, then Insert (F9) or Overwrite (F10)."); return; }
    const k = m.has_video ? "video" : "audio";
    const track = view.project.tracks.find((t) => t.kind === k && !t.locked);
    if (!track) { useUI.getState().toast("error", `No unlocked ${k} track.`); return; }
    const req = { media_id: m.id, track_id: track.id, start: playhead, source_in: markIn ?? 0, source_out: m.duration_frames == null ? null : markOut ?? m.duration_frames };
    run(kind === "insert" ? api.insertEdit(req) : api.overwriteEdit(req));
  }

  const aspect = m?.width && m?.height ? `${m.width}/${m.height}` : "16/9";
  return (
    <div className="panel monitor" aria-label="Source monitor" onPointerDown={() => set({ activeMonitor: "source" })}>
      <div className="panel-head">
        <span className="label" style={{ overflow: "hidden", textOverflow: "ellipsis" }}>Source{m && <> · <b>{m.name}</b></>}</span>
        <span className="spacer" />
        <MonitorButtons c={chrome} />
        <button className={`btn small ${markIn != null ? "on" : "ghost"}`} disabled={!m || m.kind === "image"} onClick={() => set({ markIn: frame })} title={markIn != null ? `In at ${timecode(markIn, rate)} (I)` : "Mark in (I)"}>In</button>
        <button className={`btn small ${markOut != null ? "on" : "ghost"}`} disabled={!m || m.kind === "image"} onClick={() => set({ markOut: Math.max(frame, (markIn ?? 0) + 1) })} title={markOut != null ? `Out at ${timecode(markOut, rate)} (O)` : "Mark out (O)"}>Out</button>
        <button className="btn small" disabled={!m} onClick={() => edit("insert")} title="Insert at the playhead, rippling later clips right (F9)"><CornerDownRight /><span className="lbl">Insert</span></button>
        <button className="btn small primary" disabled={!m} onClick={() => edit("overwrite")} title="Overwrite at the playhead (F10)"><Replace /><span className="lbl">Overwrite</span></button>
        {chrome?.onClose && <button className="btn icon small ghost mon-close" title="Close the source viewer (opening media in it brings it back)" aria-label="Close source viewer" onClick={chrome.onClose}><X /></button>}
      </div>
      <div className="monitor-stage">
        <div className="frame" style={{ ["--aspect" as string]: aspect, ["--ar" as string]: ratio(aspect) }}>
          {!m && <div className="black">Select media to preview</div>}
          {m && info?.status === "missing" && <div className="black" style={{ color: "var(--err)" }}>Media offline — relink it on the Media page</div>}
          {m && info?.status !== "missing" && m.kind === "image" && <img className="still" src={fileSrc(m.path)} alt={m.name} />}
          {m && m.kind !== "image" && info?.preview && <video ref={vref} src={fileSrc(info.preview)} preload="auto" />}
          {m && m.kind !== "image" && !info?.preview && info?.status !== "missing" && <div className="black"><Loader2 className="spin" size={18} /> Building preview proxy…</div>}
          {m && m.kind === "audio" && info?.wave && <img className="still" src={fileSrc(info.wave)} alt="" style={{ objectFit: "fill", opacity: 0.8, padding: "30% 6%" }} />}
        </div>
      </div>
      {m && m.kind !== "image" && <Scrub frame={frame} total={total} onSeek={seek} markIn={markIn} markOut={markOut} />}
      <Transport frame={frame} total={total} playing={playing} rate={rate} onSeek={seek} onToggle={toggle} />
    </div>
  );
}

// ------------------------------------------------------------- timeline monitor

export function TimelineMonitor({ title = "Program", gradeOverride, chrome }: { title?: string; gradeOverride?: { clipId: string; color: ColorParams }; chrome?: MonitorChrome }) {
  const view = useUI((s) => s.view);
  const playhead = useUI((s) => s.playhead);
  const playing = useUI((s) => s.playing);
  const shuttle = useUI((s) => s.shuttle);
  const previewFile = useUI((s) => s.previewFile);
  const render = useUI((s) => s.render);
  const set = useUI((s) => s.set);
  const [mode, setMode] = useState<"live" | "rendered">("live");
  const vref = useRef<HTMLVideoElement>(null);
  const project = view?.project;
  const rate = project?.settings.frame_rate ?? { num: 25, den: 1 };
  const fps = rate.num / rate.den;
  const media = useMemo(() => mediaMap(view), [view]);
  const infos = useMemo(() => infoMap(view), [view]);
  const total = projectDuration(view);

  const layers = project ? itemsAt(project, media, playhead) : [];
  const vhit = layers.filter((it): it is ClipItem => it.kind === "clip").pop();
  const vinfo = vhit ? infos.get(vhit.media.id) : undefined;
  const rendered = mode === "rendered" && previewFile;

  // OpenFX cannot be approximated in the browser: while paused, show the exact frame
  // rendered through the plugins (debounced, latest request wins).
  const hasOfx = !!project?.tracks.some((t) => t.kind === "video" && !t.muted && t.clips.some((c) => c.start <= playhead && playhead < clipEnd(c) && (c.ofx ?? []).some((f) => f.enabled)));
  const [ofxFrame, setOfxFrame] = useState<{ frame: number; url: string; view: unknown } | null>(null);
  const [ofxState, setOfxState] = useState<"idle" | "busy" | "error">("idle");
  const [ofxError, setOfxError] = useState("");
  useEffect(() => {
    if (playing || rendered || !hasOfx) { setOfxState("idle"); return; }
    let stale = false;
    const t = setTimeout(() => {
      setOfxState("busy");
      api.ofxPreviewFrame(playhead)
        .then((url) => { if (!stale) { setOfxFrame(url ? { frame: playhead, url, view } : null); setOfxState("idle"); } })
        .catch((e) => { if (!stale) { setOfxState("error"); setOfxError(String(e)); } });
    }, 250);
    return () => { stale = true; clearTimeout(t); };
  }, [playhead, playing, rendered, hasOfx, view]);
  const showOfx = !rendered && !playing && hasOfx && ofxFrame && ofxFrame.frame === playhead && ofxFrame.view === view;

  // Clock: wall-time driven so gaps, stills and retimed clips advance correctly.
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const t0 = performance.now();
    const ph = useUI.getState().playhead;
    const f0 = shuttle > 0 && ph >= total ? 0 : ph;
    const tick = () => {
      const f = f0 + Math.trunc(((performance.now() - t0) / 1000) * fps * shuttle);
      if (f >= total) { set({ playing: false, playhead: total, shuttle: 1 }); return; }
      if (f <= 0 && shuttle < 0) { set({ playing: false, playhead: 0, shuttle: 1 }); return; }
      set({ playhead: f });
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, total, fps, set, shuttle]);

  const syncEl = (el: HTMLMediaElement, src: string | null, seconds: number, rateMul: number, reversed: boolean) => {
    if (!src) { if (!el.paused) el.pause(); return; }
    const url = fileSrc(src);
    if (el.dataset.src !== url) { el.dataset.src = url; el.src = url; el.currentTime = seconds; }
    el.playbackRate = Math.max(0.25, Math.min(4, rateMul));
    if (playing && !reversed) {
      if (Math.abs(el.currentTime - seconds) > 0.2 * Math.max(1, rateMul)) el.currentTime = seconds;
      if (el.paused) el.play().catch(() => {});
    } else {
      if (!el.paused) el.pause();
      if (Math.abs(el.currentTime - seconds) > 0.5 / fps) el.currentTime = seconds;
    }
  };

  // Picture.
  useEffect(() => {
    const el = vref.current;
    if (!el) return;
    el.muted = !rendered;
    syncEl(el, rendered ? previewFile : null, framesToSeconds(playhead, rate), 1, false);
  }, [playhead, playing, rendered, previewFile]);

  // Sound: one strip per audio track, mixed live through Web Audio with fader, pan, EQ and fades.
  useEffect(() => {
    if (!project) return;
    const audio = project.tracks.filter((t) => t.kind === "audio");
    dropStripsExcept(new Set(audio.map((t) => t.id)));
    const soloing = audio.some((t) => t.solo && !t.muted);
    for (const t of audio) {
      const s = strip(t.id);
      const c = !rendered && !t.muted && (!soloing || t.solo) ? t.clips.find((c) => c.start <= playhead && playhead < clipEnd(c)) : undefined;
      const info = c ? infos.get(c.media_id) : undefined;
      setStrip(t.id, {
        gainLinear: c ? dbToLinear(c.gain_db + (t.volume_db ?? 0)) * fadeOpacity(t, c, playhead) : 0,
        pan: t.pan ?? 0,
        low: t.eq_low_db ?? 0,
        mid: t.eq_mid_db ?? 0,
        high: t.eq_high_db ?? 0,
      });
      // Audio is silent while shuttling backwards (elements cannot play in reverse).
      syncEl(s.el, c && info?.preview && shuttle > 0 ? info.preview : null, c ? framesToSeconds(sourceFrameAt(c, playhead), rate) : 0, c ? speedOf(c) * shuttle : 1, !!c?.reversed);
    }
  }, [playhead, playing, project, rendered, infos, shuttle]);

  useEffect(() => () => { dropStripsExcept(new Set()); }, []);

  const aspect = project ? `${project.settings.width}/${project.settings.height}` : "16/9";
  const previewBusy = render?.job === "preview" && !render.result && !render.error;

  const makePreview = async () => {
    const plan = await attempt(api.startRender(null, true));
    if (plan) set({ render: { job: "preview", plan, progress: null, result: null, error: null } });
  };

  return (
    <div className="panel monitor" aria-label="Timeline monitor" onPointerDown={() => set({ activeMonitor: "timeline" })}>
      <div className="panel-head">
        <span className="label">{title} · <b>{project?.name}</b></span>
        <span className="spacer" />
        <MonitorButtons c={chrome} />
        <button className="btn icon small ghost" title="Grab still: save this graded frame as PNG" onClick={() => grabStillFlow(playhead)} disabled={total === 0}><Camera /></button>
        <div className="seg" role="group" aria-label="Monitor source">
          <button aria-pressed={!rendered} onClick={() => setMode("live")} title="Live: top layer, approximate grade, effects and mix">Live</button>
          <button aria-pressed={!!rendered} disabled={!previewFile} onClick={() => setMode("rendered")} title={previewFile ? "Exact FFmpeg render of the timeline" : "Render a preview first"}>Rendered</button>
        </div>
        <button className="btn small" onClick={makePreview} disabled={previewBusy || total === 0} title="Render the whole timeline at 640px through the export pipeline">
          {previewBusy ? <span title={render?.progress?.stage}><Loader2 className="spin" />{render?.progress?.stage ? "OFX " : ""}{Math.round((render?.progress?.fraction ?? 0) * 100)}%</span> : <><Clapperboard /><span className="lbl">Render preview</span></>}
        </button>
      </div>
      <div className="monitor-stage">
        <div className="frame" style={{ ["--aspect" as string]: aspect, ["--ar" as string]: ratio(aspect) }}>
          <video ref={vref} preload="auto" crossOrigin="anonymous" style={{ visibility: rendered ? "visible" : "hidden" }} />
          {!rendered && project && <ProgramCanvas project={project} media={media} infos={infos} frame={playhead} playing={playing} shuttle={shuttle} grade={gradeOverride} sync={syncEl} />}
          {!rendered && layers.length === 0 && <div className="black">{total === 0 ? "Place media on the timeline" : "Gap"}</div>}
          {!rendered && vhit && vhit.media.kind !== "image" && !vinfo?.preview && <div className="black">{vinfo?.status === "missing" ? "Media offline" : "Building preview proxy…"}</div>}
          {showOfx && <img className="still ofx-frame" src={ofxFrame!.url} alt="OpenFX frame" />}
          {rendered && <span className="overlay-note">Rendered preview</span>}
          {!rendered && hasOfx && (
            <span className={`overlay-note ofx-note ${ofxState === "error" ? "err" : ""}`} title={ofxState === "error" ? ofxError : undefined}>
              {ofxState === "busy" ? <Loader2 size={11} className="spin" /> : <Puzzle size={11} />}
              {playing ? "OpenFX: pause to see the effect" : ofxState === "busy" ? "Rendering OpenFX…" : ofxState === "error" ? `OpenFX failed: ${ofxError.slice(0, 120)}` : "OpenFX frame"}
            </span>
          )}
          {project && (
            <div className="mon-hud num" aria-label="Viewer status">
              <span>{project.settings.width}×{project.settings.height}</span>
              <span>{fpsLabel(rate.num, rate.den)} fps</span>
              <span>Fit</span>
              {playing && shuttle !== 1 && <span className="shuttle">{shuttle < 0 ? <Rewind size={11} /> : <FastForward size={11} />}{Math.abs(shuttle)}×</span>}
            </div>
          )}
        </div>
      </div>
      <Scrub frame={playhead} total={total} onSeek={(f) => set({ playhead: f })} markIn={project?.mark_in} markOut={project?.mark_out} />
      <Transport frame={playhead} total={total} playing={playing} rate={rate} onSeek={(f) => set({ playhead: f, playing: false })} onToggle={() => set({ playing: !playing, shuttle: 1 })} extra={vhit && <span className="note num">{clipDur(vhit.clip)}f</span>} />
    </div>
  );
}

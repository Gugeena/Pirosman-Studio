import { useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowDownToLine, AudioLines, ChevronsLeftRight, Columns2, Eye, EyeOff, Film, Headphones, Link2, Lock, LockOpen, Magnet, MousePointer2, MoveHorizontal,
  Bookmark, Maximize2, Plus, Redo2, Scissors, SquareSplitHorizontal, Trash2, Type, Undo2, Volume2, VolumeX, ZoomIn, ZoomOut,
} from "lucide-react";
import { cssUrl } from "../lib/files";
import { api, type Clip, type Marker, type MarkerColor, type MediaInfo, type MediaRef, type TitleClip, type Track, type TransitionKind } from "../lib/api";
import { clipDur, clipEnd, fmtSpeed, sourceFrameAt, speedOf } from "../lib/clip";
import { infoMap, mediaMap, projectDuration, promptText, run, useUI } from "../lib/store";
import { timecode } from "../lib/time";
import { GreaseMark } from "./GreaseMark";
import { showContextMenu, type CtxItem } from "../lib/contextMenu";
import { usePeersOn, type Peer } from "../lib/collab";
import { hitAt, type DropDetail } from "../lib/dnd";
import { applyEffect } from "../lib/ofx";

export type Tool = "select" | "blade" | "slip" | "slide" | "roll";
type DragKind = "move" | "trim-l" | "trim-r" | "slip" | "slide" | "roll" | "fade-in" | "fade-out";

/** A clip or a title, as the timeline lays it out. */
interface Item {
  id: string;
  title: TitleClip | null;
  clip: Clip | null;
  start: number;
  dur: number;
  fadeIn: number;
  fadeOut: number;
}

interface Drag {
  kind: DragKind;
  item: Item;
  track: Track;
  linked: string[];
  x0: number;
  y0: number;
  dx: number; // timeline frames
  targetTrack: string;
  moved: boolean;
  next: Item | null; // roll / slide neighbour after
  prev: Item | null; // slide neighbour before
}

/** Track row height: tall enough for the filmstrip frames to read. */
const TRACK_H = 58;
const SNAP_PX = 8;
/** Left gutter so change marks around frame-0 clips are not clipped. */
const GUTTER = 16;

const CHANGE_NOTE: Record<string, string> = {
  ClipAdded: "new",
  ClipTrackChanged: "moved",
  ClipMoved: "moved",
  ClipTrimmed: "trimmed",
  ClipSlipped: "slipped",
  ColorOrEffectModified: "graded",
  LutModified: "LUT",
  AudioGainModified: "gain",
  ClipNameChanged: "renamed",
  ClipMediaChanged: "relinked",
  ClipFadeModified: "faded",
  TransitionModified: "transition",
  ClipSpeedModified: "retimed",
  TitleAdded: "new",
  TitleMoved: "moved",
  TitleModified: "edited",
};

const MARKER_COLORS: MarkerColor[] = ["yellow", "red", "blue", "green", "purple"];
const markerVar = (c: string) => `var(--mk-${c})`;

/**
 * Frames along a video clip, so every cut shows what is on either side of it. Each tile
 * shows the source frame at its left edge; the last tile is pinned to the clip's right
 * edge and shows the clip's final frame. Only tiles inside the visible scroll range render.
 */
function Filmstrip({ clip, mediaRef, info, x, w, h, ppf, viewLeft, viewRight }: {
  clip: Clip; mediaRef: MediaRef | undefined; info: MediaInfo | undefined; x: number; w: number; h: number; ppf: number; viewLeft: number; viewRight: number;
}) {
  const n = info?.strip_frames ?? 0;
  const total = mediaRef?.duration_frames ?? 0;
  if (!info?.strip || n <= 0 || total <= 0) {
    return info?.thumb ? <div className="cthumbs" style={{ backgroundImage: cssUrl(info.thumb) }} /> : null;
  }
  const aspect = mediaRef?.width && mediaRef?.height ? mediaRef.width / mediaRef.height : 16 / 9;
  const tw = Math.max(16, Math.round(h * aspect));
  const url = cssUrl(info.strip);
  const size = `${n * tw}px ${h}px`;
  const end = clipEnd(clip);
  const tileAt = (timelineFrame: number) => {
    const src = Math.min(clip.source_out - 1, Math.max(clip.source_in, sourceFrameAt(clip, timelineFrame)));
    return Math.min(n - 1, Math.max(0, Math.floor((src / total) * n)));
  };
  const first = Math.max(0, Math.floor((viewLeft - x) / tw) - 1);
  const last = Math.min(Math.ceil(w / tw), Math.ceil((viewRight - x) / tw) + 1);
  const tiles: JSX.Element[] = [];
  const pinTail = w >= tw * 1.6;
  for (let j = first; j < last; j++) {
    const left = j * tw;
    if (pinTail && left >= w - tw) break; // from here the pinned tail tile (drawn on top) covers the end
    tiles.push(<div key={j} className="fs-tile" style={{ left, width: tw, backgroundImage: url, backgroundSize: size, backgroundPosition: `${-tileAt(clip.start + Math.floor(left / ppf)) * tw}px 0` }} />);
  }
  if (pinTail && x + w >= viewLeft && x + w - tw <= viewRight) {
    tiles.push(<div key="tail" className="fs-tile tail" style={{ left: w - tw, width: tw, backgroundImage: url, backgroundSize: size, backgroundPosition: `${-tileAt(end - 1) * tw}px 0` }} />);
  }
  return <div className="filmstrip" aria-hidden="true">{tiles}</div>;
}

function itemsOf(t: Track): Item[] {
  const clips: Item[] = t.clips.map((c) => ({ id: c.id, clip: c, title: null, start: c.start, dur: clipDur(c), fadeIn: c.fade_in_frames ?? 0, fadeOut: c.fade_out_frames ?? 0 }));
  const titles: Item[] = (t.titles ?? []).map((x) => ({ id: x.id, clip: null, title: x, start: x.start, dur: x.duration, fadeIn: x.fade_in_frames ?? 0, fadeOut: x.fade_out_frames ?? 0 }));
  return [...clips, ...titles].sort((a, b) => a.start - b.start);
}

export function Timeline({ compact = false }: { compact?: boolean }) {
  const view = useUI((s) => s.view);
  const playhead = useUI((s) => s.playhead);
  const ppf = useUI((s) => s.pxPerFrame);
  const selection = useUI((s) => s.selection);
  const set = useUI((s) => s.set);
  const page = useUI((s) => s.page);
  const peers = usePeersOn(page);
  const presence = useMemo(() => {
    const m = new Map<string, Peer>();
    for (const p of peers) for (const id of p.selection ?? []) m.set(id, p);
    return m;
  }, [peers]);
  const [tool, setTool] = useState<Tool>("select");
  const [snap, setSnap] = useState(true);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [snapAt, setSnapAt] = useState<number | null>(null);
  const [scroll, setScroll] = useState({ left: 0, top: 0, width: 800 });
  const dnd = useUI((s) => s.drag);
  const [renaming, setRenaming] = useState<string | null>(null);
  const lanesRef = useRef<HTMLDivElement>(null);

  const project = view?.project;
  const rate = project?.settings.frame_rate ?? { num: 25, den: 1 };
  const fpsN = rate.num / rate.den;
  const media = useMemo(() => mediaMap(view), [view]);
  const infos = useMemo(() => infoMap(view), [view]);
  const duration = projectDuration(view);

  // Display order: video tracks top-down from the highest, then audio.
  const display = useMemo(() => {
    const t = project?.tracks ?? [];
    return [...t.filter((x) => x.kind === "video").reverse(), ...t.filter((x) => x.kind === "audio")];
  }, [project]);

  const changes = useMemo(() => {
    const m = new Map<string, string[]>();
    for (const c of view?.vcs?.head ? view.vcs.changes : []) {
      if (!c.clip_id) continue;
      const note = CHANGE_NOTE[c.op];
      if (!note) continue;
      const arr = m.get(c.clip_id) ?? [];
      if (!arr.includes(note)) arr.push(note);
      m.set(c.clip_id, arr);
    }
    return m;
  }, [view]);

  const totalFrames = Math.max(duration + Math.round(fpsN * 20), Math.ceil(scroll.width / ppf));
  const width = GUTTER + totalFrames * ppf;

  useEffect(() => {
    const el = lanesRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setScroll((s) => ({ ...s, width: el.clientWidth })));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Tool shortcuts: A select, B blade, Y slip, U slide, R roll, N snapping.
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      if (e.ctrlKey || e.metaKey || e.altKey || t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable || useUI.getState().dialog) return;
      const k = e.key.toLowerCase();
      if (k === "b") setTool((x) => (x === "blade" ? "select" : "blade"));
      else if (k === "a" || k === "v") setTool("select");
      else if (k === "y") setTool((x) => (x === "slip" ? "select" : "slip"));
      else if (k === "u") setTool((x) => (x === "slide" ? "select" : "slide"));
      else if (k === "r") setTool((x) => (x === "roll" ? "select" : "roll"));
      else if (k === "n") setSnap((x) => !x);
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, []);

  // Keep the playhead in view while playing.
  useEffect(() => {
    const el = lanesRef.current;
    if (!el || drag) return;
    const x = GUTTER + playhead * ppf;
    if (x < el.scrollLeft || x > el.scrollLeft + el.clientWidth - 40) el.scrollLeft = Math.max(0, x - el.clientWidth * 0.25);
  }, [playhead, ppf, drag]);

  // Drops from the media pool (overwrite at the pointer) and the effects library (onto a clip).
  useEffect(() => {
    const h = (e: Event) => {
      const { payload, x, y } = (e as CustomEvent<DropDetail>).detail;
      const el = lanesRef.current;
      const s = useUI.getState();
      if (!el || !s.view) return;
      const r = el.getBoundingClientRect();
      if (x < r.left || x > r.right || y < r.top || y > r.bottom) return;
      if (payload.kind === "media") {
        const tracks = s.view.project.tracks;
        const order = [...tracks.filter((t) => t.kind === "video").reverse(), ...tracks.filter((t) => t.kind === "audio")];
        const t = order[Math.floor((y - r.top + el.scrollTop) / TRACK_H)];
        if (!t) return;
        if (t.locked) { s.toast("info", `${t.name} is locked.`); return; }
        const f = Math.max(0, Math.round((x - r.left + el.scrollLeft - GUTTER) / s.pxPerFrame));
        run(api.overwriteEdit({ media_id: payload.id, track_id: t.id, start: f, source_in: null, source_out: null }));
      } else {
        const id = hitAt(x, y)?.closest("[data-clip]")?.getAttribute("data-clip");
        if (id) void applyEffect(payload, [id]);
        else s.toast("info", "Drop the effect onto a video clip.");
      }
    };
    window.addEventListener("pirosman-drop", h);
    return () => window.removeEventListener("pirosman-drop", h);
  }, []);

  if (!project) return null;

  const frameAtClientX = (clientX: number) => {
    const el = lanesRef.current!;
    const r = el.getBoundingClientRect();
    return Math.max(0, Math.round((clientX - r.left + el.scrollLeft - GUTTER) / ppf));
  };
  const trackAtClientY = (clientY: number): Track | undefined => {
    const el = lanesRef.current!;
    const r = el.getBoundingClientRect();
    return display[Math.floor((clientY - r.top + el.scrollTop) / TRACK_H)];
  };

  // Where a pointer drag from the pool or the effects library would land.
  let hoverTrack: string | null = null, hoverRow = -1, hoverFrame = 0, hoverClip: string | null = null;
  if (dnd && lanesRef.current) {
    const r = lanesRef.current.getBoundingClientRect();
    if (dnd.x >= r.left && dnd.x <= r.right && dnd.y >= r.top && dnd.y <= r.bottom) {
      if (dnd.payload.kind === "media") {
        const t = trackAtClientY(dnd.y);
        if (t && !t.locked) { hoverTrack = t.id; hoverRow = display.indexOf(t); hoverFrame = frameAtClientX(dnd.x); }
      } else {
        hoverClip = hitAt(dnd.x, dnd.y)?.closest("[data-clip]")?.getAttribute("data-clip") ?? null;
      }
    }
  }

  const snapPoints = (exclude: Set<string>) => {
    const pts = [0, playhead, ...(project.markers ?? []).map((m) => m.frame)];
    if (project.mark_in != null) pts.push(project.mark_in);
    if (project.mark_out != null) pts.push(project.mark_out);
    for (const t of project.tracks) for (const it of itemsOf(t)) if (!exclude.has(it.id)) pts.push(it.start, it.start + it.dur);
    return pts;
  };
  const snapFrame = (f: number, pts: number[]): [number, number | null] => {
    let best: number | null = null;
    for (const p of pts) if (Math.abs(p - f) * ppf <= SNAP_PX && (best === null || Math.abs(p - f) < Math.abs(best - f))) best = p;
    return best === null ? [f, null] : [best, best];
  };

  const neighbours = (t: Track, it: Item) => {
    const all = itemsOf(t);
    return {
      prev: all.find((o) => o.id !== it.id && o.start + o.dur === it.start) ?? null,
      next: all.find((o) => o.id !== it.id && o.start === it.start + it.dur) ?? null,
    };
  };

  const onItemDown = (e: React.PointerEvent, track: Track, it: Item, edge: "body" | "l" | "r" | "fade-in" | "fade-out") => {
    if (e.button !== 0) return;
    e.stopPropagation();
    if (tool === "blade" && edge === "body") {
      run(api.splitClip(it.id, frameAtClientX(e.clientX)));
      return;
    }
    const sel = e.shiftKey || e.ctrlKey ? (selection.includes(it.id) ? selection.filter((s) => s !== it.id) : [...selection, it.id]) : selection.includes(it.id) ? selection : [it.id];
    set({ selection: sel, activeMonitor: "timeline" });
    if (track.locked) return;
    let kind: DragKind;
    if (edge === "fade-in" || edge === "fade-out") kind = edge;
    else if (edge === "body") kind = tool === "slip" ? "slip" : tool === "slide" ? "slide" : "move";
    else if (tool === "roll") kind = "roll";
    else kind = edge === "l" ? "trim-l" : "trim-r";
    if ((kind === "slip" || kind === "slide") && !it.clip) kind = "move";
    let target = it;
    let n = neighbours(track, it);
    if (kind === "roll" && edge === "l") {
      // Rolling a head edge rolls the cut with the clip before it.
      if (!n.prev) return;
      target = n.prev;
      n = neighbours(track, target);
    }
    const linked = it.clip?.link_id ? project.tracks.flatMap((t) => t.clips.filter((c) => c.link_id === it.clip!.link_id && c.id !== it.id).map((c) => c.id)) : [];
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    setDrag({ kind, item: target, track, linked, x0: e.clientX, y0: e.clientY, dx: 0, targetTrack: track.id, moved: false, next: n.next, prev: n.prev });
  };

  const onPointerMove = (e: React.PointerEvent) => {
    if (!drag) return;
    const raw = Math.round((e.clientX - drag.x0) / ppf);
    const moved = drag.moved || Math.abs(e.clientX - drag.x0) > 3 || Math.abs(e.clientY - drag.y0) > 6;
    if (!moved) return;
    const it = drag.item;
    const pts = snap ? snapPoints(new Set([it.id, ...drag.linked])) : [];
    let dx = raw;
    let hit: number | null = null;
    const m = it.clip ? media.get(it.clip.media_id) : undefined;
    const sp = it.clip ? speedOf(it.clip) : 1;
    switch (drag.kind) {
      case "move":
      case "slide": {
        let start = Math.max(0, it.start + raw);
        if (snap) {
          const [s1, h1] = snapFrame(start, pts);
          const [e1, h2] = snapFrame(start + it.dur, pts);
          if (h1 !== null) { start = s1; hit = h1; } else if (h2 !== null) { start = e1 - it.dur; hit = h2; }
        }
        dx = start - it.start;
        let target = drag.targetTrack;
        if (drag.kind === "move") {
          const t = trackAtClientY(e.clientY);
          if (t && t.kind === drag.track.kind && !t.locked && (!it.title || t.kind === "video")) target = t.id;
        }
        setDrag({ ...drag, dx, targetTrack: target, moved });
        setSnapAt(hit);
        return;
      }
      case "trim-l": {
        let edge = it.start + raw;
        if (snap) { const [f, h] = snapFrame(edge, pts); edge = f; hit = h; }
        const headroom = it.clip ? Math.floor((it.clip.reversed ? (m?.duration_frames ?? 1e9) - it.clip.source_out : it.clip.source_in) / sp) : 1e9;
        dx = Math.min(Math.max(edge - it.start, -headroom, -it.start), it.dur - 1);
        break;
      }
      case "trim-r":
      case "roll": {
        let edge = it.start + it.dur + raw;
        if (snap) { const [f, h] = snapFrame(edge, pts); edge = f; hit = h; }
        const room = it.clip && m?.duration_frames != null ? Math.floor((it.clip.reversed ? it.clip.source_in : m.duration_frames - it.clip.source_out) / sp) : 1e9;
        dx = Math.max(Math.min(edge - (it.start + it.dur), room), -(it.dur - 1));
        if (drag.kind === "roll" && drag.next) dx = Math.min(dx, drag.next.dur - 1);
        break;
      }
      case "slip":
        dx = raw;
        break;
      case "fade-in":
        dx = Math.max(-it.fadeIn, Math.min(raw, it.dur - it.fadeOut - it.fadeIn));
        break;
      case "fade-out":
        dx = Math.min(it.fadeOut, Math.max(raw, -(it.dur - it.fadeIn - it.fadeOut)));
        break;
    }
    setDrag({ ...drag, dx, moved });
    setSnapAt(hit);
  };

  const onPointerUp = () => {
    if (!drag) return;
    const d = drag;
    setDrag(null);
    setSnapAt(null);
    if (!d.moved || d.dx === 0 && !(d.kind === "move" && d.targetTrack !== d.track.id)) return;
    const it = d.item;
    const c = it.clip;
    const sp = c ? speedOf(c) : 1;
    const src = (frames: number) => Math.round(frames * sp);
    switch (d.kind) {
      case "move":
        run(api.moveClip(it.id, d.targetTrack, it.start + d.dx));
        break;
      case "slide":
        run(api.slideClip(it.id, d.dx));
        break;
      case "slip":
        run(api.slipClip(it.id, -src(d.dx)));
        break;
      case "roll":
        run(api.rollEdit(it.id, d.dx));
        break;
      case "trim-l":
        if (!c) run(api.trimClip(it.id, "in", it.start + d.dx));
        else if (c.reversed) run(api.trimClip(it.id, "in", c.source_out - src(d.dx)));
        else run(api.trimClip(it.id, "in", c.source_in + src(d.dx)));
        break;
      case "trim-r":
        if (!c) run(api.trimClip(it.id, "out", it.start + it.dur + d.dx));
        else if (c.reversed) run(api.trimClip(it.id, "out", c.source_in - src(d.dx)));
        else run(api.trimClip(it.id, "out", c.source_out + src(d.dx)));
        break;
      case "fade-in":
        if (c) run(api.updateClip(it.id, { fade_in_frames: it.fadeIn + d.dx }));
        else run(api.updateTitle(it.id, { fade_in_frames: it.fadeIn + d.dx }));
        break;
      case "fade-out":
        if (c) run(api.updateClip(it.id, { fade_out_frames: it.fadeOut - d.dx }));
        else run(api.updateTitle(it.id, { fade_out_frames: it.fadeOut - d.dx }));
        break;
    }
  };

  // Geometry of an item, including in-flight drag previews.
  const geom = (track: Track, it: Item) => {
    let start = it.start, len = it.dur, trackId = track.id, fadeIn = it.fadeIn, fadeOut = it.fadeOut;
    if (drag?.moved) {
      const isMain = drag.item.id === it.id;
      const isLinked = drag.linked.includes(it.id);
      if ((drag.kind === "move" || drag.kind === "slide") && (isMain || isLinked)) {
        start += drag.dx;
        if (isMain && drag.kind === "move") trackId = drag.targetTrack;
      } else if (drag.kind === "slide" && drag.prev?.id === it.id) len += drag.dx;
      else if (drag.kind === "slide" && drag.next?.id === it.id) { start += drag.dx; len -= drag.dx; }
      else if (drag.kind === "roll" && isMain) len += drag.dx;
      else if (drag.kind === "roll" && drag.next?.id === it.id) { start += drag.dx; len -= drag.dx; }
      else if ((drag.kind === "trim-l") && (isMain || isLinked)) { start += drag.dx; len -= drag.dx; }
      else if (drag.kind === "trim-r" && (isMain || isLinked)) len += drag.dx;
      else if (drag.kind === "fade-in" && isMain) fadeIn += drag.dx;
      else if (drag.kind === "fade-out" && isMain) fadeOut -= drag.dx;
    }
    return { x: GUTTER + start * ppf, w: Math.max(2, len * ppf), trackId, fadeIn, fadeOut };
  };

  const scrubFrom = (e: React.PointerEvent) => {
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    set({ playhead: frameAtClientX(e.clientX), playing: false, activeMonitor: "timeline" });
    const move = (ev: PointerEvent) => set({ playhead: frameAtClientX(ev.clientX) });
    const up = () => { el.removeEventListener("pointermove", move); el.removeEventListener("pointerup", up); };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  };

  // Ruler ticks adapt to zoom.
  const firstVisible = Math.max(0, Math.floor((scroll.left - GUTTER) / ppf));
  const lastVisible = Math.ceil((scroll.left + scroll.width - GUTTER) / ppf);
  const steps = [1, 2, 5, 10, Math.round(fpsN), Math.round(fpsN) * 2, Math.round(fpsN) * 5, Math.round(fpsN) * 10, Math.round(fpsN) * 30, Math.round(fpsN) * 60];
  const major = steps.find((s) => s * ppf >= 110) ?? steps[steps.length - 1];
  const minor = steps.slice().reverse().find((s) => s < major && major % s === 0 && s * ppf >= 9) ?? major;
  const ticks: JSX.Element[] = [];
  for (let f = Math.floor(firstVisible / minor) * minor; f <= lastVisible; f += minor) {
    const isMajor = f % major === 0;
    ticks.push(<div key={f} className={`tick ${isMajor ? "major" : "minor"}`} style={{ left: GUTTER + f * ppf - scroll.left }} />);
    if (isMajor) ticks.push(<div key={`l${f}`} className="tlabel" style={{ left: GUTTER + f * ppf - scroll.left }}>{(() => { const tc = timecode(f, rate); return <><span style={{ opacity: 0.55 }}>{tc.slice(0, 3)}</span>{tc.slice(3)}</>; })()}</div>);
  }

  const allIds = new Set(project.tracks.flatMap((t) => itemsOf(t).map((i) => i.id)));
  const selectedItems = selection.filter((id) => allIds.has(id));
  const firstAudio = display.findIndex((t) => t.kind === "audio");
  const mIn = project.mark_in ?? null;
  const mOut = project.mark_out ?? null;

  // ---- native context menus
  const NEUTRAL = { lift: 0, gamma: 1, gain: 1, contrast: 1, saturation: 1 };
  const halfSec = Math.max(2, Math.round(fpsN / 2));
  const transitionItems = (c: Clip, hasPrev: boolean): CtxItem[] => {
    const add = (kind: TransitionKind, frames: number) => run(api.updateClip(c.id, { transition_in: { kind, duration_frames: frames } }));
    return [
      { text: "Add cross dissolve (1 s)", enabled: hasPrev, action: () => add("cross_dissolve", Math.round(fpsN)) },
      { text: "Add cross dissolve (½ s)", enabled: hasPrev, action: () => add("cross_dissolve", halfSec) },
      { text: "Add dip to black (1 s)", enabled: hasPrev, action: () => add("dip_to_black", Math.round(fpsN)) },
      { text: "Remove transition", enabled: !!c.transition_in, action: () => run(api.updateClip(c.id, { transition_in: null })) },
    ];
  };
  const itemMenu = (e: React.MouseEvent, t: Track, it: Item) => {
    const sel = selection.includes(it.id) ? selection : [it.id];
    if (!selection.includes(it.id)) set({ selection: sel });
    const at = frameAtClientX(e.clientX);
    const end = it.start + it.dur;
    const n = sel.length;
    const common: CtxItem[] = [
      { text: "Cut here", enabled: !t.locked && at > it.start && at < end, action: () => run(api.splitClip(it.id, at)) },
      { text: "Cut at playhead", accelerator: "Ctrl+B", enabled: !t.locked && playhead > it.start && playhead < end, action: () => run(api.splitClip(it.id, playhead)) },
      "sep",
      { text: n > 1 ? `Copy ${n} items` : "Copy", accelerator: "Ctrl+C", action: () => window.dispatchEvent(new CustomEvent("pirosman-clip", { detail: "copy" })) },
      { text: "Paste at playhead", accelerator: "Ctrl+V", action: () => window.dispatchEvent(new CustomEvent("pirosman-clip", { detail: "paste" })) },
      { text: n > 1 ? `Duplicate ${n} items` : "Duplicate", accelerator: "Ctrl+D", enabled: !t.locked, action: () => window.dispatchEvent(new CustomEvent("pirosman-clip", { detail: "duplicate" })) },
      "sep",
      { text: n > 1 ? `Delete ${n} items` : "Delete", accelerator: "Delete", enabled: !t.locked, action: () => run(api.deleteClips(sel, false)) },
      { text: n > 1 ? `Ripple delete ${n} items` : "Ripple delete", accelerator: "Shift+Delete", enabled: !t.locked, action: () => run(api.deleteClips(sel, true)) },
      "sep",
      { text: "Move playhead to start", action: () => set({ playhead: it.start, playing: false }) },
      { text: "Move playhead to end", action: () => set({ playhead: end, playing: false }) },
    ];
    const fades = (patch: (fin: number, fout: number) => void): CtxItem => ({
      text: "Fades",
      submenu: [
        { text: "Fade in ½ s", action: () => patch(halfSec, -1) },
        { text: "Fade out ½ s", action: () => patch(-1, halfSec) },
        { text: "Fade in and out 1 s", action: () => patch(Math.round(fpsN), Math.round(fpsN)) },
        { text: "Remove fades", enabled: it.fadeIn > 0 || it.fadeOut > 0, action: () => patch(0, 0) },
      ],
    });
    if (it.title) {
      const x = it.title;
      showContextMenu(e, [
        { text: "Edit title text…", action: () => promptText("Edit title", "Text", x.text, "Save", (v) => run(api.updateTitle(x.id, { text: v })), true) },
        fades((fi, fo) => run(api.updateTitle(x.id, { ...(fi >= 0 ? { fade_in_frames: Math.min(fi, x.duration) } : {}), ...(fo >= 0 ? { fade_out_frames: Math.min(fo, x.duration) } : {}) }))),
        "sep",
        ...common,
      ]);
      return;
    }
    const c = it.clip!;
    const hasPrev = !!t.clips.find((o) => o.id !== c.id && o.start + clipDur(o) === c.start);
    const graded = JSON.stringify(c.color) !== JSON.stringify(NEUTRAL);
    showContextMenu(e, [
      ...common,
      "sep",
      { text: "Transition before this clip", submenu: transitionItems(c, hasPrev) },
      fades((fi, fo) => run(api.updateClip(c.id, { ...(fi >= 0 ? { fade_in_frames: Math.min(fi, it.dur) } : {}), ...(fo >= 0 ? { fade_out_frames: Math.min(fo, it.dur) } : {}) }))),
      {
        text: "Speed",
        submenu: [0.25, 0.5, 1, 1.5, 2, 4].map((s) => ({ text: `${s}x`, checked: speedOf(c) === s, action: () => run(api.updateClip(c.id, { speed: s })) })),
      },
      { text: "Reverse", checked: !!c.reversed, enabled: media.get(c.media_id)?.kind !== "image", action: () => run(api.updateClip(c.id, { reversed: !c.reversed })) },
      "sep",
      { text: "Open source in viewer", action: () => set({ sourceMediaId: c.media_id, markIn: Math.min(c.source_in, c.source_out), markOut: c.source_out, activeMonitor: "source" }) },
      ...(t.kind === "video"
        ? [
            { text: "Grade in Color page", action: () => set({ page: "color", selection: [c.id], playhead: c.start }) },
            { text: "Reset grade", enabled: graded && !t.locked, action: () => run(api.updateClip(c.id, { color: NEUTRAL })) },
          ]
        : [{ text: "Reset clip gain", enabled: c.gain_db !== 0 && !t.locked, action: () => run(api.updateClip(c.id, { gain_db: 0 })) }]),
    ]);
  };
  const addTrackItems: CtxItem[] = [
    { text: "Add video track", action: () => run(api.addTrack("video")) },
    { text: "Add audio track", action: () => run(api.addTrack("audio")) },
  ];
  const trackMenu = (e: React.MouseEvent, t: Track) => {
    const isV = t.kind === "video";
    const same = project.tracks.filter((x) => x.kind === t.kind);
    const pos = same.findIndex((x) => x.id === t.id);
    const idxOf = (id: string) => project.tracks.findIndex((x) => x.id === id);
    const upN = isV ? same[pos + 1] : same[pos - 1];
    const downN = isV ? same[pos - 1] : same[pos + 1];
    const count = t.clips.length + (t.titles?.length ?? 0);
    showContextMenu(e, [
      { text: `Rename ${t.name}…`, action: () => setRenaming(t.id) },
      { text: isV ? "Hide track" : "Mute track", checked: t.muted, action: () => run(api.updateTrack(t.id, { muted: !t.muted })) },
      ...(isV ? [] : [{ text: "Solo track", checked: !!t.solo, action: () => run(api.updateTrack(t.id, { solo: !t.solo })) }]),
      { text: "Lock track", checked: t.locked, action: () => run(api.updateTrack(t.id, { locked: !t.locked })) },
      "sep",
      { text: "Move track up", enabled: !!upN, action: () => upN && run(api.reorderTrack(t.id, idxOf(upN.id))) },
      { text: "Move track down", enabled: !!downN, action: () => downN && run(api.reorderTrack(t.id, idxOf(downN.id))) },
      "sep",
      ...addTrackItems,
      { text: count ? `Remove ${t.name} (not empty)` : `Remove ${t.name}`, enabled: count === 0, action: () => run(api.removeTrack(t.id)) },
    ]);
  };
  const addTitleAt = (trackId: string, at: number) =>
    promptText("Add title", "Title text", "Title", "Add title", (v) => run(api.addTitle({ track_id: trackId, start: at, duration: Math.round(fpsN * 4), text: v })), true);
  const laneMenu = (e: React.MouseEvent) => {
    const at = frameAtClientX(e.clientX);
    const t = trackAtClientY(e.clientY);
    showContextMenu(e, [
      { text: "Move playhead here", action: () => set({ playhead: at, playing: false }) },
      { text: "Add marker here", accelerator: "M", action: () => run(api.addMarker(at)) },
      { text: "Add title here…", enabled: t?.kind === "video" && !t.locked, action: () => t && addTitleAt(t.id, at) },
      "sep",
      { text: "Set In here", accelerator: "I", action: () => run(api.setRange(at, mOut != null && mOut > at ? mOut : null)) },
      { text: "Set Out here", accelerator: "O", action: () => run(api.setRange(mIn != null && mIn < at ? mIn : null, at)) },
      { text: "Clear In/Out", enabled: mIn != null || mOut != null, action: () => run(api.setRange(null, null)) },
      "sep",
      { text: "Cut all tracks at playhead", accelerator: "Ctrl+B", action: () => run(api.splitAt(playhead)) },
      { text: view?.undo_label ? `Undo ${view.undo_label}` : "Undo", accelerator: "Ctrl+Z", enabled: !!view?.undo_label, action: () => run(api.undo()) },
      { text: view?.redo_label ? `Redo ${view.redo_label}` : "Redo", accelerator: "Ctrl+Shift+Z", enabled: !!view?.redo_label, action: () => run(api.redo()) },
      "sep",
      { text: "Select all", accelerator: "Ctrl+A", action: () => set({ selection: [...allIds] }) },
      { text: "Snapping", checked: snap, action: () => setSnap(!snap) },
      { text: "Fit timeline", accelerator: "Shift+Z", action: () => set({ pxPerFrame: Math.max(0.05, Math.min(24, (scroll.width - 40) / Math.max(duration, 1))) }) },
      "sep",
      ...addTrackItems,
    ]);
  };
  const markerMenu = (e: React.MouseEvent, m: Marker) =>
    showContextMenu(e, [
      { text: `Rename “${m.name}”…`, action: () => promptText("Rename marker", "Name", m.name, "Rename", (v) => run(api.updateMarker(m.id, { name: v }))) },
      { text: "Edit note…", action: () => promptText("Marker note", "Note", m.note ?? "", "Save", (v) => run(api.updateMarker(m.id, { note: v })), true) },
      { text: "Color", submenu: MARKER_COLORS.map((c) => ({ text: c[0].toUpperCase() + c.slice(1), checked: m.color === c, action: () => run(api.updateMarker(m.id, { color: c })) })) },
      { text: "Move to playhead", action: () => run(api.updateMarker(m.id, { frame: playhead })) },
      "sep",
      { text: "Delete marker", action: () => run(api.deleteMarker(m.id)) },
    ]);

  const toolButton = (t: Tool, Icon: typeof Scissors, title: string) => (
    <button aria-pressed={tool === t} className={t === "blade" ? "blade" : ""} onClick={() => setTool(t)} title={title} aria-label={title.split(":")[0].replace(/ \(.\)$/, "")}><Icon size={14} /></button>
  );

  return (
    <div className="panel tl" style={{ ["--track-h" as string]: `${TRACK_H}px` }}>
      <div className="tl-tools" role="toolbar" aria-label="Timeline tools">
        <div className="rail" role="group" aria-label="Edit tool">
          {toolButton("select", MousePointer2, "Selection (V)")}
          {toolButton("blade", Scissors, "Blade: click a clip to cut (B)")}
          {toolButton("slip", ChevronsLeftRight, "Slip: drag to change which source frames show (Y)")}
          {toolButton("slide", MoveHorizontal, "Slide: drag between neighbours, which absorb the move (U)")}
          {toolButton("roll", Columns2, "Roll: drag a cut to move it between two clips (R)")}
        </div>
        <div className="rail" role="group" aria-label="Edit">
          <button onClick={() => run(api.splitAt(playhead))} title="Cut all unlocked tracks at the playhead (Ctrl+B)" aria-label="Cut at playhead"><SquareSplitHorizontal size={14} /></button>
          <button disabled={!selectedItems.length} onClick={() => run(api.deleteClips(selectedItems, false))} title="Delete selected (Delete)" aria-label="Delete selected"><Trash2 size={14} /></button>
          <button disabled={!selectedItems.length} onClick={() => run(api.deleteClips(selectedItems, true))} title="Ripple delete: remove and close the gap (Shift+Delete)" aria-label="Ripple delete"><ArrowDownToLine size={14} style={{ transform: "rotate(-90deg)" }} /></button>
          <button onClick={() => run(api.addMarker(playhead))} title="Add marker at the playhead (M)" aria-label="Add marker"><Bookmark size={14} /></button>
        </div>
        <div className="rail" role="group" aria-label="Snapping and history">
          <button aria-pressed={snap} onClick={() => setSnap(!snap)} title={`Snapping ${snap ? "on" : "off"} (N)`} aria-label="Snapping"><Magnet size={14} /></button>
          <button disabled={!view?.undo_label} onClick={() => run(api.undo())} title={view?.undo_label ? `Undo ${view.undo_label} (Ctrl+Z)` : "Nothing to undo"} aria-label="Undo"><Undo2 size={14} /></button>
          <button disabled={!view?.redo_label} onClick={() => run(api.redo())} title={view?.redo_label ? `Redo ${view.redo_label} (Ctrl+Shift+Z)` : "Nothing to redo"} aria-label="Redo"><Redo2 size={14} /></button>
        </div>
        <div className="rail" role="group" aria-label="Add">
          <button
            className="labelled" title="Add a video track, an audio track, or a title at the playhead" aria-label="Add track"
            onClick={(e) => showContextMenu(e, [
              ...addTrackItems,
              "sep",
              { text: "Title at playhead", action: () => { const t = project.tracks.filter((x) => x.kind === "video" && !x.locked).slice(-1)[0]; if (t) addTitleAt(t.id, playhead); } },
            ])}
          ><Plus size={14} /><span>Track</span></button>
          <button title="Add a title at the playhead on the top video track" aria-label="Add title" onClick={() => { const t = project.tracks.filter((x) => x.kind === "video" && !x.locked).slice(-1)[0]; if (t) addTitleAt(t.id, playhead); }}><Type size={14} /></button>
        </div>
        <span className="spacer" />
        {!compact && changes.size > 0 && <span className="label" style={{ color: "var(--marker)", marginRight: 18 }}>{changes.size} item{changes.size > 1 ? "s" : ""} marked since {view?.vcs?.head?.slice(0, 7)}</span>}
        <div className="zoom rail" role="group" aria-label="Zoom">
          <button title="Zoom out (-)" aria-label="Zoom out" onClick={() => set({ pxPerFrame: Math.max(0.05, ppf / 1.25) })}><ZoomOut size={14} /></button>
          <input type="range" min={0} max={1} step={0.001} value={Math.log(ppf / 0.05) / Math.log(24 / 0.05)} aria-label="Timeline zoom" title="Timeline zoom (Ctrl+wheel over the timeline)"
            onChange={(e) => set({ pxPerFrame: 0.05 * Math.pow(24 / 0.05, Number(e.target.value)) })} />
          <button title="Zoom in (+)" aria-label="Zoom in" onClick={() => set({ pxPerFrame: Math.min(24, ppf * 1.25) })}><ZoomIn size={14} /></button>
          <span className="readout num">{ppf >= 1 ? `${ppf.toFixed(1)}px/f` : `${(1 / ppf).toFixed(1)}f/px`}</span>
          <button title="Fit timeline (Shift+Z)" aria-label="Fit timeline" onClick={() => set({ pxPerFrame: Math.max(0.05, Math.min(24, (scroll.width - 40) / Math.max(duration, 1))) })}><Maximize2 size={13} /></button>
        </div>
      </div>

      <div className="tl-ruler-row">
        <div className="tl-guide" title="First frame in view">
          <b className="num">{timecode(firstVisible, rate)}</b>
        </div>
        <div className="ruler" onPointerDown={(e) => e.button === 0 && scrubFrom(e)} onContextMenu={laneMenu} aria-label="Timeline ruler">
          {(mIn != null || mOut != null) && (
            <div
              className="range-band"
              style={{ left: GUTTER + (mIn ?? 0) * ppf - scroll.left, width: ((mOut ?? duration) - (mIn ?? 0)) * ppf }}
              title={`In ${timecode(mIn ?? 0, rate)} · Out ${timecode(mOut ?? duration, rate)}`}
            >
              {mIn != null && <span className="bracket in" />}
              {mOut != null && <span className="bracket out" />}
            </div>
          )}
          {ticks}
          {(project.markers ?? []).map((m) => (
            <button
              key={m.id} className="marker" style={{ left: GUTTER + m.frame * ppf - scroll.left, ["--mk" as string]: markerVar(m.color) }}
              title={`${m.name} · ${timecode(m.frame, rate)}${m.note ? `\n${m.note}` : ""}`}
              onPointerDown={(e) => { e.stopPropagation(); set({ playhead: m.frame, playing: false }); }}
              onDoubleClick={() => promptText("Rename marker", "Name", m.name, "Rename", (v) => run(api.updateMarker(m.id, { name: v })))}
              onContextMenu={(e) => markerMenu(e, m)}
              aria-label={`Marker ${m.name}`}
            />
          ))}
          {peers.filter((p) => p.playhead != null).map((p) => (
            <div key={p.id} className="ghost-head" style={{ left: GUTTER + p.playhead! * ppf - scroll.left, ["--peer" as string]: p.color }} title={`${p.name} · ${timecode(p.playhead!, rate)}`}><span>{p.name}</span></div>
          ))}
          <div className="phead" style={{ left: GUTTER + playhead * ppf - scroll.left }} />
          <div className="tlabel" style={{ right: 0, left: "auto", top: 0, bottom: 0, transform: "none", padding: "3px 8px 0 10px", background: "var(--ink-2)", color: "var(--bone)", boxShadow: "-8px 0 8px var(--ink-2)", zIndex: 2 }} title="Last frame in view">{timecode(lastVisible, rate)}</div>
        </div>
      </div>

      <div className="tl-body">
        <div className="tl-heads" style={{ transform: `translateY(${-scroll.top}px)` }} onContextMenu={(e) => showContextMenu(e, addTrackItems)}>
          {display.map((t, i) => {
            const isV = t.kind === "video";
            const n = t.clips.length + (t.titles?.length ?? 0);
            return (
              <div key={t.id} className={`thead ${t.kind} ${i === firstAudio ? "sep-top" : ""}`} onContextMenu={(e) => trackMenu(e, t)}>
                {isV ? <Film size={14} color="var(--clip-v-edge)" /> : <AudioLines size={14} color="var(--clip-a-edge)" />}
                <div style={{ minWidth: 0 }}>
                  {renaming === t.id ? (
                    <input
                      className="input tname-edit" autoFocus defaultValue={t.name}
                      onBlur={(e) => { setRenaming(null); if (e.target.value !== t.name) run(api.updateTrack(t.id, { name: e.target.value })); }}
                      onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); if (e.key === "Escape") setRenaming(null); }}
                    />
                  ) : (
                    <div className={`tname ${t.kind}`} onDoubleClick={() => setRenaming(t.id)} title="Double-click to rename">{t.name}</div>
                  )}
                  <div className="tsub">{n} item{n === 1 ? "" : "s"}</div>
                </div>
                <div className="tbtns">
                  {!isV && (
                    <button className={`btn icon small ${t.solo ? "solo-on" : "ghost"}`} aria-pressed={!!t.solo} title={t.solo ? "Unsolo" : "Solo"} onClick={() => run(api.updateTrack(t.id, { solo: !t.solo }))}><Headphones /></button>
                  )}
                  <button className={`btn icon small ${t.muted ? "warn-on" : "ghost"}`} aria-pressed={t.muted} title={isV ? (t.muted ? "Show track" : "Hide track") : t.muted ? "Unmute" : "Mute"} onClick={() => run(api.updateTrack(t.id, { muted: !t.muted }))}>
                    {isV ? (t.muted ? <EyeOff /> : <Eye />) : t.muted ? <VolumeX /> : <Volume2 />}
                  </button>
                  <button className={`btn icon small ${t.locked ? "on" : "ghost"}`} aria-pressed={t.locked} title={t.locked ? "Unlock track" : "Lock track"} onClick={() => run(api.updateTrack(t.id, { locked: !t.locked }))}>
                    {t.locked ? <Lock /> : <LockOpen />}
                  </button>
                </div>
              </div>
            );
          })}
        </div>

        <div
          className={`tl-lanes tool-${tool}`} ref={lanesRef}
          onScroll={(e) => { const el = e.currentTarget; setScroll({ left: el.scrollLeft, top: el.scrollTop, width: el.clientWidth }); }}
          onWheel={(e) => {
            if (e.ctrlKey) {
              e.preventDefault();
              const f = frameAtClientX(e.clientX);
              const next = Math.max(0.05, Math.min(24, ppf * (e.deltaY < 0 ? 1.15 : 1 / 1.15)));
              set({ pxPerFrame: next });
              requestAnimationFrame(() => { const el = lanesRef.current; if (el) el.scrollLeft = GUTTER + f * next - (e.clientX - el.getBoundingClientRect().left); });
            } else if (!e.shiftKey && Math.abs(e.deltaX) < Math.abs(e.deltaY) && display.length * TRACK_H <= (lanesRef.current?.clientHeight ?? 0)) {
              lanesRef.current!.scrollLeft += e.deltaY;
            }
          }}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerDown={(e) => { if (e.button === 0 && (e.target === e.currentTarget || (e.target as HTMLElement).classList.contains("lane"))) { set({ selection: [] }); scrubFrom(e); } }}
          onContextMenu={laneMenu}
        >
          <div className="tl-lanes-inner" style={{ width, height: display.length * TRACK_H }}>
            {display.map((t, i) => (
              <div
                key={t.id}
                className={`lane ${i === firstAudio ? "sep-top" : ""} ${t.locked ? "locked" : ""} ${t.muted ? "muted" : ""} ${hoverTrack === t.id ? "drop-ok" : ""}`}
                data-track={t.id}
              />
            ))}
            {hoverTrack && dnd?.payload.kind === "media" && (
              <div className="drop-ghost" style={{ left: GUTTER + hoverFrame * ppf, top: hoverRow * TRACK_H + 4, width: Math.max(24, (dnd.payload.frames ?? Math.round(fpsN * 5)) * ppf), height: TRACK_H - 8 }}>
                <span>{dnd.payload.label}</span><span className="tc num">{timecode(hoverFrame, rate)}</span>
              </div>
            )}
            {(mIn != null || mOut != null) && (
              <div className="range-shade" style={{ left: GUTTER + (mIn ?? 0) * ppf, width: ((mOut ?? duration) - (mIn ?? 0)) * ppf }} />
            )}
            {(project.markers ?? []).map((m) => (
              <div key={m.id} className="marker-line" style={{ left: GUTTER + m.frame * ppf, ["--mk" as string]: markerVar(m.color) }} />
            ))}
            {display.map((t, row) =>
              itemsOf(t).map((it) => {
                const g = geom(t, it);
                const r = drag?.moved && drag.item.id === it.id ? display.findIndex((x) => x.id === g.trackId) : row;
                const c = it.clip;
                const m = c ? media.get(c.media_id) : undefined;
                const info = c ? infos.get(c.media_id) : undefined;
                const isAudio = t.kind === "audio";
                const marks = changes.get(it.id);
                const mediaW = (m?.duration_frames ?? 0) * ppf / (c ? speedOf(c) : 1);
                const y = r * TRACK_H + 4;
                const h = TRACK_H - 8;
                const fiW = Math.min(g.w, g.fadeIn * ppf);
                const foW = Math.min(g.w, g.fadeOut * ppf);
                const speedBadge = c ? fmtSpeed(c) : null;
                const graded = c && (c.color.lift !== 0 || c.color.gamma !== 1 || c.color.gain !== 1 || c.color.contrast !== 1 || c.color.saturation !== 1 || !!c.lut);
                const cls = it.title ? "title" : isAudio ? "audio" : m?.kind === "image" ? "image" : "";
                const name = it.title ? it.title.text.split("\n")[0] : c!.name;
                const tr = c?.transition_in;
                return (
                  <div key={it.id}>
                    <div
                      className={`clip ${cls} ${selection.includes(it.id) ? "selected" : ""} ${drag?.item.id === it.id && drag.moved ? "dragging" : ""} ${presence.has(it.id) ? "peer-edit" : ""} ${hoverClip === it.id ? (isAudio ? "fx-no" : "fx-target") : ""}`}
                      data-clip={c ? it.id : undefined}
                      style={{ left: g.x, width: g.w, top: y, bottom: "auto", height: h, ...(presence.has(it.id) ? { ["--peer" as string]: presence.get(it.id)!.color } : {}) }}
                      onPointerDown={(e) => onItemDown(e, t, it, "body")}
                      onContextMenu={(e) => itemMenu(e, t, it)}
                      onDoubleClick={() => set({ playhead: it.start })}
                      data-peer={presence.get(it.id)?.name}
                      title={`${name}\n${timecode(it.start, rate)} – ${timecode(it.start + it.dur, rate)}${c ? `\nsource ${c.source_in}–${c.source_out}` : ""}`}
                      role="button"
                      aria-pressed={selection.includes(it.id)}
                      aria-label={`${name} on ${t.name}`}
                    >
                      {!isAudio && c && <Filmstrip clip={c} mediaRef={m} info={info} x={g.x} w={g.w} h={h} ppf={ppf} viewLeft={scroll.left} viewRight={scroll.left + scroll.width} />}
                      {isAudio && c && info?.wave && mediaW > 0 && (
                        <div className="cwave" style={{ backgroundImage: cssUrl(info.wave), backgroundSize: `${mediaW}px 100%`, backgroundPositionX: -(c.source_in / speedOf(c)) * ppf, transform: c.reversed ? "scaleX(-1)" : undefined }} />
                      )}
                      {(fiW > 0 || foW > 0) && (
                        <svg className="fade-ramps" width={g.w} height={h} aria-hidden="true">
                          {fiW > 0 && <><polygon points={`0,0 ${fiW},0 0,${h}`} /><line x1="0" y1={h} x2={fiW} y2="0" /></>}
                          {foW > 0 && <><polygon points={`${g.w},0 ${g.w - foW},0 ${g.w},${h}`} /><line x1={g.w} y1={h} x2={g.w - foW} y2="0" /></>}
                        </svg>
                      )}
                      <div className="cname">{it.title && <Type size={11} style={{ verticalAlign: -1, marginRight: 4 }} />}{name}</div>
                      {graded && <span className="fx">{c?.lut ? "LUT" : "CC"}</span>}
                      {speedBadge && <span className="speed-badge">{speedBadge}</span>}
                      {c?.ofx?.some((f) => f.enabled) && <span className="speed-badge ofx-badge" title={`OpenFX: ${c.ofx.filter((f) => f.enabled).map((f) => f.label).join(" → ")}`}>OFX</span>}
                      {isAudio && c && c.gain_db !== 0 && <span className="gain">{c.gain_db > 0 ? "+" : ""}{c.gain_db.toFixed(1)} dB</span>}
                      {c?.link_id && g.w > 40 && <span className="link" title="Linked picture and sound"><Link2 /></span>}
                      {!t.locked && tool !== "blade" && (
                        <>
                          <div className={`handle l ${tool === "roll" ? "roll" : ""}`} onPointerDown={(e) => onItemDown(e, t, it, "l")} title={tool === "roll" ? "Roll this cut" : "Trim in"} />
                          <div className={`handle r ${tool === "roll" ? "roll" : ""}`} onPointerDown={(e) => onItemDown(e, t, it, "r")} title={tool === "roll" ? "Roll this cut" : "Trim out"} />
                          {tool === "select" && g.w > 24 && (
                            <>
                              <div className="fade-handle l" style={{ left: Math.max(0, fiW - 4) }} onPointerDown={(e) => onItemDown(e, t, it, "fade-in")} title={`Fade in: ${g.fadeIn} frames — drag`} />
                              <div className="fade-handle r" style={{ right: Math.max(0, foW - 4) }} onPointerDown={(e) => onItemDown(e, t, it, "fade-out")} title={`Fade out: ${g.fadeOut} frames — drag`} />
                            </>
                          )}
                        </>
                      )}
                      {drag?.moved && drag.item.id === it.id && drag.kind === "slip" && <div className="slip-note">slip {(-drag.dx * speedOf(c!)) > 0 ? "+" : ""}{Math.round(-drag.dx * speedOf(c!))} f</div>}
                    </div>
                    {tr && !drag?.moved && (() => {
                      const lead = tr.kind === "cross_dissolve" ? Math.floor(tr.duration_frames / 2) : tr.duration_frames - Math.floor(tr.duration_frames / 2);
                      const left = g.x - Math.floor(tr.duration_frames / 2) * ppf;
                      const w = Math.max(14, tr.duration_frames * ppf);
                      return (
                        <div
                          className={`transition-pill ${tr.kind}`}
                          style={{ left: Math.min(left, g.x - 7), width: w, top: y + h / 2 - 9 }}
                          title={`${tr.kind === "cross_dissolve" ? "Cross dissolve" : "Dip to black"} · ${tr.duration_frames} frames (${lead} into this clip's head)`}
                          onPointerDown={(e) => { e.stopPropagation(); set({ selection: [c!.id] }); }}
                          onContextMenu={(e) => showContextMenu(e, [
                            { text: "Cross dissolve", checked: tr.kind === "cross_dissolve", action: () => run(api.updateClip(c!.id, { transition_in: { ...tr, kind: "cross_dissolve" } })) },
                            { text: "Dip to black", checked: tr.kind === "dip_to_black", action: () => run(api.updateClip(c!.id, { transition_in: { ...tr, kind: "dip_to_black" } })) },
                            { text: "Duration", submenu: [halfSec, Math.round(fpsN), Math.round(fpsN * 2)].map((f) => ({ text: `${f} frames`, checked: tr.duration_frames === f, action: () => run(api.updateClip(c!.id, { transition_in: { ...tr, duration_frames: f } })) })) },
                            "sep",
                            { text: "Remove transition", action: () => run(api.updateClip(c!.id, { transition_in: null })) },
                          ])}
                        >
                          {w > 54 && <span>{tr.kind === "cross_dissolve" ? "Dissolve" : "Dip"}</span>}
                        </div>
                      );
                    })()}
                    {marks && !drag?.moved && (
                      <>
                        <GreaseMark x={g.x} y={y} w={g.w} h={h} seed={it.id} />
                        {g.w > 30 && (() => {
                          const above = r > 0 && itemsOf(display[r - 1]).some((o) => changes.has(o.id) && o.start < it.start + it.dur && o.start + o.dur > it.start);
                          const inside = r === 0 || above;
                          return <div className="gp-note" style={{ left: g.x + g.w - (inside ? 6 : 2), top: inside ? y + h - 14 : y - 11, translate: "-100% 0" }}>{marks.join(" · ")}</div>;
                        })()}
                      </>
                    )}
                  </div>
                );
              }),
            )}
            {snapAt !== null && <div className="snapline" style={{ left: GUTTER + snapAt * ppf }} />}
            {peers.filter((p) => p.playhead != null).map((p) => <div key={p.id} className="ghost-line" style={{ left: GUTTER + p.playhead! * ppf, ["--peer" as string]: p.color }} />)}
            <div className="playhead" style={{ left: GUTTER + playhead * ppf }} />
          </div>
        </div>
      </div>
    </div>
  );
}

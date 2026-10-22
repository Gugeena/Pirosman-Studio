import { useEffect, useMemo, useRef, useState } from "react";
import { fileSrc } from "../lib/files";
import {
  ArrowLeftRight, Camera, FastForward, Rewind, ChevronFirst, ChevronLast, ChevronLeft, ChevronRight, Clapperboard, Columns2, CornerDownRight, ExternalLink, Loader2, Pause, Play,
  Puzzle, RectangleHorizontal, Replace, StepBack, StepForward, X,
} from "lucide-react";
import { api, type Clip, type ColorParams, type MediaRef, type Project, type TitleClip, type Track } from "../lib/api";
import { clipDur, clipEnd, fadeOpacity, sourceFrameAt, speedOf, titleEnd } from "../lib/clip";
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

interface Hit { clip: Clip; track: Track; media: MediaRef }

function topVideoAt(p: Project, media: Map<string, MediaRef>, frame: number): Hit | null {
  for (const t of p.tracks.filter((t) => t.kind === "video" && !t.muted).reverse()) {
    const c = t.clips.find((c) => c.start <= frame && frame < clipEnd(c));
    if (c) return { clip: c, track: t, media: media.get(c.media_id)! };
  }
  return null;
}

/** CSS approximations of the clip's transform, crop and effects (export renders the real filters). */
function clipStyle(c: Clip, opacity: number, gradeId: string | null): { media: React.CSSProperties; overlays: JSX.Element[] } {
  const t = c.transform;
  const k = c.crop;
  const parts: string[] = [];
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
  if (t) parts.push(`translate(${t.position_x * 100}%, ${t.position_y * 100}%) rotate(${t.rotation_deg}deg) scale(${t.scale_x}, ${t.scale_y})`);
  return {
    media: {
      opacity,
      filter: filters.length ? filters.join(" ") : undefined,
      transform: parts.join(" ") || undefined,
      clipPath: k ? `inset(${k.top * 100}% ${k.right * 100}% ${k.bottom * 100}% ${k.left * 100}%)` : undefined,
    },
    overlays,
  };
}

function TitleOverlay({ project, frame }: { project: Project; frame: number }) {
  const active: { t: Track; x: TitleClip }[] = [];
  for (const t of project.tracks.filter((t) => t.kind === "video" && !t.muted)) for (const x of t.titles ?? []) if (x.start <= frame && frame < titleEnd(x)) active.push({ t, x });
  if (!active.length) return null;
  const W = project.settings.width;
  return (
    <div className="title-overlay">
      {active.map(({ x }) => {
        const k = frame - x.start;
        let o = 1;
        if ((x.fade_in_frames ?? 0) > 0 && k < x.fade_in_frames!) o = Math.min(o, (k + 1) / (x.fade_in_frames! + 1));
        if ((x.fade_out_frames ?? 0) > 0 && k >= x.duration - x.fade_out_frames!) o = Math.min(o, (x.duration - k) / (x.fade_out_frames! + 1));
        const pos: React.CSSProperties =
          x.position === "top" ? { top: "8%", left: "50%", transform: "translateX(-50%)", textAlign: "center" }
          : x.position === "center" ? { top: "50%", left: "50%", transform: "translate(-50%,-50%)", textAlign: "center" }
          : x.position === "bottom" ? { bottom: "8%", left: "50%", transform: "translateX(-50%)", textAlign: "center" }
          : { top: "72%", left: "7%" };
        return <div key={x.id} style={{ ...pos, opacity: o, color: x.color_hex, fontSize: `${(x.font_size / W) * 100}cqw` }}>{x.text}</div>;
      })}
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

  const vhit = project ? topVideoAt(project, media, playhead) : null;
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
    if (rendered) { el.muted = false; syncEl(el, previewFile, framesToSeconds(playhead, rate), 1, false); return; }
    el.muted = true;
    const src = vhit && vhit.media.kind !== "image" && vinfo?.preview ? vinfo.preview : null;
    syncEl(el, src, vhit ? framesToSeconds(sourceFrameAt(vhit.clip, playhead), rate) : 0, vhit ? speedOf(vhit.clip) * Math.abs(shuttle) : 1, !!vhit?.clip.reversed || shuttle < 0);
  }, [playhead, playing, vhit?.clip.id, vinfo?.preview, rendered, previewFile, shuttle]);

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

  const grade = gradeOverride && vhit?.clip.id === gradeOverride.clipId ? gradeOverride.color : vhit?.clip.color;
  const filterId = vhit ? `grade-${vhit.clip.id}` : "";
  const useGrade = !rendered && grade && !neutral(grade);
  const aspect = project ? `${project.settings.width}/${project.settings.height}` : "16/9";
  const previewBusy = render?.job === "preview" && !render.result && !render.error;
  const opacity = vhit ? fadeOpacity(vhit.track, vhit.clip, playhead) : 1;
  const look = vhit && !rendered ? clipStyle(vhit.clip, opacity, useGrade ? filterId : null) : { media: {}, overlays: [] as JSX.Element[] };

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
          {useGrade && grade && <GradeFilter id={filterId} c={grade} />}
          <video ref={vref} preload="auto" crossOrigin="anonymous" style={{ ...look.media, visibility: rendered || (vhit && vhit.media.kind !== "image" && vinfo?.preview) ? "visible" : "hidden", ...(rendered ? { opacity: 1, transform: undefined, filter: undefined, clipPath: undefined } : {}) }} />
          {!rendered && vhit?.media.kind === "image" && <img className="still" src={fileSrc(vhit.media.path)} alt="" style={look.media} />}
          {!rendered && look.overlays}
          {!rendered && project && <TitleOverlay project={project} frame={playhead} />}
          {!rendered && !vhit && !(project?.tracks.some((t) => (t.titles ?? []).some((x) => x.start <= playhead && playhead < titleEnd(x)))) && <div className="black">{total === 0 ? "Place media on the timeline" : "Gap"}</div>}
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

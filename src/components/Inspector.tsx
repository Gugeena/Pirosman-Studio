import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ArrowDown, ArrowUp, Eye, EyeOff, FolderOpen, GitBranch, Plus, Puzzle, RotateCcw, Trash2 } from "lucide-react";
import { api, type Clip, type Crop, type DiffEntry, type OfxEffect, type OfxParamInfo, type OfxValue, type TitleClip, type TitlePosition, type Track, type Transform, type TransitionKind, type Vfx, type VfxKind } from "../lib/api";
import { clipDur, findTitle, speedOf } from "../lib/clip";
import { loadOfxCatalog, paramValue, pluginFor } from "../lib/ofx";
import { findClip, mediaCode, mediaMap, run, useUI } from "../lib/store";
import { timecode } from "../lib/time";

export function ChangeList({ items, empty }: { items: DiffEntry[]; empty: string }) {
  if (!items.length) return <p className="note">{empty}</p>;
  return <ul className="mark-list">{items.map((d, i) => <li key={i}>{d.summary}</li>)}</ul>;
}

/**
 * Slider + scrubbable value pill. Drag the value left/right to change it (Shift ×10,
 * Alt ×0.1), click it to type, arrows on the slider step. Commits once: on release,
 * Enter or blur.
 */
export function Num({ label, value, min, max, step, unit, onCommit, fmt }: { label: string; value: number; min: number; max: number; step: number; unit?: string; onCommit: (v: number) => void; fmt?: (v: number) => string }) {
  const [v, setV] = useState(value);
  const [txt, setTxt] = useState(String(value));
  const [scrubbing, setScrubbing] = useState(false);
  const escaped = useRef(false);
  const show = (x: number) => (fmt ? fmt(x) : String(+x.toFixed(step >= 1 ? 0 : 3)));
  useEffect(() => { setV(value); setTxt(show(value)); }, [value]);
  const clamp = (x: number) => Math.min(max, Math.max(min, x));
  const commit = (x: number) => { const c = clamp(x); if (Number.isFinite(c) && c !== value) onCommit(c); };
  const scrub = (e: React.PointerEvent<HTMLInputElement>) => {
    const el = e.currentTarget;
    if (e.button !== 0 || document.activeElement === el) return;
    e.preventDefault();
    const x0 = e.clientX;
    const v0 = v;
    let moved = false;
    let last = v0;
    el.setPointerCapture(e.pointerId);
    const mv = (ev: PointerEvent) => {
      const dx = ev.clientX - x0;
      if (!moved && Math.abs(dx) < 3) return;
      if (!moved) { moved = true; setScrubbing(true); }
      const k = ev.shiftKey ? 10 : ev.altKey ? 0.1 : 1;
      const raw = v0 + dx * step * k;
      last = clamp(step >= 1 ? Math.round(raw) : Math.round(raw / (step * (ev.altKey ? 0.1 : 1))) * step * (ev.altKey ? 0.1 : 1));
      setV(last);
      setTxt(show(last));
    };
    const up = () => {
      el.removeEventListener("pointermove", mv);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
      setScrubbing(false);
      if (moved) commit(last);
      else { el.focus(); el.select(); }
    };
    el.addEventListener("pointermove", mv);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  };
  return (
    <div className="slider-row">
      <span>{label}</span>
      <input type="range" min={min} max={max} step={step} value={v} aria-label={label}
        onChange={(e) => { setV(Number(e.target.value)); setTxt(e.target.value); }}
        onPointerUp={() => commit(v)} onKeyUp={(e) => e.key.startsWith("Arrow") && commit(v)} />
      <input className={`scrub-val ${scrubbing ? "scrubbing" : ""}`} value={txt} aria-label={`${label} value`} title="Drag to adjust (Shift ×10, Alt fine), click to type"
        onPointerDown={scrub} onChange={(e) => setTxt(e.target.value)}
        onBlur={() => { if (escaped.current) { escaped.current = false; setTxt(show(value)); } else commit(Number(txt)); }}
        onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); if (e.key === "Escape") { escaped.current = true; (e.target as HTMLInputElement).blur(); } }} />
      <span className="note">{unit ?? ""}</span>
    </div>
  );
}

const IDENTITY: Transform = { scale_x: 1, scale_y: 1, position_x: 0, position_y: 0, rotation_deg: 0 };
const NO_CROP: Crop = { left: 0, right: 0, top: 0, bottom: 0 };
export const VFX_LABEL: Record<VfxKind, string> = {
  gaussian_blur: "Gaussian blur",
  glow: "Glow / halation",
  vignette: "Vignette",
  film_grain: "Film grain",
  chromatic_aberration: "Chromatic aberration",
};

const EDITABLE = new Set(["double", "int", "bool", "choice", "rgb", "rgba", "double2d", "int2d", "double3d", "int3d", "string"]);
const DIMS: Record<string, string[]> = { rgb: ["R", "G", "B"], rgba: ["R", "G", "B", "A"], double2d: ["X", "Y"], int2d: ["X", "Y"], double3d: ["X", "Y", "Z"], int3d: ["X", "Y", "Z"] };

function rangeOf(info: OfxParamInfo, i: number, v: number): [number, number] {
  const lo = info.display_min?.[i] ?? info.min?.[i];
  const hi = info.display_max?.[i] ?? info.max?.[i];
  if (lo != null && hi != null && hi > lo) return [lo, hi];
  if (info.normalised) return [-1, 2];
  if (info.kind.startsWith("int")) return [Math.min(0, v), Math.max(100, v * 2)];
  return [Math.min(0, v * 2), Math.max(1, Math.abs(v) * 2)];
}
const toHex = (c: number[]) => `#${c.slice(0, 3).map((x) => Math.round(Math.max(0, Math.min(1, x)) * 255).toString(16).padStart(2, "0")).join("")}`;
const fromHex = (h: string) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16) / 255);

/** One OpenFX parameter, edited with the control its type calls for. */
function OfxParam({ info, value, onChange }: { info: OfxParamInfo; value: OfxValue | null; onChange: (v: OfxValue) => void }) {
  const label = info.label || info.name;
  const tip = info.hint || undefined;
  if (info.kind === "bool") {
    return <label className="check ofx-check" title={tip}><input type="checkbox" checked={!!value} onChange={(e) => onChange(e.target.checked)} />{label}</label>;
  }
  if (info.kind === "choice") {
    return (
      <div className="slider-row" title={tip}><span>{label}</span>
        <select className="select" style={{ gridColumn: "2 / 5" }} value={Number(value ?? 0)} onChange={(e) => onChange(Number(e.target.value))}>
          {(info.options ?? []).map((o, i) => <option key={i} value={i}>{o}</option>)}
        </select>
      </div>
    );
  }
  if (info.kind === "string") {
    return (
      <label className="field" title={tip}><span>{label}</span>
        <input className="input" defaultValue={String(value ?? "")} key={String(value ?? "")} onBlur={(e) => e.target.value !== String(value ?? "") && onChange(e.target.value)} onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()} />
      </label>
    );
  }
  const arr = Array.isArray(value) ? value : [Number(value ?? 0)];
  const isInt = info.kind.startsWith("int");
  const dims = DIMS[info.kind];
  const set = (i: number, x: number) => {
    const v = isInt ? Math.round(x) : x;
    if (!dims) onChange(v);
    else onChange(arr.map((y, j) => (j === i ? v : y)));
  };
  return (
    <div className="ofx-param" title={tip}>
      {(info.kind === "rgb" || info.kind === "rgba") && (
        <div className="slider-row"><span>{label}</span>
          <input type="color" className="ofx-color" value={toHex(arr)} onChange={(e) => onChange([...fromHex(e.target.value), ...(info.kind === "rgba" ? [arr[3] ?? 1] : [])])} aria-label={`${label} colour`} />
          <span className="note" style={{ gridColumn: "3 / 5" }}>{arr.slice(0, 3).map((x) => x.toFixed(2)).join(" ")}</span>
        </div>
      )}
      {(dims ?? [""]).map((d, i) => {
        if ((info.kind === "rgb" || info.kind === "rgba") && i < 3) return null;
        const v = arr[i] ?? 0;
        const [lo, hi] = rangeOf(info, i, v);
        return <Num key={i} label={d ? `${label} ${d}` : label} value={v} min={lo} max={hi} step={isInt ? 1 : (hi - lo) / 400} unit={info.normalised ? "×frame" : undefined} onCommit={(x) => set(i, x)} />;
      })}
    </div>
  );
}

/** The clip's OpenFX stack: bypass, reorder, reset, remove, and every parameter. */
function OfxSection({ clip, patch }: { clip: Clip; patch: (p: Parameters<typeof api.updateClip>[1]) => Promise<boolean> }) {
  const cat = useUI((s) => s.ofx);
  useEffect(() => { if (!cat) void loadOfxCatalog(); }, [cat]);
  const list = clip.ofx ?? [];
  const setOfx = (next: OfxEffect[]) => patch({ ofx: next });
  const move = (i: number, d: number) => { const n = [...list]; const [x] = n.splice(i, 1); n.splice(i + d, 0, x); setOfx(n); };
  return (
    <section className="insp-sec">
      <span className="label" style={{ display: "flex", alignItems: "center", gap: 6 }}><Puzzle size={12} />OpenFX<span className="spacer" /><span className="note">{list.length ? `${list.length} in stack` : ""}</span></span>
      {list.length === 0 && <p className="note">No OpenFX effects. Open <b>Effects</b> next to the Media Pool and drag an effect onto the clip, or double-click it while the clip is selected.</p>}
      {list.map((fx, i) => {
        const p = cat ? pluginFor(fx) : undefined;
        return (
          <div key={fx.id} className={`vfx-row ofx-row ${fx.enabled ? "" : "off"}`}>
            <div style={{ display: "flex", alignItems: "center", gap: 2 }}>
              <b style={{ fontSize: 12 }} title={`${fx.plugin_id}\n${fx.grouping}`}>{fx.label}</b><span className="spacer" />
              <button className="btn icon small ghost" title="Move up (applied earlier)" disabled={i === 0} onClick={() => move(i, -1)}><ArrowUp /></button>
              <button className="btn icon small ghost" title="Move down (applied later)" disabled={i === list.length - 1} onClick={() => move(i, 1)}><ArrowDown /></button>
              <button className="btn icon small ghost" title="Reset parameters to the plugin defaults" disabled={!Object.keys(fx.params).length} onClick={() => setOfx(list.map((x) => (x.id === fx.id ? { ...x, params: {} } : x)))}><RotateCcw /></button>
              <button className="btn icon small ghost" title={fx.enabled ? "Bypass" : "Enable"} onClick={() => setOfx(list.map((x) => (x.id === fx.id ? { ...x, enabled: !x.enabled } : x)))}>{fx.enabled ? <Eye /> : <EyeOff />}</button>
              <button className="btn icon small ghost" title="Remove effect" onClick={() => setOfx(list.filter((x) => x.id !== fx.id))}><Trash2 /></button>
            </div>
            {cat && !p && <p className="note err-text">{fx.plugin_id} is not installed on this computer. The clip cannot render until the plugin is installed or the effect is removed.</p>}
            {p && !p.supported && <p className="note err-text">Cannot run here: {p.reason}</p>}
            {p && fx.enabled && p.params.filter((x) => !x.secret && EDITABLE.has(x.kind)).map((info) => (
              <OfxParam key={info.name} info={info} value={paramValue(fx, info.name, info.default)}
                onChange={(v) => setOfx(list.map((x) => (x.id === fx.id ? { ...x, params: { ...x.params, [info.name]: v } } : x)))} />
            ))}
          </div>
        );
      })}
      {list.length > 0 && <p className="note">OpenFX renders through the plugin for exports, Render preview and the paused viewer frame.</p>}
    </section>
  );
}

function ClipInspector({ track, clip, changes }: { track: Track; clip: Clip; changes: DiffEntry[] }) {
  const view = useUI((s) => s.view)!;
  const rate = view.project.settings.frame_rate;
  const fps = Math.round(rate.num / rate.den);
  const m = mediaMap(view).get(clip.media_id);
  const [name, setName] = useState(clip.name);
  useEffect(() => setName(clip.name), [clip.id, clip.name]);
  const dur = clipDur(clip);
  const isV = track.kind === "video";
  const still = m?.kind === "image";
  const tf = clip.transform ?? IDENTITY;
  const cr = clip.crop ?? NO_CROP;
  const vfx = clip.vfx ?? [];
  const patch = (p: Parameters<typeof api.updateClip>[1]) => run(api.updateClip(clip.id, p));
  const prev = track.clips.find((o) => o.id !== clip.id && o.start + clipDur(o) === clip.start);
  const tr = clip.transition_in;
  const setVfx = (next: Vfx[]) => patch({ vfx: next });

  return (
    <>
      <section className="insp-sec">
        <span className="label">Clip</span>
        <label className="field"><span>Name</span>
          <input className="input" value={name} onChange={(e) => setName(e.target.value)} onBlur={() => name !== clip.name && patch({ name })} onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()} />
        </label>
        <dl className="kv">
          <dt>Track</dt><dd>{track.name}</dd>
          <dt>Start</dt><dd>{timecode(clip.start, rate)}</dd>
          <dt>End</dt><dd>{timecode(clip.start + dur, rate)}</dd>
          <dt>Source</dt><dd>{clip.source_in}–{clip.source_out} f</dd>
          <dt>Duration</dt><dd>{dur} f</dd>
          <dt>Media</dt><dd className="text">{m ? `${mediaCode(view, m.id)} ${m.name}` : "—"}</dd>
        </dl>
        {still && <Num label="Still length" value={dur} min={1} max={fps * 600} step={1} unit="f" onCommit={(v) => run(api.trimClip(clip.id, "out", clip.source_in + Math.round(v)))} />}
      </section>

      {!still && (
        <section className="insp-sec">
          <span className="label">Speed</span>
          <div className="slider-row">
            <span>Speed</span>
            <select className="select" value={speedOf(clip)} onChange={(e) => patch({ speed: Number(e.target.value) })}>
              {[0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4].map((s) => <option key={s} value={s}>{s}x</option>)}
            </select>
            <label className="check" style={{ gridColumn: "3 / 5" }}><input type="checkbox" checked={!!clip.reversed} onChange={(e) => patch({ reversed: e.target.checked })} />Reverse</label>
          </div>
        </section>
      )}

      <section className="insp-sec">
        <span className="label">Fades & transition</span>
        <Num label="Fade in" value={clip.fade_in_frames ?? 0} min={0} max={dur - (clip.fade_out_frames ?? 0)} step={1} unit="f" onCommit={(v) => patch({ fade_in_frames: Math.round(v) })} />
        <Num label="Fade out" value={clip.fade_out_frames ?? 0} min={0} max={dur - (clip.fade_in_frames ?? 0)} step={1} unit="f" onCommit={(v) => patch({ fade_out_frames: Math.round(v) })} />
        <div className="slider-row">
          <span>In from cut</span>
          <select className="select" disabled={!prev} value={tr?.kind ?? "none"} title={prev ? undefined : "No clip ends where this one starts"}
            onChange={(e) => patch({ transition_in: e.target.value === "none" ? null : { kind: e.target.value as TransitionKind, duration_frames: tr?.duration_frames ?? fps } })}>
            <option value="none">None</option><option value="cross_dissolve">Cross dissolve</option><option value="dip_to_black">Dip to black</option>
          </select>
          {tr ? (
            <input className="input num" defaultValue={tr.duration_frames} key={tr.duration_frames} aria-label="Transition frames"
              onBlur={(e) => { const n = Math.round(Number(e.target.value)); if (n !== tr.duration_frames) patch({ transition_in: { ...tr, duration_frames: n } }); }}
              onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()} />
          ) : <span />}
          <span className="note">{tr ? "f" : ""}</span>
        </div>
      </section>

      {isV && (
        <section className="insp-sec">
          <span className="label" style={{ display: "flex" }}>Transform & crop<span className="spacer" />
            <button className="btn icon small ghost" title="Reset transform and crop" disabled={JSON.stringify(tf) === JSON.stringify(IDENTITY) && JSON.stringify(cr) === JSON.stringify(NO_CROP)} onClick={() => patch({ transform: IDENTITY, crop: NO_CROP })}><RotateCcw /></button>
          </span>
          <Num label="Zoom X" value={tf.scale_x} min={0.05} max={8} step={0.01} onCommit={(v) => patch({ transform: { ...tf, scale_x: v } })} />
          <Num label="Zoom Y" value={tf.scale_y} min={0.05} max={8} step={0.01} onCommit={(v) => patch({ transform: { ...tf, scale_y: v } })} />
          <Num label="Position X" value={tf.position_x} min={-2} max={2} step={0.005} onCommit={(v) => patch({ transform: { ...tf, position_x: v } })} />
          <Num label="Position Y" value={tf.position_y} min={-2} max={2} step={0.005} onCommit={(v) => patch({ transform: { ...tf, position_y: v } })} />
          <Num label="Rotation" value={tf.rotation_deg} min={-180} max={180} step={0.5} unit="°" onCommit={(v) => patch({ transform: { ...tf, rotation_deg: v } })} />
          {(["left", "right", "top", "bottom"] as const).map((k) => (
            <Num key={k} label={`Crop ${k}`} value={cr[k] * 100} min={0} max={45} step={0.5} unit="%" onCommit={(v) => patch({ crop: { ...cr, [k]: Math.round(v * 10) / 1000 } })} />
          ))}
        </section>
      )}

      {isV && (
        <section className="insp-sec">
          <span className="label" style={{ display: "flex", alignItems: "center" }}>Effects<span className="spacer" />
            <select className="select" style={{ width: 150, height: 22 }} value="" aria-label="Add effect"
              onChange={(e) => e.target.value && setVfx([...vfx, { id: crypto.randomUUID(), kind: e.target.value as VfxKind, amount: 0.35, enabled: true }])}>
              <option value="">Add effect…</option>
              {(Object.keys(VFX_LABEL) as VfxKind[]).map((k) => <option key={k} value={k}>{VFX_LABEL[k]}</option>)}
            </select>
          </span>
          {vfx.length === 0 && <p className="note">No effects. Effects render through FFmpeg (gblur, glow, vignette, noise, rgbashift) on export; the viewer shows an approximation.</p>}
          {vfx.map((fx, i) => (
            <div key={fx.id} className="vfx-row">
              <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
                <b style={{ fontSize: 12, opacity: fx.enabled ? 1 : 0.5 }}>{VFX_LABEL[fx.kind]}</b><span className="spacer" />
                <button className="btn icon small ghost" title={fx.enabled ? "Bypass" : "Enable"} onClick={() => setVfx(vfx.map((x) => (x.id === fx.id ? { ...x, enabled: !x.enabled } : x)))}>{fx.enabled ? <Eye /> : <EyeOff />}</button>
                <button className="btn icon small ghost" title="Remove effect" onClick={() => setVfx(vfx.filter((x) => x.id !== fx.id))}><Trash2 /></button>
              </div>
              <Num label="Amount" value={fx.amount} min={0} max={1} step={0.01} onCommit={(v) => setVfx(vfx.map((x, j) => (j === i ? { ...x, amount: v } : x)))} />
            </div>
          ))}
          <div className="slider-row">
            <span>LUT</span>
            <span className="note" style={{ gridColumn: "2 / 4", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={clip.lut ?? undefined}>{clip.lut ? clip.lut.split(/[\\/]/).pop() : "none"}</span>
            <span style={{ display: "flex", gap: 2 }}>
              <button className="btn icon small ghost" title="Choose .cube LUT" onClick={async () => { const p = await open({ multiple: false, filters: [{ name: "3D LUT", extensions: ["cube"] }] }); if (typeof p === "string") patch({ lut: p }); }}><FolderOpen /></button>
              {clip.lut && <button className="btn icon small ghost" title="Remove LUT" onClick={() => patch({ lut: null })}><Trash2 /></button>}
            </span>
          </div>
        </section>
      )}

      {isV && <OfxSection clip={clip} patch={patch} />}

      {track.kind === "audio" && (
        <section className="insp-sec">
          <span className="label">Audio</span>
          <Num label="Clip gain" value={clip.gain_db} min={-30} max={12} step={0.5} unit="dB" onCommit={(v) => patch({ gain_db: v })} />
        </section>
      )}

      <section className="insp-sec">
        <span className="label">Since last revision</span>
        <ChangeList items={changes} empty={view.vcs?.head ? "Unchanged." : "No revision yet."} />
      </section>
    </>
  );
}

function TitleInspector({ title }: { title: TitleClip }) {
  const [text, setText] = useState(title.text);
  useEffect(() => setText(title.text), [title.id, title.text]);
  const patch = (p: Parameters<typeof api.updateTitle>[1]) => run(api.updateTitle(title.id, p));
  return (
    <section className="insp-sec">
      <span className="label">Title</span>
      <label className="field"><span>Text</span>
        <textarea className="textarea" rows={3} value={text} onChange={(e) => setText(e.target.value)} onBlur={() => text !== title.text && text.trim() && patch({ text })} />
      </label>
      <Num label="Size" value={title.font_size} min={8} max={400} step={1} unit="px" onCommit={(v) => patch({ font_size: Math.round(v) })} />
      <div className="slider-row">
        <span>Color</span>
        <input type="color" value={title.color_hex} onChange={(e) => patch({ color_hex: e.target.value })} aria-label="Title color" style={{ width: "100%", height: 24, background: "none", border: 0 }} />
        <select className="select" style={{ gridColumn: "3 / 5" }} value={title.position} onChange={(e) => patch({ position: e.target.value as TitlePosition })}>
          <option value="top">Top</option><option value="center">Center</option><option value="bottom">Bottom</option><option value="lower_third">Lower third</option>
        </select>
      </div>
      <Num label="Duration" value={title.duration} min={1} max={20000} step={1} unit="f" onCommit={(v) => patch({ duration: Math.round(v) })} />
      <Num label="Fade in" value={title.fade_in_frames ?? 0} min={0} max={title.duration} step={1} unit="f" onCommit={(v) => patch({ fade_in_frames: Math.round(v) })} />
      <Num label="Fade out" value={title.fade_out_frames ?? 0} min={0} max={title.duration} step={1} unit="f" onCommit={(v) => patch({ fade_out_frames: Math.round(v) })} />
    </section>
  );
}

/** Commit box and the changes it would record, styled as a compact version-control widget. */
export function RevisionControl() {
  const view = useUI((s) => s.view);
  const author = useUI((s) => s.author);
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);
  if (!view) return null;
  const changes = view.vcs?.head ? view.vcs.changes : [];
  const canCommit = !!view.path && !view.vcs?.merging;
  const commit = async () => {
    const m = msg.trim();
    if (!m || !canCommit) return;
    setBusy(true);
    if (await run(api.commit(m, author), "Revision created")) setMsg("");
    setBusy(false);
  };
  return (
    <section className="insp-sec rev-mini">
      <span className="label">Revision control</span>
      {view.vcs && (
        <div className="head">
          <GitBranch size={13} /><b>{view.vcs.branch}</b>
          <span className="hint">{view.vcs.head ? view.vcs.head.slice(0, 7) : "no revisions yet"}</span>
        </div>
      )}
      {!view.path ? (
        <p className="note">Save the project (Ctrl+S) to start its revision history.</p>
      ) : (
        <>
          <textarea className="textarea" rows={2} value={msg} placeholder="Describe this cut" aria-label="Revision message"
            onChange={(e) => setMsg(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter" && e.ctrlKey) void commit(); }} />
          <div className="actions">
            <button className="btn small primary" disabled={!canCommit || !msg.trim() || busy} onClick={() => void commit()} title="Commit a revision (Ctrl+Enter)"><Plus />Commit revision</button>
            <span className="hint">{view.vcs?.merging ? `merging ${view.vcs.merging}` : view.vcs?.head ? `${changes.length} change${changes.length === 1 ? "" : "s"} since last` : "first revision"}</span>
          </div>
          {view.vcs?.head && <ChangeList items={changes} empty="The cut matches the last revision." />}
        </>
      )}
    </section>
  );
}

export function Inspector() {
  const view = useUI((s) => s.view);
  const selection = useUI((s) => s.selection);
  if (!view) return null;
  const id = selection[selection.length - 1];
  const sel = findClip(view, id);
  const title = findTitle(view.project, id);
  const changes = view.vcs?.head ? view.vcs.changes : [];
  const heading = sel ? sel.clip.name : title ? "Title" : null;

  return (
    <div className="panel" aria-label="Inspector">
      <div className="panel-head"><span className="label">Inspector{heading && <> · <b>{heading}</b></>}</span></div>
      <div className="panel-body">
        <div className="insp">
          {sel ? (
            <ClipInspector track={sel.track} clip={sel.clip} changes={changes.filter((c) => c.clip_id === sel.clip.id)} />
          ) : title ? (
            <>
              <TitleInspector title={title.title} />
              <section className="insp-sec"><span className="label">Since last revision</span><ChangeList items={changes.filter((c) => c.clip_id === title.title.id)} empty="Unchanged." /></section>
            </>
          ) : (
            <>
              <section className="insp-sec">
                <span className="label">Project specs</span>
                <dl className="kv">
                  <dt>Resolution</dt><dd>{view.project.settings.width}×{view.project.settings.height}</dd>
                  <dt>Frame rate</dt><dd>{(view.project.settings.frame_rate.num / view.project.settings.frame_rate.den).toFixed(2)} fps</dd>
                  <dt>Sample rate</dt><dd>{(view.project.settings.sample_rate / 1000).toFixed(1)} kHz</dd>
                  <dt>Media</dt><dd>{view.project.media.length}</dd>
                  <dt>Clips</dt><dd>{view.project.tracks.reduce((n, t) => n + t.clips.length, 0)}</dd>
                  <dt>Markers</dt><dd>{view.project.markers?.length ?? 0}</dd>
                </dl>
              </section>
              <RevisionControl />
            </>
          )}
        </div>
      </div>
    </div>
  );
}

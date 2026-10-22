import { useEffect, useMemo, useState } from "react";
import { cssUrl } from "../lib/files";
import { open } from "@tauri-apps/plugin-dialog";
import { Copy, FolderOpen, RotateCcw } from "lucide-react";
import { Scopes } from "../components/Scopes";
import { Split } from "../components/Split";
import { api, type ColorParams } from "../lib/api";
import { findClip, infoMap, run, useUI } from "../lib/store";
import { timecode } from "../lib/time";
import { TimelineMonitor } from "../components/Monitors";

const NEUTRAL: ColorParams = { lift: 0, gamma: 1, gain: 1, contrast: 1, saturation: 1 };
const CONTROLS: { key: keyof ColorParams; label: string; min: number; max: number; step: number }[] = [
  { key: "lift", label: "Lift", min: -0.5, max: 0.5, step: 0.005 },
  { key: "gamma", label: "Gamma", min: 0.2, max: 4, step: 0.01 },
  { key: "gain", label: "Gain", min: 0.25, max: 4, step: 0.01 },
  { key: "contrast", label: "Contrast", min: 0, max: 2, step: 0.01 },
  { key: "saturation", label: "Saturation", min: 0, max: 3, step: 0.01 },
];

/** Clips on video tracks in timeline order: the grading strip. */
function ClipStrip() {
  const view = useUI((s) => s.view);
  const selection = useUI((s) => s.selection);
  const set = useUI((s) => s.set);
  const infos = useMemo(() => infoMap(view), [view]);
  if (!view) return null;
  const clips = view.project.tracks.filter((t) => t.kind === "video").flatMap((t) => t.clips.map((c) => ({ t, c }))).sort((a, b) => a.c.start - b.c.start);
  const rate = view.project.settings.frame_rate;
  return (
    <div className="panel" aria-label="Clips to grade">
      <div className="panel-head"><span className="label">Clips · <b>{clips.length}</b></span><span className="spacer" /><span className="note">Select a clip to grade it; the playhead jumps to it.</span></div>
      <div className="panel-body" style={{ display: "flex", gap: 10, padding: 10, overflowX: "auto", overflowY: "hidden" }}>
        {clips.length === 0 && <div className="empty" style={{ width: "100%" }}><p>No video clips on the timeline yet.</p></div>}
        {clips.map(({ t, c }, i) => {
          const info = infos.get(c.media_id);
          const graded = JSON.stringify(c.color) !== JSON.stringify(NEUTRAL);
          const on = selection.includes(c.id);
          return (
            <button key={c.id} className="pool-cell" aria-selected={on} style={{ width: 150, flex: "none" }} onClick={() => set({ selection: [c.id], playhead: c.start })}>
              <div className="pool-thumb" style={info?.thumb ? { backgroundImage: cssUrl(info.thumb) } : undefined}>
                <span className="dur">{timecode(c.start, rate)}</span>
                {graded && <span className="kind" style={{ width: "auto", padding: "0 5px", color: "var(--bone)", fontSize: 10, fontWeight: 700 }}>CC</span>}
              </div>
              <div className="pool-tag"><span className="code">{String(i + 1).padStart(2, "0")} {t.name}</span><span className="name">{c.name}</span></div>
            </button>
          );
        })}
      </div>
    </div>
  );
}

export function ColorWorkspace() {
  const view = useUI((s) => s.view);
  const selection = useUI((s) => s.selection);
  const sel = findClip(view, selection[selection.length - 1]);
  const target = sel && sel.track.kind === "video" ? sel : null;
  const [draft, setDraft] = useState<ColorParams | null>(null);
  const [copied, setCopied] = useState<ColorParams | null>(null);
  useEffect(() => setDraft(null), [target?.clip.id]);
  const values = draft ?? target?.clip.color ?? NEUTRAL;
  const commit = (c: ColorParams) => {
    if (!target) return;
    if (JSON.stringify(c) !== JSON.stringify(target.clip.color)) run(api.updateClip(target.clip.id, { color: c })).then(() => setDraft(null));
    else setDraft(null);
  };
  const others = selection.filter((id) => id !== target?.clip.id && findClip(view, id)?.track.kind === "video");

  return (
    <div className="page" style={{ gridTemplateRows: "minmax(0, 1fr) 190px" }}>
      <div className="page" style={{ gridTemplateColumns: "minmax(0, 1fr) 360px" }}>
        <div className="page">
          <Split id="color-v" dir="col" initial={[64, 36]} min={150}>
            <TimelineMonitor title="Color" gradeOverride={target && draft ? { clipId: target.clip.id, color: draft } : undefined} />
            <Scopes />
          </Split>
        </div>
        <div className="panel" aria-label="Primary correction">
          <div className="panel-head"><span className="label">Primaries{target && <> · <b>{target.clip.name}</b></>}</span></div>
          <div className="panel-body">
            {!target ? (
              <div className="empty"><h3>No clip selected</h3><p>Pick a video clip from the strip below or the Edit timeline.</p></div>
            ) : (
              <div className="insp">
                <section className="insp-sec">
                  <span className="label">Correction</span>
                  {CONTROLS.map(({ key, label, min, max, step }) => (
                    <div className="slider-row" key={key}>
                      <span>{label}</span>
                      <input
                        type="range" min={min} max={max} step={step} value={values[key]} aria-label={label}
                        onChange={(e) => setDraft({ ...values, [key]: Number(e.target.value) })}
                        onPointerUp={() => draft && commit(draft)}
                        onKeyUp={() => draft && commit(draft)}
                        onDoubleClick={() => commit({ ...values, [key]: NEUTRAL[key] })}
                      />
                      <input
                        className="input num" value={Number(values[key].toFixed(3))} aria-label={`${label} value`}
                        onChange={(e) => { const v = Number(e.target.value); if (Number.isFinite(v)) setDraft({ ...values, [key]: v }); }}
                        onBlur={() => draft && commit(draft)} onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
                      />
                      <button className="btn icon small ghost" title={`Reset ${label}`} onClick={() => commit({ ...values, [key]: NEUTRAL[key] })}><RotateCcw /></button>
                    </div>
                  ))}
                </section>
                <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                  <button className="btn" onClick={() => commit(NEUTRAL)}><RotateCcw />Reset grade</button>
                  <button className="btn ghost" onClick={() => setCopied(values)}><Copy />Copy grade</button>
                  <button className="btn ghost" disabled={!copied} onClick={() => copied && commit(copied)}>Paste</button>
                  {others.length > 0 && <button className="btn ghost" onClick={async () => { for (const id of others) await run(api.updateClip(id, { color: values })); }}>Apply to {others.length} other selected</button>}
                </div>
                <section className="insp-sec">
                  <span className="label">3D LUT</span>
                  <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                    <span className="note" style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={target.clip.lut ?? undefined}>{target.clip.lut ? target.clip.lut.split(/[\\/]/).pop() : "No LUT"}</span>
                    <button className="btn small" onClick={async () => { const p = await open({ multiple: false, filters: [{ name: "3D LUT", extensions: ["cube"] }] }); if (typeof p === "string") run(api.updateClip(target.clip.id, { lut: p })); }}><FolderOpen />Choose .cube</button>
                    {target.clip.lut && <button className="btn small ghost" onClick={() => run(api.updateClip(target.clip.id, { lut: null }))}>Clear</button>}
                  </div>
                  <p className="note">Applied after the primaries with FFmpeg <b>lut3d</b> (trilinear). The live monitor does not apply LUTs; the scopes and Rendered preview do.</p>
                </section>
                <section className="insp-sec">
                  <span className="label">How this renders</span>
                  <p className="note">Export applies FFmpeg <b>colorlevels</b> (lift, gain) then <b>eq</b> (contrast, saturation, gamma) to this clip only. The monitor shows an SVG-filter approximation of the same math; use <b>Render preview</b> to see the exact FFmpeg result.</p>
                </section>
              </div>
            )}
          </div>
        </div>
      </div>
      <ClipStrip />
    </div>
  );
}

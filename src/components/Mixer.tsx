import { useEffect, useRef, useState } from "react";
import { api, type Track, type TrackPatch } from "../lib/api";
import { levelDb } from "../lib/audioEngine";
import { run, useUI } from "../lib/store";

/** Audio mixer: per-track fader, pan, 3-band EQ, mute/solo, live meters. */
export function Mixer() {
  const view = useUI((s) => s.view);
  const tracks = view?.project.tracks.filter((t) => t.kind === "audio") ?? [];
  return (
    <div className="panel">
      <div className="panel-head"><span className="label">Mixer · <b>{tracks.length}</b></span></div>
      <div className="mixer" style={{ flex: 1, minHeight: 0 }}>
        {tracks.map((t) => <Strip key={t.id} t={t} />)}
        {!tracks.length && <div className="empty"><p>No audio tracks.</p></div>}
      </div>
    </div>
  );
}

function Strip({ t }: { t: Track }) {
  const [db, setDb] = useState(-90);
  const [draft, setDraft] = useState<TrackPatch>({});
  const timer = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => {
    let raf = 0;
    const tick = () => { setDb(levelDb(t.id)); raf = requestAnimationFrame(tick); };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [t.id]);
  useEffect(() => setDraft({}), [t]);

  // Sliders patch locally, then commit one undoable edit after the drag settles.
  const patch = (p: TrackPatch) => {
    setDraft((d) => ({ ...d, ...p }));
    clearTimeout(timer.current);
    timer.current = setTimeout(() => run(api.updateTrack(t.id, p)), 220);
  };
  const v = { volume_db: t.volume_db ?? 0, pan: t.pan ?? 0, eq_low_db: t.eq_low_db ?? 0, eq_mid_db: t.eq_mid_db ?? 0, eq_high_db: t.eq_high_db ?? 0, ...draft };
  const h = Math.max(0, Math.min(100, ((db + 60) / 66) * 100));
  // Same bands as the export (render.rs: bass f=100, equalizer f=1000 w=1.5 octaves, treble f=8000).
  const eq = (k: "eq_low_db" | "eq_mid_db" | "eq_high_db", label: string, band: string) => (
    <>
      <span title={band}>{label}</span>
      <input type="range" min={-18} max={18} step={0.5} value={v[k]} aria-label={`${band} EQ`} title={`${band}: ${v[k].toFixed(1)} dB (double-click to reset)`} onChange={(e) => patch({ [k]: Number(e.target.value) })} onDoubleClick={() => patch({ [k]: 0 })} />
      <span className={`val ${v[k] ? "on" : ""}`}>{v[k] > 0 ? "+" : ""}{v[k] % 1 ? v[k].toFixed(1) : v[k]}</span>
    </>
  );
  return (
    <div className="strip" aria-label={`${t.name} channel strip`}>
      <div className="sname" title={t.name}>{t.name}</div>
      <div className="eq">{eq("eq_high_db", "H", "High shelf 8 kHz")}{eq("eq_mid_db", "M", "Mid bell 1 kHz")}{eq("eq_low_db", "L", "Low shelf 100 Hz")}</div>
      <div className="fader">
        <input type="range" min={-60} max={12} step={0.5} value={v.volume_db} aria-label="Fader" onChange={(e) => patch({ volume_db: Number(e.target.value) })} onDoubleClick={() => patch({ volume_db: 0 })} />
        <div className="meter" title={`${db.toFixed(1)} dBFS`}><div style={{ height: `${h}%` }} /></div>
      </div>
      <div className="pan"><input type="range" min={-1} max={1} step={0.05} value={v.pan} aria-label="Pan" title={`Pan ${v.pan === 0 ? "C" : v.pan < 0 ? `L${Math.round(-v.pan * 100)}` : `R${Math.round(v.pan * 100)}`}`} onChange={(e) => patch({ pan: Number(e.target.value) })} onDoubleClick={() => patch({ pan: 0 })} /></div>
      <div className="readout">{v.volume_db > 0 ? "+" : ""}{v.volume_db.toFixed(1)} dB</div>
      <div className="sm">
        <button className={`btn small ${t.muted ? "warn-on" : ""}`} onClick={() => run(api.updateTrack(t.id, { muted: !t.muted }))} aria-pressed={t.muted}>M</button>
        <button className={`btn small ${t.solo ? "solo-on" : ""}`} onClick={() => run(api.updateTrack(t.id, { solo: !t.solo }))} aria-pressed={!!t.solo}>S</button>
      </div>
    </div>
  );
}

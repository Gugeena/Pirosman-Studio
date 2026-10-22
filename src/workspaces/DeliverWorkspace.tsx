import { useEffect, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { Camera, CheckCircle2, FileOutput, FolderSearch, Loader2, Play, Square, TerminalSquare, XCircle } from "lucide-react";
import { exportOtioFlow, grabStillFlow } from "../lib/actions";
import { api, type RenderPlan, type RenderSettings } from "../lib/api";
import { attempt, errText, projectDuration, useUI } from "../lib/store";
import { bytes, timecode } from "../lib/time";
import { TimelineMonitor } from "../components/Monitors";

interface Preset { id: string; label: string; apply: (s: RenderSettings, w: number, h: number) => RenderSettings }
const PRESETS: Preset[] = [
  { id: "h264", label: "H.264 MP4 — delivery", apply: (s, w, h) => ({ ...s, width: w, height: h, video_codec: "h264", quality: 20, audio_codec: "aac", audio_bitrate_kbps: 192, output: swapExt(s.output, "mp4") }) },
  { id: "web720", label: "H.264 720p — review copy", apply: (s, w, h) => ({ ...s, width: even((w * 720) / h), height: 720, video_codec: "h264", quality: 24, audio_codec: "aac", audio_bitrate_kbps: 128, output: swapExt(s.output, "mp4") }) },
  { id: "prores", label: "ProRes 422 MOV — mastering", apply: (s, w, h) => ({ ...s, width: w, height: h, video_codec: "prores", audio_codec: "pcm", output: swapExt(s.output, "mov") }) },
  { id: "hevc", label: "H.265 / HEVC MP4 — small masters", apply: (s, w, h) => ({ ...s, width: w, height: h, video_codec: "hevc", quality: 24, audio_codec: "aac", audio_bitrate_kbps: 192, output: swapExt(s.output, "mp4") }) },
  { id: "dnxhr", label: "DNxHR HQ MOV — finishing intermediate", apply: (s, w, h) => ({ ...s, width: w, height: h, video_codec: "dnxhr", audio_codec: "pcm", output: swapExt(s.output, "mov") }) },
  { id: "mpeg4", label: "MPEG-4 Part 2 — legacy players", apply: (s, w, h) => ({ ...s, width: w, height: h, video_codec: "mpeg4", quality: 4, audio_codec: "aac", audio_bitrate_kbps: 192, output: swapExt(s.output, "mp4") }) },
  { id: "wav", label: "Audio only — WAV PCM", apply: (s) => ({ ...s, video_codec: "none", audio_codec: "pcm", output: swapExt(s.output, "wav") }) },
  { id: "m4a", label: "Audio only — AAC M4A", apply: (s) => ({ ...s, video_codec: "none", audio_codec: "aac", audio_bitrate_kbps: 256, output: swapExt(s.output, "m4a") }) },
];
const EXT: Record<RenderSettings["video_codec"], string[]> = { h264: ["mp4", "mov", "mkv"], hevc: ["mp4", "mov", "mkv"], mpeg4: ["mp4", "mov"], prores: ["mov"], dnxhr: ["mov", "mxf"], png: ["png"], none: ["wav", "m4a"] };
const even = (n: number) => Math.max(16, Math.round(n / 2) * 2);
function swapExt(p: string, ext: string) {
  return p.replace(/\.[A-Za-z0-9]+$/, "") + "." + ext;
}
const RATES: [string, number, number][] = [["23.976", 24000, 1001], ["24", 24, 1], ["25", 25, 1], ["29.97", 30000, 1001], ["30", 30, 1], ["50", 50, 1], ["59.94", 60000, 1001], ["60", 60, 1]];

export function DeliverWorkspace() {
  const view = useUI((s) => s.view);
  const render = useUI((s) => s.render);
  const set = useUI((s) => s.set);
  const [s, setS] = useState<RenderSettings | null>(null);
  const [preset, setPreset] = useState("h264");
  const [plan, setPlan] = useState<RenderPlan | null>(null);
  const [planErr, setPlanErr] = useState<string | null>(null);
  const [showCmd, setShowCmd] = useState(false);
  const p = view?.project;

  useEffect(() => {
    if (!p || s) return;
    const dir = view?.path ? view.path.replace(/[\\/][^\\/]+$/, "") : "";
    const sep = view?.path?.includes("\\") ? "\\" : "/";
    api.defaultRenderSettings(`${dir ? dir + sep : ""}${p.name.replace(/[^\w.-]+/g, "_")}.mp4`).then(setS);
  }, [p?.id]);

  // Validate and regenerate the FFmpeg plan as settings or the timeline change.
  useEffect(() => {
    if (!s) return;
    const t = setTimeout(() => {
      api.renderPlan(s).then((r) => { setPlan(r); setPlanErr(null); }).catch((e) => { setPlan(null); setPlanErr(errText(e)); });
    }, 150);
    return () => clearTimeout(t);
  }, [s, view]);

  if (!p || !s) return <div className="page"><div className="panel" /></div>;
  const running = render?.job === "export" && !render.result && !render.error;
  const up = (patch: Partial<RenderSettings>) => setS({ ...s, ...patch });
  const total = projectDuration(view);

  const exts = s.video_codec === "none" ? [s.audio_codec === "pcm" ? "wav" : "m4a"] : EXT[s.video_codec];
  const hasRange = p.mark_in != null || p.mark_out != null;
  const useRange = s.range_in != null || s.range_out != null;
  const browse = async () => {
    const ext = exts[0];
    const out = await save({ defaultPath: s.output, filters: [{ name: s.video_codec === "none" ? "Audio" : "Video", extensions: exts }] });
    if (out) up({ output: /\.[A-Za-z0-9]+$/.test(out) ? out : `${out}.${ext}` });
  };
  const start = async () => {
    const r = await attempt(api.startRender(s, false));
    if (r) set({ render: { job: "export", plan: r, progress: null, result: null, error: null } });
  };
  const rateIdx = RATES.findIndex(([, n, d]) => n * s.frame_rate.den === d * s.frame_rate.num);

  return (
    <div className="page" style={{ gridTemplateColumns: "340px minmax(0, 1fr) minmax(360px, 0.8fr)" }}>
      <div className="panel" aria-label="Render settings">
        <div className="panel-head"><span className="label">Render settings</span></div>
        <div className="panel-body">
          <div className="insp">
            <label className="field"><span>Preset</span>
              <select className="select" value={preset} onChange={(e) => { setPreset(e.target.value); setS(PRESETS.find((x) => x.id === e.target.value)!.apply(s, p.settings.width, p.settings.height)); }}>
                {PRESETS.map((x) => <option key={x.id} value={x.id}>{x.label}</option>)}
              </select>
            </label>
            <label className="field"><span>Output file</span>
              <div style={{ display: "flex", gap: 6 }}>
                <input className="input" value={s.output} onChange={(e) => up({ output: e.target.value })} title={s.output} />
                <button className="btn" onClick={browse}>Browse…</button>
              </div>
            </label>
            <section className="insp-sec">
              <span className="label">Video</span>
              <div className="grid2">
                <label className="field"><span>Codec</span>
                  <select className="select" value={s.video_codec} onChange={(e) => {
                    const v = e.target.value as RenderSettings["video_codec"];
                    const ext = v === "none" ? (s.audio_codec === "pcm" ? "wav" : "m4a") : EXT[v][0];
                    const intermediate = v === "prores" || v === "dnxhr";
                    up({ video_codec: v, quality: v === "mpeg4" ? 4 : v === "hevc" ? 24 : 20, output: EXT[v].some((x) => s.output.toLowerCase().endsWith("." + x)) ? s.output : swapExt(s.output, ext), audio_codec: v === "none" && s.audio_codec === "none" ? "aac" : intermediate && s.audio_codec === "aac" ? "pcm" : s.audio_codec });
                  }}>
                    <option value="h264">H.264 (libx264)</option><option value="hevc">H.265 / HEVC (libx265)</option><option value="mpeg4">MPEG-4 Part 2</option>
                    <option value="prores">Apple ProRes 422</option><option value="dnxhr">Avid DNxHR HQ</option><option value="none">No video (audio only)</option>
                  </select>
                </label>
                <label className="field"><span>Frame rate</span>
                  <select className="select" value={rateIdx} onChange={(e) => { const [, n, d] = RATES[Number(e.target.value)]; up({ frame_rate: { num: n, den: d } }); }}>
                    {rateIdx < 0 && <option value={-1}>{s.frame_rate.num}/{s.frame_rate.den}</option>}
                    {RATES.map(([l], i) => <option key={l} value={i}>{l} fps</option>)}
                  </select>
                </label>
                <label className="field"><span>Width</span><input className="input num" type="number" step={2} value={s.width} onChange={(e) => up({ width: Number(e.target.value) })} /></label>
                <label className="field"><span>Height</span><input className="input num" type="number" step={2} value={s.height} onChange={(e) => up({ height: Number(e.target.value) })} /></label>
                {(s.video_codec === "h264" || s.video_codec === "hevc" || s.video_codec === "mpeg4") && (
                  <label className="field"><span>{s.video_codec !== "mpeg4" ? "Quality (CRF, lower = better)" : "Quality (qscale 1–31)"}</span>
                    <input className="input num" type="number" value={s.quality} onChange={(e) => up({ quality: Number(e.target.value) })} />
                  </label>
                )}
              </div>
            </section>
            <section className="insp-sec">
              <span className="label">Audio</span>
              <div className="grid2">
                <label className="field"><span>Codec</span>
                  <select className="select" value={s.audio_codec} onChange={(e) => up({ audio_codec: e.target.value as RenderSettings["audio_codec"] })}>
                    <option value="aac">AAC</option><option value="pcm">PCM 16-bit</option><option value="none">No audio</option>
                  </select>
                </label>
                <label className="field"><span>Sample rate</span>
                  <select className="select" value={s.sample_rate} onChange={(e) => up({ sample_rate: Number(e.target.value) })}>
                    <option value={48000}>48 kHz</option><option value={44100}>44.1 kHz</option>
                  </select>
                </label>
                {s.audio_codec === "aac" && <label className="field"><span>Bitrate (kbps)</span><input className="input num" type="number" value={s.audio_bitrate_kbps} onChange={(e) => up({ audio_bitrate_kbps: Number(e.target.value) })} /></label>}
              </div>
            </section>
            <section className="insp-sec">
              <span className="label">Range</span>
              <div className="seg" role="group" aria-label="Render range">
                <button aria-pressed={!useRange} onClick={() => up({ range_in: null, range_out: null })}>Entire timeline</button>
                <button aria-pressed={useRange} disabled={!hasRange} title={hasRange ? "" : "Mark In (I) and Out (O) on the timeline first"} onClick={() => up({ range_in: p.mark_in ?? 0, range_out: p.mark_out ?? total })}>In to Out</button>
              </div>
              {useRange && <p className="note num">{timecode(s.range_in ?? 0, p.settings.frame_rate)} → {timecode(s.range_out ?? total, p.settings.frame_rate)} · {(s.range_out ?? total) - (s.range_in ?? 0)} frames</p>}
            </section>
            <label className="check"><input type="checkbox" checked={s.overwrite} onChange={(e) => up({ overwrite: e.target.checked })} />Replace the file if it already exists</label>
            {planErr && <p className="note err-text" role="alert">{planErr}</p>}
            {plan?.warnings.map((w) => <p key={w} className="note" style={{ color: "var(--warn)" }}>{w}</p>)}
            <div style={{ display: "flex", gap: 8 }}>
              {!running ? (
                <button className="btn primary" disabled={!plan} onClick={start}><Play />Render {timecode(plan?.duration_frames ?? total, p.settings.frame_rate)}</button>
              ) : (
                <button className="btn danger" onClick={() => api.cancelRender()}><Square />Cancel render</button>
              )}
            </div>
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
              <button className="btn ghost" onClick={exportOtioFlow} title="Write the cut as OpenTimelineIO for other editors"><FileOutput />Export OTIO…</button>
              <button className="btn ghost" onClick={() => grabStillFlow(useUI.getState().playhead)} disabled={total === 0}><Camera />Grab still</button>
            </div>
          </div>
        </div>
      </div>

      <TimelineMonitor title="Deliver" />

      <div className="panel" aria-label="Render job">
        <div className="panel-head"><span className="label">Job</span><span className="spacer" /><button className={`btn small ${showCmd ? "on" : "ghost"}`} onClick={() => setShowCmd(!showCmd)}><TerminalSquare />FFmpeg command</button></div>
        <div className="panel-body">
          <div className="insp">
            {plan && (
              <dl className="kv">
                <dt>Length</dt><dd>{timecode(plan.duration_frames, p.settings.frame_rate)} · {plan.duration_frames} f</dd>
                <dt>Video layers</dt><dd>{plan.video_clips} clip{plan.video_clips === 1 ? "" : "s"}</dd>
                <dt>Audio mixed</dt><dd>{plan.audio_clips} clip{plan.audio_clips === 1 ? "" : "s"}</dd>
                <dt>Output</dt><dd title={plan.output}>{plan.output.split(/[\\/]/).pop()}</dd>
              </dl>
            )}
            {render?.job === "export" && (
              <section className="insp-sec" aria-live="polite">
                <span className="label">{render.result ? (plan && render.result.output !== plan.output ? "Last render (different file)" : "Finished") : render.error ? "Failed" : "Rendering"}</span>
                {!render.result && !render.error && (
                  <>
                    <div className="progress"><div style={{ width: `${(render.progress?.fraction ?? 0) * 100}%` }} /></div>
                    <div className="note num" style={{ display: "flex", gap: 12 }}>
                      <Loader2 size={13} className="spin" />
                      <span>{Math.round((render.progress?.fraction ?? 0) * 100)}%</span>
                      {render.progress?.stage ? <span>{render.progress.stage}</span> : <span>{((render.progress?.out_time_us ?? 0) / 1e6).toFixed(1)} / {(render.plan.duration_us / 1e6).toFixed(1)} s</span>}
                      {render.progress?.speed && <span>{render.progress.speed}</span>}
                      {render.progress?.fps != null && <span>{render.progress.fps} fps</span>}
                    </div>
                    <p className="note">Progress is read from FFmpeg's <b>-progress</b> stream. The file is written as <b>…partial</b> and renamed only when FFmpeg succeeds.</p>
                  </>
                )}
                {render.result && (
                  <>
                    <p className="ok-text" style={{ display: "flex", gap: 8, alignItems: "center", margin: 0 }}><CheckCircle2 size={16} />Verified with ffprobe{render.seconds ? ` · rendered in ${render.seconds.toFixed(1)} s` : ""}</p>
                    <dl className="kv">
                      <dt>File</dt><dd className="text" style={{ whiteSpace: "normal", wordBreak: "break-all", userSelect: "text" }}>{render.result.output}</dd>
                      <dt>Size</dt><dd>{bytes(render.result.size)}</dd>
                      <dt>Duration</dt><dd>{render.result.duration_seconds.toFixed(3)} s</dd>
                      <dt>Video</dt><dd>{render.result.video_codec} {render.result.width}×{render.result.height} @ {render.result.frame_rate}</dd>
                      <dt>Audio</dt><dd>{render.result.audio_codec ?? "none"}</dd>
                    </dl>
                    <button className="btn" onClick={() => api.revealPath(render.result!.output)}><FolderSearch />Show in folder</button>
                  </>
                )}
                {render.error && <p className="err-text" style={{ display: "flex", gap: 8, margin: 0, userSelect: "text" }}><XCircle size={16} style={{ flex: "none" }} />{render.error}</p>}
              </section>
            )}
            {showCmd && plan && (
              <section className="insp-sec">
                <span className="label">Filtergraph</span>
                <pre className="cmd">{plan.filtergraph}</pre>
                <span className="label">Arguments</span>
                <pre className="cmd">ffmpeg {plan.args.map((a) => (/[\s;\[\]]/.test(a) ? `"${a}"` : a)).join(" ")}</pre>
              </section>
            )}
            {!plan && !planErr && <p className="note">Preparing render plan…</p>}
          </div>
        </div>
      </div>
    </div>
  );
}

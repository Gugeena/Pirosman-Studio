import { useEffect, useRef, useState } from "react";
import { api, type Scopes as ScopeData } from "../lib/api";
import { useUI } from "../lib/store";

/** Waveform (luma) and RGB parade of the exact rendered frame at the playhead. */
export function Scopes() {
  const view = useUI((s) => s.view);
  const playhead = useUI((s) => s.playhead);
  const playing = useUI((s) => s.playing);
  const [data, setData] = useState<ScopeData | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const luma = useRef<HTMLCanvasElement>(null);
  const parade = useRef<HTMLCanvasElement>(null);

  // Scopes come from FFmpeg (the export grade), so sample after the playhead settles.
  useEffect(() => {
    if (playing || !view) return;
    const t = setTimeout(() => {
      api.videoScopes(playhead).then((d) => { setData(d); setErr(null); }).catch((e) => setErr(String(e)));
    }, 180);
    return () => clearTimeout(t);
  }, [playhead, playing, view]);

  useEffect(() => {
    draw(luma.current, data, [["luma", "#f2efe9"]]);
    draw(parade.current, data, [["red", "#ff4a3d"], ["green", "#5fb59a"], ["blue", "#5f9be6"]]);
  }, [data]);

  return (
    <div className="panel" aria-label="Scopes">
      <div className="panel-head">
        <span className="label">Scopes</span>
        <span className="spacer" />
        {data && <span className="note num">{data.clipped_low > 0.005 ? `crushed ${(data.clipped_low * 100).toFixed(1)}% · ` : ""}{data.clipped_high > 0.005 ? `clipped ${(data.clipped_high * 100).toFixed(1)}%` : data.clipped_low > 0.005 ? "" : "in range"}</span>}
      </div>
      <div className="scopes" style={{ flex: 1, minHeight: 0 }}>
        <div className="scope"><span className="label">Waveform</span><canvas ref={luma} width={512} height={256} /></div>
        <div className="scope"><span className="label">RGB parade</span><canvas ref={parade} width={512} height={256} /></div>
      </div>
      {err && <p className="note err-text" style={{ padding: "4px 10px" }}>{err}</p>}
      {!data && !err && <p className="note" style={{ padding: "4px 10px" }}>{playing ? "Paused scopes update when playback stops." : "No picture at the playhead."}</p>}
    </div>
  );
}

/** Each series is a cols×levels density grid (column-major); parade draws series side by side. */
function draw(cv: HTMLCanvasElement | null, d: ScopeData | null, series: [keyof ScopeData, string][]) {
  if (!cv) return;
  const g = cv.getContext("2d")!;
  const W = cv.width, H = cv.height;
  g.fillStyle = "#03060c";
  g.fillRect(0, 0, W, H);
  g.strokeStyle = "rgba(148,166,196,.16)";
  g.lineWidth = 1;
  for (let i = 0; i <= 4; i++) { const y = Math.round((i / 4) * (H - 1)) + 0.5; g.beginPath(); g.moveTo(0, y); g.lineTo(W, y); g.stroke(); }
  if (!d) return;
  const sw = W / series.length;
  g.globalCompositeOperation = "lighter";
  series.forEach(([key, color], si) => {
    const grid = d[key] as number[];
    let max = 0;
    for (const v of grid) max = Math.max(max, v);
    if (!max) return;
    const cw = sw / d.cols, lh = H / d.levels;
    g.fillStyle = color;
    for (let c = 0; c < d.cols; c++) {
      for (let l = 0; l < d.levels; l++) {
        const v = grid[c * d.levels + l];
        if (!v) continue;
        g.globalAlpha = Math.min(1, 0.12 + Math.sqrt(v / max) * 0.95);
        g.fillRect(si * sw + c * cw, H - (l + 1) * lh, Math.max(1, cw), Math.max(1, lh));
      }
    }
  });
  g.globalAlpha = 1;
  g.globalCompositeOperation = "source-over";
}

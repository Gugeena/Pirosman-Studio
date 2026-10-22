import { FolderSearch, Link2, Trash2 } from "lucide-react";
import { api } from "../lib/api";
import { relinkFlow } from "../lib/actions";
import { infoMap, mediaCode, run, useUI } from "../lib/store";
import { bytes, shortDuration, timecode } from "../lib/time";
import { MediaPool } from "../components/MediaPool";
import { SourceMonitor } from "../components/Monitors";
import { Split } from "../components/Split";

function MediaDetails() {
  const view = useUI((s) => s.view);
  const id = useUI((s) => s.sourceMediaId);
  const m = view?.project.media.find((x) => x.id === id);
  if (!view) return null;
  const info = id ? infoMap(view).get(id) : undefined;
  const rate = view.project.settings.frame_rate;
  const used = m ? view.project.tracks.flatMap((t) => t.clips).filter((c) => c.media_id === m.id).length : 0;
  return (
    <div className="panel" aria-label="Media details">
      <div className="panel-head"><span className="label">Metadata{m && <> · <b>{mediaCode(view, m.id)}</b></>}</span></div>
      <div className="panel-body">
        {!m ? (
          <div className="empty"><p>Select a clip in the media pool to see its streams, content hash and status.</p></div>
        ) : (
          <div className="insp">
            <section className="insp-sec">
              <span className="label">File</span>
              <dl className="kv">
                <dt>Name</dt><dd className="text">{m.name}</dd>
                <dt>Status</dt><dd className="text" style={{ color: info?.status === "online" ? "var(--ok)" : info?.status === "missing" ? "var(--err)" : "var(--warn)" }}>{info?.status === "online" ? "Online" : info?.status === "missing" ? "Offline" : "Changed on disk"}</dd>
                <dt>Size</dt><dd>{bytes(m.size)}</dd>
                <dt>Container</dt><dd>{(m.path.split(".").pop() ?? "").toUpperCase() || "—"}</dd>
                <dt>Demuxer</dt><dd title={m.container ?? ""}>{m.container?.split(",")[0] ?? "—"}</dd>
                <dt>Used by</dt><dd className="text">{used} clip{used === 1 ? "" : "s"}</dd>
              </dl>
              <div className="note" style={{ userSelect: "text", wordBreak: "break-all" }}>{m.path}</div>
            </section>
            <section className="insp-sec">
              <span className="label">Streams</span>
              <dl className="kv">
                <dt>Kind</dt><dd className="text">{m.kind}</dd>
                {m.duration_frames != null && <><dt>Duration</dt><dd>{timecode(m.duration_frames, rate)} ({shortDuration(m.duration_frames, rate)})</dd></>}
                {m.has_video && <><dt>Video</dt><dd>{m.video_codec} {m.width}×{m.height}</dd></>}
                {m.frame_rate && <><dt>Source rate</dt><dd>{m.frame_rate.num}/{m.frame_rate.den} ({(m.frame_rate.num / m.frame_rate.den).toFixed(3)})</dd></>}
                {m.has_audio && <><dt>Audio</dt><dd>{m.audio_codec} {m.sample_rate} Hz · {m.channels} ch</dd></>}
                <dt>Preview</dt><dd className="text">{info?.needs_proxy ? (info.preview ? "H.264 proxy" : "Proxy building…") : "Original file"}</dd>
              </dl>
              {m.frame_rate && (m.frame_rate.num / m.frame_rate.den).toFixed(3) !== (rate.num / rate.den).toFixed(3) && (
                <p className="note">Source rate differs from the timeline ({(rate.num / rate.den).toFixed(3)} fps). Export conforms it with FFmpeg's <b>fps</b> filter (frames are dropped or repeated, not interpolated).</p>
              )}
            </section>
            <section className="insp-sec">
              <span className="label">Content identity</span>
              <div className="note num" style={{ userSelect: "text", wordBreak: "break-all" }}>SHA-256 {m.sha256 ?? "not computed"}</div>
              <p className="note">Revisions reference media by this hash; relinking only accepts a file with identical content unless you explicitly choose otherwise.</p>
            </section>
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
              <button className="btn" onClick={() => relinkFlow(m.id)}><Link2 />Relink…</button>
              {info?.status !== "missing" && <button className="btn ghost" onClick={() => api.revealPath(m.path)}><FolderSearch />Show in folder</button>}
              <button className="btn danger" disabled={used > 0} title={used ? "Remove its clips from the timeline first" : "Remove from project"} onClick={() => run(api.removeMedia(m.id))}><Trash2 />Remove</button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

export function MediaWorkspace() {
  return (
    <div className="page">
      <Split id="media-h" dir="row" initial={[36, 42, 22]} min={260}>
        <MediaPool />
        <SourceMonitor />
        <MediaDetails />
      </Split>
    </div>
  );
}

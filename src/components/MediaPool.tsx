import { cssUrl } from "../lib/files";
import { ArrowDown, ArrowUp, AudioLines, Film, FolderInput, Image as ImageIcon, LayoutGrid, Link2Off, List, ListVideo, Loader2, Search } from "lucide-react";
import { useMemo, useState } from "react";
import { importFlow, importTimelineFlow, relinkFlow } from "../lib/actions";
import { api, type MediaRef } from "../lib/api";
import { showContextMenu } from "../lib/contextMenu";
import { beginDrag } from "../lib/dnd";
import { fpsLabel } from "../lib/presets";
import { infoMap, mediaCode, prefs, run, useUI } from "../lib/store";
import { shortDuration } from "../lib/time";

type Col = "name" | "duration" | "codec" | "fps" | "size";
type Kind = "all" | "video" | "audio" | "image";

const codecOf = (m: MediaRef) => (m.has_video ? m.video_codec : m.audio_codec) ?? "";
const fpsOf = (m: MediaRef) => (m.frame_rate ? m.frame_rate.num / m.frame_rate.den : 0);

export function MediaPool({ title = "Media Pool", switcher }: { title?: string; switcher?: React.ReactNode }) {
  const view = useUI((s) => s.view);
  const sel = useUI((s) => s.sourceMediaId);
  const set = useUI((s) => s.set);
  const infos = useMemo(() => infoMap(view), [view]);
  const media = view?.project.media ?? [];
  const rate = view?.project.settings.frame_rate ?? { num: 25, den: 1 };
  const [layout, setLayout] = useState(prefs.get("poolLayout", "grid"));
  const [q, setQ] = useState("");
  const [kind, setKind] = useState<Kind>("all");
  const [sort, setSort] = useState<{ col: Col; dir: 1 | -1 }>({ col: "name", dir: 1 });
  const [hot, setHot] = useState(false);

  const shown = useMemo(() => {
    const f = media.filter((m) => (kind === "all" || m.kind === kind) && (!q || m.name.toLowerCase().includes(q.toLowerCase()) || codecOf(m).includes(q.toLowerCase())));
    if (layout !== "list") return f;
    const key = (m: MediaRef): string | number =>
      sort.col === "name" ? m.name.toLowerCase() : sort.col === "duration" ? m.duration_frames ?? -1 : sort.col === "codec" ? codecOf(m) : sort.col === "fps" ? fpsOf(m) : (m.width ?? 0) * (m.height ?? 0);
    return [...f].sort((a, b) => (key(a) < key(b) ? -1 : key(a) > key(b) ? 1 : 0) * sort.dir);
  }, [media, q, kind, layout, sort]);

  const pickLayout = (l: string) => { setLayout(l); prefs.set("poolLayout", l); };
  const choose = (m: MediaRef) => set({ sourceMediaId: m.id, markIn: null, markOut: null });

  const menu = (e: React.MouseEvent, m: MediaRef) => {
    const missing = infos.get(m.id)?.status === "missing";
    const used = view!.project.tracks.some((t) => t.clips.some((c) => c.media_id === m.id));
    const atPlayhead = (detail: string) => { choose(m); setTimeout(() => window.dispatchEvent(new CustomEvent("pirosman-source", { detail })), 0); };
    showContextMenu(e, [
      { text: "Open in source viewer", action: () => choose(m) },
      { text: "Insert at playhead", accelerator: "F9", enabled: !missing, action: () => atPlayhead("insert") },
      { text: "Overwrite at playhead", accelerator: "F10", enabled: !missing, action: () => atPlayhead("overwrite") },
      "sep",
      { text: missing ? "Relink missing media…" : "Relink…", action: () => relinkFlow(m.id) },
      { text: "Show in folder", enabled: !missing, action: () => api.revealPath(m.path) },
      { text: "Copy file path", action: () => navigator.clipboard.writeText(m.path) },
      "sep",
      { text: used ? "Remove from project (in use)" : "Remove from project", enabled: !used, action: () => run(api.removeMedia(m.id)) },
      "sep",
      { text: "Import media…", accelerator: "Ctrl+I", action: importFlow },
    ]);
  };
  // Drag onto a timeline track to overwrite there (pointer-driven; see lib/dnd).
  const dragProps = (m: MediaRef) => ({
    draggable: false,
    onPointerDown: (e: React.PointerEvent) => {
      if (infos.get(m.id)?.status !== "missing") beginDrag(e, { kind: "media", id: m.id, label: m.name, frames: m.duration_frames });
    },
  });
  const th = (col: Col, label: string, right = false) => (
    <th className={right ? "r" : ""} aria-sort={sort.col === col ? (sort.dir === 1 ? "ascending" : "descending") : "none"}>
      <button className="th-sort" onClick={() => setSort((s) => ({ col, dir: s.col === col ? (s.dir === 1 ? -1 : 1) : 1 }))}>
        {label}{sort.col === col && (sort.dir === 1 ? <ArrowUp size={11} /> : <ArrowDown size={11} />)}
      </button>
    </th>
  );

  return (
    <div className="panel" aria-label="Media pool">
      <div className="panel-head">
        {switcher ?? <span className="label">{title} · <b>{media.length}</b></span>}
        <span className="spacer" />
        <button className="btn icon small ghost" onClick={importTimelineFlow} title="Open an FCP 7 XML, OpenTimelineIO, or Resolve .drp/.drt timeline as a new project" aria-label="Import timeline"><ListVideo /></button>
        <button className="btn small" onClick={importFlow} title="Import media (Ctrl+I)"><FolderInput />Import</button>
      </div>
      {media.length > 0 && (
        <div className="pool-bar">
          <label className="search"><Search /><input className="input" placeholder="Search" value={q} onChange={(e) => setQ(e.target.value)} aria-label="Search media" /></label>
          <select className="select small" value={kind} onChange={(e) => setKind(e.target.value as Kind)} aria-label="Media kind">
            <option value="all">All</option><option value="video">Video</option><option value="audio">Audio</option><option value="image">Stills</option>
          </select>
          <div className="seg" role="group" aria-label="Layout">
            <button aria-pressed={layout === "grid"} onClick={() => pickLayout("grid")} title="Thumbnails"><LayoutGrid size={12} /></button>
            <button aria-pressed={layout === "list"} onClick={() => pickLayout("list")} title="List with sortable columns"><List size={12} /></button>
          </div>
        </div>
      )}
      <div className="panel-body">
        {media.length === 0 ? (
          <button
            className={`drop-zone ${hot ? "hot" : ""}`} onClick={importFlow}
            onDragEnter={() => setHot(true)} onDragLeave={() => setHot(false)} onDrop={() => setHot(false)}
          >
            <Film size={20} strokeWidth={1.5} />
            <b>Drop media or press <kbd className="k">Ctrl</kbd>+<kbd className="k">I</kbd></b>
            <span className="tech">SHA-256 fingerprint · referenced in place · zero copy</span>
          </button>
        ) : shown.length === 0 ? (
          <p className="note" style={{ padding: 10 }}>Nothing matches.</p>
        ) : layout === "list" ? (
          <table className="tbl pool-table" role="listbox" aria-label="Media">
            <thead><tr>{th("name", "Name")}{th("duration", "Duration", true)}{th("codec", "Codec")}{th("fps", "FPS", true)}{th("size", "Size", true)}</tr></thead>
            <tbody>
              {shown.map((m) => {
                const info = infos.get(m.id);
                const Icon = m.kind === "video" ? Film : m.kind === "audio" ? AudioLines : ImageIcon;
                return (
                  <tr
                    key={m.id} role="option" aria-selected={sel === m.id} className={info?.status === "missing" ? "gone" : ""} {...dragProps(m)}
                    onClick={() => choose(m)} onDoubleClick={() => info?.status === "missing" && relinkFlow(m.id)} onContextMenu={(e) => menu(e, m)} title={m.path}
                  >
                    <td className="strong"><span className="cell-name"><Icon size={13} /><span className="code num">{mediaCode(view, m.id)}</span>{m.name}{info?.status === "missing" && <Link2Off size={12} className="err-text" />}</span></td>
                    <td className="num r">{m.duration_frames != null ? shortDuration(m.duration_frames, rate) : "—"}</td>
                    <td className="num">{codecOf(m) || "—"}</td>
                    <td className="num r">{m.frame_rate ? fpsLabel(m.frame_rate.num, m.frame_rate.den) : "—"}</td>
                    <td className="num r">{m.width && m.height ? `${m.width}×${m.height}` : "—"}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        ) : (
          <div className="pool-grid" role="listbox" aria-label="Media">
            {shown.map((m) => {
              const info = infos.get(m.id);
              const missing = info?.status === "missing";
              const Icon = m.kind === "video" ? Film : m.kind === "audio" ? AudioLines : ImageIcon;
              return (
                <button
                  key={m.id} className="pool-cell" role="option" aria-selected={sel === m.id} {...dragProps(m)}
                  onClick={() => choose(m)} onDoubleClick={() => missing && relinkFlow(m.id)} onContextMenu={(e) => menu(e, m)} title={`${m.name}\n${m.path}`}
                >
                  <div className="pool-thumb" style={info?.thumb ? { backgroundImage: cssUrl(info.thumb) } : undefined}>
                    {m.kind === "audio" && info?.wave && <div className="wave" style={{ backgroundImage: cssUrl(info.wave) }} />}
                    <span className="kind"><Icon /></span>
                    {m.duration_frames != null && <span className="dur">{shortDuration(m.duration_frames, rate)}</span>}
                    {missing && <div className="flag"><Link2Off size={16} />Offline</div>}
                    {!missing && info?.needs_proxy && !info.preview && <div className="flag" style={{ color: "var(--marker)" }}><Loader2 size={16} className="spin" />Proxy</div>}
                    {info?.status === "modified" && <div className="flag" style={{ color: "var(--warn)", background: "rgba(5,10,20,.5)" }}>Changed on disk</div>}
                  </div>
                  <div className="pool-tag">
                    <span className="code">{mediaCode(view, m.id)}</span>
                    <span className="name">{m.name}</span>
                  </div>
                </button>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}

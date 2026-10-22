import { useEffect, useMemo, useState } from "react";
import { fileSrc } from "../lib/files";
import { listen } from "@tauri-apps/api/event";
import { ask } from "@tauri-apps/plugin-dialog";
import {
  ArrowLeft, ArrowUpRight, FolderOpen, GitCommitVertical, HardDrive, Info, Keyboard, LayoutGrid, Link2Off, List, MoreHorizontal, PackageOpen, Plus, Search, Settings, Stethoscope,
} from "lucide-react";
import { api, type HubProject, type RecentRevision, type StorageReport } from "../lib/api";
import { importArchiveFlow, openPathFlow, openProjectFlow } from "../lib/actions";
import { showContextMenu } from "../lib/contextMenu";
import { attempt, prefs, promptText, useUI, type HubSection } from "../lib/store";
import { setMyName } from "../lib/collab";
import { PRESETS, fpsLabel } from "../lib/presets";
import { bytes, relTime, shortDuration } from "../lib/time";
import { BrandMark } from "../components/Chrome";
import { FrameShape } from "../components/FrameShape";

const NAV: { id: HubSection; label: string; icon: typeof FolderOpen }[] = [
  { id: "projects", label: "Projects", icon: FolderOpen },
  { id: "revisions", label: "Recent revisions", icon: GitCommitVertical },
  { id: "storage", label: "Storage", icon: HardDrive },
  { id: "settings", label: "Settings", icon: Settings },
];

const iso = (secs: number | null) => (secs ? new Date(secs * 1000).toISOString() : "");
const APP_VERSION = "0.1.0-alpha";

/** Render engine and plugin readiness, read once when the hub opens. */
function EngineStatus() {
  const ofx = useUI((s) => s.ofx);
  const [ff, setFf] = useState<{ ok: boolean; text: string } | null>(null);
  useEffect(() => {
    api.ffmpegStatus()
      .then((t) => setFf({ ok: t.found, text: t.found ? `FFmpeg ${((t.ffmpeg_version ?? "").split(" ")[2] ?? "").split("-")[0]} ready`.replace("  ", " ") : "FFmpeg missing" }))
      .catch(() => setFf({ ok: false, text: "FFmpeg unknown" }));
  }, []);
  const fx = ofx?.plugins.filter((p) => p.supported).length;
  return (
    <div className="hub-engine" aria-label="Engine status">
      <span><i className={ff ? (ff.ok ? "ok" : "bad") : ""} />{ff?.text ?? "Checking FFmpeg…"}</span>
      <span><i className={ofx ? "ok" : ""} />{ofx ? `${fx} OpenFX effect${fx === 1 ? "" : "s"}` : "Scanning OpenFX…"}</span>
    </div>
  );
}

export function ProjectHub() {
  const section = useUI((s) => s.hubSection);
  const view = useUI((s) => s.view);
  const set = useUI((s) => s.set);
  const hasWork = !!view && (!!view.path || view.project.media.length > 0 || view.unsaved);

  return (
    <div className="hub screen-enter">
      <aside className="hub-side" aria-label="Project Hub">
        <div className="hub-brand">
          <BrandMark size={30} />
          <div><b>Pirosman</b><span>Studio</span><span className="ver" title="Media is fingerprinted with SHA-256 and referenced in place, never copied">v{APP_VERSION} · SHA-256</span></div>
        </div>
        <div className="hub-side-actions">
          <button className="btn primary" onClick={() => set({ dialog: { kind: "new" } })} title="New project (Ctrl+N)"><Plus />New project</button>
          <button className="btn" onClick={openProjectFlow} title="Open a .pirosman project (Ctrl+O)"><FolderOpen />Open project…</button>
        </div>
        {hasWork && view && (
          <button className="hub-resume" onClick={() => set({ screen: "editor" })} title="Return to the editor without closing anything">
            <ArrowLeft size={14} />
            <span><span className="label">Open in editor</span><b>{view.project.name}</b></span>
            {view.unsaved && <span className="unsaved-dot" aria-label="Unsaved changes" />}
          </button>
        )}
        <nav className="hub-nav">
          {NAV.map(({ id, label, icon: Icon }) => (
            <button key={id} aria-current={section === id ? "page" : undefined} onClick={() => set({ hubSection: id })}><Icon />{label}</button>
          ))}
        </nav>
        <span className="spacer" />
        <EngineStatus />
        <div className="hub-side-foot">
          <button className="btn small ghost" onClick={() => set({ dialog: { kind: "shortcuts" } })}><Keyboard />Shortcuts</button>
          <button className="btn small ghost" onClick={() => set({ dialog: { kind: "diagnostics" } })}><Stethoscope />Diagnostics</button>
          <button className="btn small ghost" onClick={() => set({ dialog: { kind: "about" } })}><Info />About</button>
        </div>
      </aside>
      <main className="hub-main">
        {section === "projects" && <ProjectsSection />}
        {section === "revisions" && <RevisionsSection />}
        {section === "storage" && <StorageSection />}
        {section === "settings" && <SettingsSection />}
      </main>
    </div>
  );
}

// ------------------------------------------------------------------ projects

type Sort = "modified" | "name" | "duration";

function useHubProjects() {
  const [list, setList] = useState<HubProject[] | null>(null);
  const load = () => api.hubProjects().then(setList).catch(() => setList([]));
  useEffect(() => {
    void load();
    const off = listen("hub-updated", () => void load());
    return () => { off.then((f) => f()); };
  }, []);
  return { list, load };
}

function ProjectsSection() {
  const set = useUI((s) => s.set);
  const { list, load } = useHubProjects();
  const [layout, setLayout] = useState(prefs.get("hubLayout", "grid"));
  const [q, setQ] = useState("");
  const [sort, setSort] = useState<Sort>("modified");
  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const hay = (p: HubProject) => `${p.name} ${p.path} ${p.width}x${p.height} ${p.width}×${p.height} ${fpsLabel(p.fps_num, p.fps_den)}fps ${p.branch ?? ""}`.toLowerCase();
    const f = (list ?? []).filter((p) => !needle || hay(p).includes(needle));
    if (sort === "name") return [...f].sort((a, b) => a.name.localeCompare(b.name));
    if (sort === "duration") return [...f].sort((a, b) => b.duration_frames / (b.fps_num / b.fps_den || 1) - a.duration_frames / (a.fps_num / a.fps_den || 1));
    return f; // the recents list is already most recent first
  }, [list, q, sort]);
  const pickLayout = (l: string) => { setLayout(l); prefs.set("hubLayout", l); };

  const menu = (e: React.MouseEvent, p: HubProject) => {
    e.preventDefault();
    e.stopPropagation();
    showContextMenu(e, [
      { text: p.open ? "Return to editor" : "Open", enabled: p.exists && !p.error, action: () => openPathFlow(p.path) },
      "sep",
      { text: "Duplicate", enabled: p.exists && !p.error, action: async () => { if (await attempt(api.hubDuplicate(p.path))) { useUI.getState().toast("ok", `Duplicated ${p.name}`); load(); } } },
      {
        text: "Rename…", enabled: p.exists && !p.error,
        action: () => promptText("Rename project", "Name (the file and its revision history are renamed too)", p.name, "Rename", async (n) => {
          const r = await attempt(api.hubRename(p.path, n));
          if (r) {
            if (p.open) useUI.getState().setView(await api.getView());
            load();
          }
        }),
      },
      { text: "Show in folder", enabled: p.exists, action: () => attempt(api.revealPath(p.path)) },
      { text: "Copy path", action: () => navigator.clipboard.writeText(p.path) },
      "sep",
      { text: "Remove from recents", action: async () => { await attempt(api.hubForget(p.path)); load(); } },
      {
        text: "Move to Recycle Bin…", enabled: p.exists && !p.open,
        action: async () => {
          const yes = await ask(`Move “${p.name}” to the Recycle Bin?\n\nThe project file, its backup and its revision history are moved together. Media files are not touched. You can restore them from the Recycle Bin.`, { title: "Move project to Recycle Bin", kind: "warning", okLabel: "Move to Recycle Bin", cancelLabel: "Cancel" });
          if (yes && (await attempt(api.hubTrash(p.path))) !== undefined) { useUI.getState().toast("ok", `${p.name} moved to the Recycle Bin`); load(); }
        },
      },
    ]);
  };

  return (
    <div className="hub-page">
      <header className="hub-head">
        <h1>Projects</h1>
        <div className="hub-actions">
          <button className="btn big" onClick={importArchiveFlow} title="Create a project, with its revision history, from a .pirosman-bundle"><PackageOpen />Import archive…</button>
        </div>
      </header>

      <section className="hub-sec" aria-label="Start from a preset">
        <div className="hub-sec-head"><span className="label">Start from a preset</span></div>
        <div className="preset-row wide">
          {PRESETS.map((x) => (
            <button key={x.id} className="preset" onClick={() => set({ dialog: { kind: "new", preset: x.id } })}>
              <FrameShape width={x.width} height={x.height} box={44} />
              <span><b>{x.name}</b><span className="num">{x.width}×{x.height} · {fpsLabel(x.num, x.den)} fps</span><span className="use">{x.use}</span></span>
            </button>
          ))}
          <button className="preset" onClick={() => set({ dialog: { kind: "new", preset: "custom" } })}>
            <FrameShape width={4} height={3} box={44} dashed />
            <span><b>Custom project…</b><span className="num">23.976 – 60 fps · 48 kHz</span><span className="use">Name, resolution and frame rate</span></span>
          </button>
        </div>
      </section>

      <section className="hub-sec" aria-label="Recent projects">
        <div className="hub-sec-head">
          <span className="label">Recent projects{list && <> · <b>{list.length}</b></>}</span>
          <span className="spacer" />
          <label className="search"><Search /><input className="input" placeholder="Filter by name, resolution or branch" value={q} onChange={(e) => setQ(e.target.value)} aria-label="Filter projects" /></label>
          <select className="select small" value={sort} onChange={(e) => setSort(e.target.value as Sort)} aria-label="Sort projects">
            <option value="modified">Recently opened</option><option value="name">Name</option><option value="duration">Duration</option>
          </select>
          <div className="seg" role="group" aria-label="Layout">
            <button aria-pressed={layout === "grid"} onClick={() => pickLayout("grid")} title="Grid"><LayoutGrid size={13} /></button>
            <button aria-pressed={layout === "list"} onClick={() => pickLayout("list")} title="List"><List size={13} /></button>
          </div>
        </div>
        {list === null && <p className="note">Reading recent projects…</p>}
        {list && list.length === 0 && (
          <div className="hub-empty">
            <p><b>No projects yet.</b> Pick a preset above, open a <code>.pirosman</code> file, or drop media onto this window to start cutting straight away; files are fingerprinted with SHA-256 and referenced in place, never copied.</p>
          </div>
        )}
        {list && list.length > 0 && shown.length === 0 && <p className="note">No project matches “{q}”.</p>}
        {layout === "grid" ? (
          <div className="proj-grid">
            {shown.map((p) => (
              <div key={p.path} className={`proj-card ${p.exists && !p.error ? "" : "gone"}`} onContextMenu={(e) => menu(e, p)}>
                <button className="proj-open" onClick={() => p.exists && !p.error && openPathFlow(p.path)} disabled={!p.exists || !!p.error} title={p.path}>
                  <span className="proj-thumb">
                    {p.thumb ? <img key={p.modified ?? 0} src={fileSrc(p.thumb)} alt="" /> : <span className="proj-nothumb"><FrameShape width={p.width || 16} height={p.height || 9} box={38} dashed /></span>}
                    {!p.exists && <span className="proj-flag"><Link2Off size={14} />File missing</span>}
                    {p.error && <span className="proj-flag">Unreadable</span>}
                    {p.open && <span className="proj-open-tag">Open</span>}
                    {p.exists && !p.error && <span className="dur num">{shortDuration(p.duration_frames, { num: p.fps_num, den: p.fps_den || 1 })}</span>}
                  </span>
                  <span className="proj-name">{p.name}</span>
                  <span className="proj-meta">{p.modified ? relTime(iso(p.modified)) : "—"}{p.branch && <> · {p.branch}{p.revisions ? ` · ${p.revisions} rev` : ""}</>}</span>
                  {p.exists && !p.error && (
                    <span className="proj-badges num">
                      <span>{p.width}×{p.height} · {fpsLabel(p.fps_num, p.fps_den)}</span>
                      <span>{p.clip_count} clip{p.clip_count === 1 ? "" : "s"}</span>
                      <span className={p.revisions ? "rev" : ""}>{p.revisions ? `${p.revisions} revision${p.revisions === 1 ? "" : "s"}` : "no history"}</span>
                    </span>
                  )}
                </button>
                <button className="btn icon small ghost proj-more" aria-label={`More actions for ${p.name}`} onClick={(e) => menu(e, p)}><MoreHorizontal /></button>
              </div>
            ))}
          </div>
        ) : (
          shown.length > 0 && (
            <table className="tbl proj-table">
              <thead><tr><th style={{ width: 64 }} /><th>Name</th><th>Opened</th><th>Duration</th><th>Format</th><th>History</th><th style={{ width: 34 }} /></tr></thead>
              <tbody>
                {shown.map((p) => (
                  <tr key={p.path} className={p.exists && !p.error ? "" : "gone"} onClick={() => p.exists && !p.error && openPathFlow(p.path)} onContextMenu={(e) => menu(e, p)} title={p.path}>
                    <td><span className="proj-mini">{p.thumb ? <img key={p.modified ?? 0} src={fileSrc(p.thumb)} alt="" /> : <FrameShape width={p.width || 16} height={p.height || 9} box={20} dashed />}</span></td>
                    <td className="strong">{p.name}{p.open && <span className="proj-open-tag inline">Open</span>}<div className="path">{p.exists ? p.path : "File missing: " + p.path}</div></td>
                    <td>{p.modified ? relTime(iso(p.modified)) : "—"}</td>
                    <td className="num">{p.exists && !p.error ? shortDuration(p.duration_frames, { num: p.fps_num, den: p.fps_den || 1 }) : "—"}</td>
                    <td className="num">{p.exists && !p.error ? `${p.width}×${p.height} · ${fpsLabel(p.fps_num, p.fps_den)}` : "—"}</td>
                    <td>{p.branch ? `${p.branch} · ${p.revisions}` : "—"}</td>
                    <td><button className="btn icon small ghost" aria-label={`More actions for ${p.name}`} onClick={(e) => menu(e, p)}><MoreHorizontal /></button></td>
                  </tr>
                ))}
              </tbody>
            </table>
          )
        )}
      </section>
    </div>
  );
}

// ------------------------------------------------------------------ revisions

function RevisionsSection() {
  const set = useUI((s) => s.set);
  const [list, setList] = useState<RecentRevision[] | null>(null);
  useEffect(() => { api.recentRevisions(40).then(setList).catch(() => setList([])); }, []);
  return (
    <div className="hub-page">
      <header className="hub-head"><h1>Recent revisions</h1><p className="note">The newest commits across your recent projects. Open one to compare, branch or restore it.</p></header>
      {list === null && <p className="note">Reading revision history…</p>}
      {list && list.length === 0 && <div className="hub-empty"><p>No revisions yet. Save a project, then describe your changes and press <b>Commit</b> on its Revisions page.</p></div>}
      {list && list.length > 0 && (
        <ul className="rev-feed">
          {list.map((r) => (
            <li key={`${r.project_path}:${r.id}`}>
              <button onClick={async () => { if (await openPathFlow(r.project_path)) set({ page: "revisions" }); }}>
                <span className="rev-dot" aria-hidden="true" />
                <span className="rev-body">
                  <span className="msg">{r.message}</span>
                  <span className="meta"><b>{r.project_name}</b>{r.branches.map((b) => <span key={b} className="bref">{b}</span>)}<span>{r.author || "unknown"}</span><span>{relTime(r.created_at)}</span><span className="num">{r.id.slice(0, 7)}</span></span>
                </span>
                <ArrowUpRight size={14} className="go" />
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ------------------------------------------------------------------ storage

function StorageSection() {
  const [r, setR] = useState<StorageReport | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const load = () => api.storageReport().then(setR).catch(() => setR(null));
  useEffect(() => { void load(); }, []);
  const clear = async (key: string, label: string) => {
    const yes = await ask(`Delete all ${label.toLowerCase()}?\n\nThey are rebuilt automatically when needed; projects and media are not affected.`, { title: "Clear cache", kind: "warning", okLabel: "Clear", cancelLabel: "Cancel" });
    if (!yes) return;
    setBusy(key);
    if ((await attempt(api.storageClear(key))) !== undefined) useUI.getState().toast("ok", `${label} cleared`);
    setBusy(null);
    void load();
  };
  const max = Math.max(1, ...(r?.items.map((i) => i.bytes) ?? [1]));
  return (
    <div className="hub-page">
      <header className="hub-head">
        <h1>Storage</h1>
        <p className="note">Pirosman keeps rebuildable working files in one cache folder. Projects, revision history and your media live elsewhere and are never touched here.</p>
      </header>
      {!r ? <p className="note">Measuring the cache…</p> : (
        <>
          <div className="store-total">
            <span className="num big">{bytes(r.total)}</span>
            <span className="note">in {r.cache_dir}</span>
            <button className="btn small" onClick={() => attempt(api.revealPath(r.cache_dir))}><FolderOpen />Show folder</button>
          </div>
          <table className="tbl store-table">
            <thead><tr><th>Category</th><th style={{ width: "34%" }}>Share</th><th className="r">Files</th><th className="r">Size</th><th style={{ width: 90 }} /></tr></thead>
            <tbody>
              {r.items.map((i) => (
                <tr key={i.key}>
                  <td className="strong">{i.label}<div className="path">{i.detail}</div></td>
                  <td><span className="share"><span style={{ width: `${(i.bytes / max) * 100}%` }} /></span></td>
                  <td className="num r">{i.files.toLocaleString()}</td>
                  <td className="num r">{bytes(i.bytes)}</td>
                  <td className="r">{i.clearable && <button className="btn small ghost danger" disabled={!i.bytes || busy === i.key} onClick={() => clear(i.key, i.label)}>{busy === i.key ? "Clearing…" : "Clear"}</button>}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <p className="note">Settings: <span className="num">{r.config_dir}</span></p>
        </>
      )}
    </div>
  );
}

// ------------------------------------------------------------------ settings

function SettingsSection() {
  const author = useUI((s) => s.author);
  const set = useUI((s) => s.set);
  const [name, setName] = useState(author);
  const [start, setStart] = useState(prefs.get("startScreen", "hub"));
  const [ff, setFf] = useState<string | null>(null);
  useEffect(() => { api.ffmpegStatus().then((t) => setFf(t.found ? `${t.source === "bundled" ? "Bundled" : t.source === "override" ? "Override" : "System"} · ${t.ffmpeg_version?.split(" ")[2] ?? ""}` : "Not available")).catch(() => setFf(null)); }, []);
  const saveName = () => {
    const n = name.trim();
    prefs.set("author", n);
    set({ author: n });
    setMyName(n);
  };
  return (
    <div className="hub-page narrow">
      <header className="hub-head"><h1>Settings</h1></header>
      <div className="set-list">
        <div className="set-row">
          <div><b>Your name</b><p className="note">Signs your revisions and labels your cursor in Live Sync.</p></div>
          <input className="input" value={name} placeholder="e.g. Nino" onChange={(e) => setName(e.target.value)} onBlur={saveName} onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()} aria-label="Your name" />
        </div>
        <div className="set-row">
          <div><b>When Pirosman starts</b><p className="note">Show this hub, or reopen the project you used last.</p></div>
          <div className="seg" role="group" aria-label="Start screen">
            <button aria-pressed={start === "hub"} onClick={() => { setStart("hub"); prefs.set("startScreen", "hub"); }}>Project Hub</button>
            <button aria-pressed={start === "last"} onClick={() => { setStart("last"); prefs.set("startScreen", "last"); }}>Last project</button>
          </div>
        </div>
        <div className="set-row">
          <div><b>FFmpeg</b><p className="note">Media engine used for import, previews and export.</p></div>
          <div className="set-val"><span className="num">{ff ?? "Checking…"}</span><button className="btn small" onClick={() => set({ dialog: { kind: "ffmpeg" } })}>Details…</button></div>
        </div>
        <div className="set-row">
          <div><b>Cache and storage</b><p className="note">Proxies, thumbnails and previews.</p></div>
          <button className="btn small" onClick={() => set({ hubSection: "storage" })}><HardDrive />Open Storage</button>
        </div>
        <div className="set-row">
          <div><b>Keyboard shortcuts</b><p className="note">50 shortcuts for playback, marking, editing and tools.</p></div>
          <button className="btn small" onClick={() => set({ dialog: { kind: "shortcuts" } })}><Keyboard />Show list</button>
        </div>
      </div>
    </div>
  );
}

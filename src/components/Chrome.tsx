import { useEffect, useRef, useState } from "react";
import { CheckCircle2, Film, FolderOpen, GitBranch, House, Info, Keyboard, Palette, Radio, Send, Users, X, XCircle } from "lucide-react";
import { api } from "../lib/api";
import logo from "../assets/pirosmani.png";
import {
  confirmDiscard, exportOtioFlow, grabStillFlow, importBundleFlow, importFlow, importTimelineFlow, openPathFlow, openProjectFlow, reloadFlow, saveFlow,
} from "../lib/actions";
import { run, useUI, type Page } from "../lib/store";
import { setMode, useCollab } from "../lib/collab";

export function BrandMark({ size = 22 }: { size?: number }) {
  return <img src={logo} width={size} height={size} alt="" className="brand-logo" draggable={false} />;
}

interface Item { label: string; kbd?: string; act: () => void; disabled?: boolean; hint?: string }
type Entry = Item | "sep" | { heading: string };

function Menu({ name, items, open, setOpen, onOpen }: { name: string; items: Entry[]; open: string | null; setOpen: (n: string | null) => void; onOpen?: () => void }) {
  const isOpen = open === name;
  return (
    <div className="menu-root">
      <button className="menu-btn" aria-haspopup="menu" aria-expanded={isOpen} onClick={() => { if (!isOpen) onOpen?.(); setOpen(isOpen ? null : name); }} onMouseEnter={() => { if (open && open !== name) { onOpen?.(); setOpen(name); } }}>
        {name}
      </button>
      {isOpen && (
        <div className="menu" role="menu">
          {items.map((it, i) =>
            it === "sep" ? <div key={i} className="menu-sep" /> : "heading" in it ? <div key={i} className="menu-heading">{it.heading}</div> : (
              <button key={i} role="menuitem" className="menu-item" disabled={it.disabled} title={it.hint} onClick={() => { setOpen(null); it.act(); }}>
                <span className="menu-label">{it.label}</span>{it.kbd && <kbd>{it.kbd}</kbd>}
              </button>
            ),
          )}
        </div>
      )}
    </div>
  );
}

function ProjectTitle() {
  const view = useUI((s) => s.view);
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState("");
  if (!view) return <div className="title-strip" />;
  const changes = view.vcs?.head ? view.vcs.changes.length : 0;
  const commit = () => {
    setEditing(false);
    const n = name.trim();
    if (n && n !== view.project.name) run(api.renameProject(n), `Project renamed to ${n}`);
  };
  return (
    <div className="title-strip">
      {editing ? (
        <input
          className="input title-input" autoFocus value={name} aria-label="Project name"
          onChange={(e) => setName(e.target.value)} onBlur={commit}
          onKeyDown={(e) => { if (e.key === "Enter") commit(); if (e.key === "Escape") setEditing(false); }}
        />
      ) : (
        <button className="title-btn" title="Rename project" onClick={() => { setName(view.project.name); setEditing(true); }}>
          {view.project.name}
        </button>
      )}
      <span className="title-path" title={view.path ?? undefined}>{view.path ?? "not saved yet"}</span>
      {view.unsaved ? (
        <button className="save-state dirty" onClick={() => saveFlow(false)} title="Save now (Ctrl+S)">
          <i aria-hidden="true" />{view.path ? "Not saved" : "Never saved"}{changes > 0 ? ` · ${changes} uncommitted` : ""}
        </button>
      ) : view.path ? (
        <span className="save-state" title="All changes are saved"><i aria-hidden="true" />Saved{changes > 0 ? ` · ${changes} uncommitted` : ""}</span>
      ) : null}
    </div>
  );
}

function ModeSwitch() {
  const mode = useCollab((s) => s.mode);
  const status = useCollab((s) => s.status);
  const peers = useCollab((s) => s.peers);
  const set = useUI((s) => s.set);
  const others = Object.values(peers);
  return (
    <div className="mode-switch" role="group" aria-label="Collaboration mode">
      <button aria-pressed={mode === "revisions"} onClick={() => setMode("revisions")} title="Git-style revisions: offline, branches, diffs, merges, bundles. No network.">
        <GitBranch size={13} />Revisions
      </button>
      <button aria-pressed={mode === "live"} onClick={() => { setMode("live"); if (status !== "live") set({ dialog: { kind: "collab" } }); }} title="Live Sync: real-time multiplayer over WebSockets">
        <Radio size={13} />Live
        {mode === "live" && <span className={`live-dot ${status}`} aria-label={status} />}
      </button>
      {mode === "live" && (
        <button className="peers" onClick={() => set({ dialog: { kind: "collab" } })} title={status === "live" ? `${others.length} collaborator(s) connected` : "Host or join a session"}>
          <Users size={13} />
          {others.slice(0, 4).map((p) => <span key={p.id} className="peer-chip" style={{ background: p.color }} title={p.name}>{p.name.slice(0, 1).toUpperCase()}</span>)}
          {others.length === 0 && <span className="muted">{status === "live" ? "alone" : status}</span>}
        </button>
      )}
    </div>
  );
}

export function MenuBar() {
  const view = useUI((s) => s.view);
  const set = useUI((s) => s.set);
  const playhead = useUI((s) => s.playhead);
  const selection = useUI((s) => s.selection);
  const mode = useCollab((s) => s.mode);
  const [open, setOpen] = useState<string | null>(null);
  const [recent, setRecent] = useState<{ path: string; name: string; exists: boolean }[]>([]);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const h = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) setOpen(null); };
    const k = (e: KeyboardEvent) => { if (e.key === "Escape") setOpen(null); };
    window.addEventListener("mousedown", h);
    window.addEventListener("keydown", k);
    return () => { window.removeEventListener("mousedown", h); window.removeEventListener("keydown", k); };
  }, []);
  const loadRecent = () => { api.recentProjects().then(setRecent).catch(() => setRecent([])); };
  const go = (p: Page) => set({ page: p });
  const hasRevisions = !!view?.path;
  const changes = view?.vcs?.head ? view.vcs.changes.length : 0;

  return (
    <div className="menubar" ref={ref}>
      <button className="home-btn" onClick={() => set({ screen: "hub", hubSection: "projects" })} title="Project Hub: recent projects, presets, storage and settings. Your open project stays open." aria-label="Project Hub"><House size={15} /></button>
      <button className="brand" onClick={() => set({ dialog: { kind: "about" } })} title="About Pirosman Studio"><BrandMark />Pirosman</button>
      <Menu name="File" open={open} setOpen={setOpen} onOpen={loadRecent} items={[
        { label: "Project Hub", act: () => set({ screen: "hub", hubSection: "projects" }) },
        "sep",
        { label: "New project…", kbd: "Ctrl+N", act: async () => { if (await confirmDiscard()) set({ dialog: { kind: "new" } }); } },
        { label: "Open project…", kbd: "Ctrl+O", act: openProjectFlow },
        { label: "Reload from disk", act: reloadFlow, disabled: !view?.path, hint: "Re-read the saved .pirosman file and its history" },
        { heading: "Open recent" },
        ...(recent.length ? recent.slice(0, 8).map((r) => ({ label: r.name, hint: r.path, disabled: !r.exists, act: () => openPathFlow(r.path) })) : [{ label: "No recent projects", act: () => {}, disabled: true }]),
        "sep",
        { label: "Save", kbd: "Ctrl+S", act: () => saveFlow(false) },
        { label: "Save as…", kbd: "Ctrl+Shift+S", act: () => saveFlow(true) },
        "sep",
        { label: "Import media…", kbd: "Ctrl+I", act: importFlow },
        { label: "Import timeline (FCP 7 XML, OTIO, Resolve .drp)…", act: importTimelineFlow },
        { label: "Import revision bundle…", act: importBundleFlow, disabled: !hasRevisions },
        "sep",
        { label: "Export video…", kbd: "Shift+5", act: () => go("deliver") },
        { label: "Export OpenTimelineIO…", act: exportOtioFlow },
        { label: "Grab still (PNG)…", act: () => grabStillFlow(playhead) },
      ]} />
      <Menu name="Edit" open={open} setOpen={setOpen} items={[
        { label: view?.undo_label ? `Undo ${view.undo_label}` : "Undo", kbd: "Ctrl+Z", act: () => run(api.undo()), disabled: !view?.undo_label },
        { label: view?.redo_label ? `Redo ${view.redo_label}` : "Redo", kbd: "Ctrl+Shift+Z", act: () => run(api.redo()), disabled: !view?.redo_label },
        "sep",
        { label: "Copy", kbd: "Ctrl+C", act: () => window.dispatchEvent(new CustomEvent("pirosman-clip", { detail: "copy" })), disabled: !selection.length },
        { label: "Paste at playhead", kbd: "Ctrl+V", act: () => window.dispatchEvent(new CustomEvent("pirosman-clip", { detail: "paste" })) },
        { label: "Duplicate", kbd: "Ctrl+D", act: () => window.dispatchEvent(new CustomEvent("pirosman-clip", { detail: "duplicate" })), disabled: !selection.length },
        { label: "Delete selected", kbd: "Del", act: () => run(api.deleteClips(selection, false)), disabled: !selection.length },
        { label: "Ripple delete", kbd: "Shift+Del", act: () => run(api.deleteClips(selection, true)), disabled: !selection.length },
      ]} />
      <Menu name="Timeline" open={open} setOpen={setOpen} items={[
        { label: "Cut at playhead", kbd: "Ctrl+B", act: () => run(api.splitAt(playhead)) },
        { label: "Insert source at playhead", kbd: "F9", act: () => window.dispatchEvent(new CustomEvent("pirosman-source", { detail: "insert" })) },
        { label: "Overwrite source at playhead", kbd: "F10", act: () => window.dispatchEvent(new CustomEvent("pirosman-source", { detail: "overwrite" })) },
        "sep",
        { label: "Add marker at playhead", kbd: "M", act: () => run(api.addMarker(playhead)) },
        { label: "Set In / Out", kbd: "I / O", act: () => set({ activeMonitor: "timeline" }) },
        { label: "Clear In / Out", act: () => run(api.setRange(null, null)) },
        "sep",
        { label: "Add video track", act: () => run(api.addTrack("video")) },
        { label: "Add audio track", act: () => run(api.addTrack("audio")) },
      ]} />
      <Menu name="Revisions" open={open} setOpen={setOpen} items={[
        { label: "Quick commit…", kbd: "Ctrl+Shift+C", act: () => set({ dialog: { kind: "commit" } }), disabled: !hasRevisions || mode !== "revisions" },
        { label: "Open Revisions page", kbd: "Shift+4", act: () => go("revisions") },
        { label: "Import revision bundle…", act: importBundleFlow, disabled: !hasRevisions },
      ]} />
      <Menu name="Help" open={open} setOpen={setOpen} items={[
        { label: "Keyboard shortcuts", kbd: "?", act: () => set({ dialog: { kind: "shortcuts" } }) },
        { label: "Diagnostics & feature status", act: () => set({ dialog: { kind: "diagnostics" } }) },
        { label: "Resolve binary & interop inspector", act: () => set({ dialog: { kind: "diagnostics", tab: "resolve" } }) },
        { label: "FFmpeg status…", act: () => set({ dialog: { kind: "ffmpeg" } }) },
        { label: "About Pirosman Studio", act: () => set({ dialog: { kind: "about" } }) },
      ]} />
      <ProjectTitle />
      <div className="utility-bar">
        {mode === "revisions" && view?.path && view.vcs && (
          <button className="branch-chip" onClick={() => go("revisions")} title={`Revisions on ${view.vcs.branch}: changes since the last revision (Shift+4)`}>
            <GitBranch />{view.vcs.branch}
            {view.vcs.merging && <span className="changes">merging {view.vcs.merging}</span>}
            {!view.vcs.head ? <span className="count">no revisions</span> : <span className="count">{changes > 0 ? `${changes} changed` : "clean"}</span>}
          </button>
        )}
        {mode === "revisions" && hasRevisions && (
          <button className="btn small primary" onClick={() => set({ dialog: { kind: "commit" } })} title="Commit a revision without leaving this page (Ctrl+Shift+C)">Commit</button>
        )}
        <ModeSwitch />
        <button className="btn icon small ghost" onClick={() => set({ dialog: { kind: "shortcuts" } })} title="Keyboard shortcuts (?)" aria-label="Keyboard shortcuts"><Keyboard /></button>
      </div>
    </div>
  );
}

const PAGES: { id: Page; label: string; icon: typeof Film }[] = [
  { id: "media", label: "Media", icon: FolderOpen },
  { id: "edit", label: "Edit", icon: Film },
  { id: "color", label: "Color", icon: Palette },
  { id: "revisions", label: "Revisions", icon: GitBranch },
  { id: "deliver", label: "Deliver", icon: Send },
];

export function PageSwitcher() {
  const page = useUI((s) => s.page);
  const set = useUI((s) => s.set);
  const view = useUI((s) => s.view);
  const render = useUI((s) => s.render);
  const changes = view?.vcs?.head ? view.vcs.changes.length : 0;
  return (
    <nav className="pages" aria-label="Workspaces">
      {PAGES.map(({ id, label, icon: Icon }, i) => (
        <button key={id} className="page-tab" aria-current={page === id ? "page" : undefined} onClick={() => set({ page: id })} title={`${label} (Shift+${i + 1})`}>
          <Icon />{label}
          {id === "revisions" && changes > 0 && <span className="badge">{changes}</span>}
          {id === "deliver" && render?.job === "export" && !render.result && !render.error && <span className="badge">{Math.round((render.progress?.fraction ?? 0) * 100)}%</span>}
        </button>
      ))}
      <div className="status">
        {view?.vcs && <span title="Current branch"><GitBranch size={12} />{view.vcs.branch}</span>}
        {view && <span className="num">{view.project.media.length} media · {view.project.tracks.reduce((n, t) => n + t.clips.length, 0)} clip{view.project.tracks.reduce((n, t) => n + t.clips.length, 0) === 1 ? "" : "s"}</span>}
        {view && <span className="num">{view.project.settings.width}×{view.project.settings.height} · {(view.project.settings.frame_rate.num / view.project.settings.frame_rate.den).toFixed(3).replace(/\.?0+$/, "")} fps</span>}
        {view && view.project.media.length > 0 && (
          <span className="num sha" title="Media fingerprinted with SHA-256 and referenced in place (never copied)">
            SHA-256 {view.project.media.filter((m) => m.sha256).length}/{view.project.media.length}
          </span>
        )}
      </div>
    </nav>
  );
}

export function Toasts() {
  const toasts = useUI((s) => s.toasts);
  const dismiss = useUI((s) => s.dismiss);
  return (
    <div className="toasts" role="status" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className={`toast ${t.kind}`}>
          {t.kind === "error" ? <XCircle /> : t.kind === "ok" ? <CheckCircle2 /> : <Info />}
          <span style={{ userSelect: "text" }}>{t.text}</span>
          <button className="btn icon small ghost" aria-label="Dismiss" onClick={() => dismiss(t.id)}><X /></button>
        </div>
      ))}
    </div>
  );
}

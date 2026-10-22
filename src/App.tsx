import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { api, type RenderProgress, type RenderResult } from "./lib/api";
import { importPaths, openPathFlow, openProjectFlow, saveFlow, importFlow, confirmDiscard } from "./lib/actions";
import { attempt, prefs, projectDuration, run, useUI, type Page } from "./lib/store";
import { connect, setMyName, useCollab } from "./lib/collab";
import { RemoteCursors } from "./components/RemoteCursors";
import { DragGhost } from "./lib/dnd";
import { TooltipLayer } from "./lib/tooltip";
import { loadOfxCatalog } from "./lib/ofx";
import { ProjectHub } from "./hub/ProjectHub";
import { MenuBar, PageSwitcher, Toasts } from "./components/Chrome";
import { DialogHost } from "./components/Dialogs";
import { showContextMenu, showTextFieldMenu } from "./lib/contextMenu";
import { MediaWorkspace } from "./workspaces/MediaWorkspace";
import { EditWorkspace } from "./workspaces/EditWorkspace";
import { ColorWorkspace } from "./workspaces/ColorWorkspace";
import { RevisionsWorkspace } from "./workspaces/RevisionsWorkspace";
import { DeliverWorkspace } from "./workspaces/DeliverWorkspace";

const PAGES: Page[] = ["media", "edit", "color", "revisions", "deliver"];

function isTyping(e: KeyboardEvent) {
  const t = e.target as HTMLElement | null;
  return !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable);
}

/** Clipboard verbs shared by keys, the Edit menu and the clip context menu. */
async function clipboard(kind: string) {
  const s = useUI.getState();
  if (kind === "copy") {
    if (!s.selection.length) return;
    const n = await attempt(api.copyItems(s.selection));
    if (n) s.toast("info", `Copied ${n} item${n === 1 ? "" : "s"}`);
  } else if (kind === "paste") run(api.pasteItems(s.playhead));
  else if (kind === "duplicate" && s.selection.length) run(api.duplicateItems(s.selection));
}

function useGlobalKeys() {
  useEffect(() => {
    const h = (e: Event) => void clipboard((e as CustomEvent<string>).detail);
    window.addEventListener("pirosman-clip", h);
    return () => window.removeEventListener("pirosman-clip", h);
  }, []);
  useEffect(() => {
    const h = async (e: KeyboardEvent) => {
      const s = useUI.getState();
      const v = s.view;
      if (!v) return;
      const ctrl = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      if (ctrl && k === "s") { e.preventDefault(); saveFlow(e.shiftKey); return; }
      if (ctrl && k === "o") { e.preventDefault(); openProjectFlow(); return; }
      if (ctrl && k === "n") { e.preventDefault(); if (await confirmDiscard()) s.set({ dialog: { kind: "new" } }); return; }
      // The Project Hub only takes the project-level shortcuts above.
      if (s.screen === "hub") return;
      if (ctrl && k === "i") { e.preventDefault(); importFlow(); return; }
      if (isTyping(e) || s.dialog) return;
      if (ctrl && (k === "z" || k === "y")) {
        e.preventDefault();
        if (k === "y" || e.shiftKey) { if (v.redo_label) run(api.redo()); } else if (v.undo_label) run(api.undo());
        return;
      }
      if (ctrl && k === "b") { e.preventDefault(); run(api.splitAt(s.playhead)); return; }
      if (ctrl && e.shiftKey && k === "c") { e.preventDefault(); if (v.path) s.set({ dialog: { kind: "commit" } }); else s.toast("info", "Save the project first: revisions live next to the .pirosman file."); return; }
      if (ctrl && k === "c") { e.preventDefault(); clipboard("copy"); return; }
      if (ctrl && k === "v") { e.preventDefault(); clipboard("paste"); return; }
      if (ctrl && k === "d") { e.preventDefault(); clipboard("duplicate"); return; }
      if (ctrl && k === "a") { e.preventDefault(); s.set({ selection: v.project.tracks.flatMap((t) => t.clips.map((c) => c.id)) }); return; }
      if (e.shiftKey && /^Digit[1-5]$/.test(e.code)) { e.preventDefault(); s.set({ page: PAGES[Number(e.code.slice(5)) - 1] }); return; }
      if (e.altKey && k === "x") { e.preventDefault(); run(api.setRange(null, null)); return; }
      if (ctrl || e.altKey) return;
      const timelineMarks = s.activeMonitor === "timeline" || s.page !== "edit" && s.page !== "media";
      const total = projectDuration(v);
      const sec = Math.round(v.project.settings.frame_rate.num / v.project.settings.frame_rate.den);
      switch (e.key) {
        // J/K/L shuttle: repeated J or L doubles the speed (1×, 2×, 4×) backwards or forwards.
        case " ": e.preventDefault(); s.set({ playing: !s.playing, shuttle: 1 }); break;
        case "k": case "K": s.set({ playing: false, shuttle: 1 }); break;
        case "l": case "L": s.set(s.playing && s.shuttle > 0 ? { shuttle: Math.min(4, s.shuttle * 2) } : { playing: true, shuttle: 1 }); break;
        case "j": case "J": s.set(s.playing && s.shuttle < 0 ? { shuttle: Math.max(-4, s.shuttle * 2) } : { playing: true, shuttle: -1 }); break;
        case "ArrowLeft": e.preventDefault(); s.set({ playing: false, playhead: Math.max(0, s.playhead - (e.shiftKey ? sec : 1)) }); break;
        case "ArrowRight": e.preventDefault(); s.set({ playing: false, playhead: Math.min(total, s.playhead + (e.shiftKey ? sec : 1)) }); break;
        case "Home": s.set({ playhead: 0, playing: false }); break;
        case "End": s.set({ playhead: total, playing: false }); break;
        case "Delete": case "Backspace": if (s.selection.length) run(api.deleteClips(s.selection, e.shiftKey)); break;
        case "i": case "I":
          if (timelineMarks) run(api.setRange(s.playhead, v.project.mark_out != null && v.project.mark_out > s.playhead ? v.project.mark_out : null));
          else window.dispatchEvent(new CustomEvent("pirosman-source", { detail: "in" }));
          break;
        case "o": case "O":
          if (timelineMarks) run(api.setRange(v.project.mark_in != null && v.project.mark_in < s.playhead ? v.project.mark_in : null, Math.max(1, s.playhead)));
          else window.dispatchEvent(new CustomEvent("pirosman-source", { detail: "out" }));
          break;
        case "m": case "M": run(api.addMarker(s.playhead)); break;
        case "F9": e.preventDefault(); window.dispatchEvent(new CustomEvent("pirosman-source", { detail: "insert" })); break;
        case "F10": e.preventDefault(); window.dispatchEvent(new CustomEvent("pirosman-source", { detail: "overwrite" })); break;
        case "=": case "+": s.set({ pxPerFrame: Math.min(24, s.pxPerFrame * 1.25) }); break;
        case "-": s.set({ pxPerFrame: Math.max(0.05, s.pxPerFrame / 1.25) }); break;
        case "Z": if (e.shiftKey) window.dispatchEvent(new CustomEvent("pirosman-fit")); break;
        case "?": s.set({ dialog: { kind: "shortcuts" } }); break;
        case "Escape": s.set({ selection: [] }); break;
      }
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, []);
}

/** Replace the WebView's browser context menu with native menus everywhere. */
function useNativeContextMenu() {
  useEffect(() => {
    const h = (e: MouseEvent) => {
      if (e.defaultPrevented) return; // a component already showed its own menu
      const t = e.target as HTMLElement;
      if (t.closest("input:not([type=range]):not([type=checkbox]), textarea")) { showTextFieldMenu(e); return; }
      const s = useUI.getState();
      const v = s.view;
      showContextMenu(e, [
        { text: v?.undo_label ? `Undo ${v.undo_label}` : "Undo", accelerator: "Ctrl+Z", enabled: !!v?.undo_label, action: () => run(api.undo()) },
        { text: v?.redo_label ? `Redo ${v.redo_label}` : "Redo", accelerator: "Ctrl+Shift+Z", enabled: !!v?.redo_label, action: () => run(api.redo()) },
        "sep",
        { text: "Import media…", accelerator: "Ctrl+I", action: importFlow },
        { text: "Save", accelerator: "Ctrl+S", action: () => saveFlow(false) },
        "sep",
        { text: "Go to page", submenu: PAGES.map((p, i) => ({ text: p[0].toUpperCase() + p.slice(1), accelerator: `Shift+${i + 1}`, checked: s.page === p, action: () => s.set({ page: p }) })) },
        { text: "Keyboard shortcuts", action: () => s.set({ dialog: { kind: "shortcuts" } }) },
      ]);
    };
    window.addEventListener("contextmenu", h);
    return () => window.removeEventListener("contextmenu", h);
  }, []);
}

function useBackendEvents() {
  useEffect(() => {
    const offs: Promise<() => void>[] = [];
    let refetch: ReturnType<typeof setTimeout> | undefined;
    offs.push(listen("media-updated", () => {
      clearTimeout(refetch);
      refetch = setTimeout(() => api.getView().then((v) => useUI.getState().setView(v)), 120);
    }));
    offs.push(listen<{ id: string; job: string; state: string; error?: string }>("media-job", (e) => {
      if (e.payload.state === "failed") useUI.getState().toast("error", `Preview proxy failed: ${e.payload.error}`);
    }));
    offs.push(listen<{ job: "export" | "preview"; progress: RenderProgress }>("render-progress", (e) => {
      const r = useUI.getState().render;
      if (r && r.job === e.payload.job) useUI.setState({ render: { ...r, progress: e.payload.progress } });
    }));
    offs.push(listen<{ job: "export" | "preview"; result: RenderResult; seconds: number }>("render-finished", (e) => {
      const s = useUI.getState();
      const r = s.render;
      if (e.payload.job === "preview") {
        useUI.setState({ render: null, previewFile: e.payload.result.output });
        s.toast("ok", `Preview rendered in ${e.payload.seconds.toFixed(1)} s — switch the monitor to Rendered`);
      } else if (r) {
        useUI.setState({ render: { ...r, result: e.payload.result, seconds: e.payload.seconds } });
        s.toast("ok", `Export finished: ${e.payload.result.output}`);
      }
    }));
    offs.push(listen<{ job: "export" | "preview"; error: string }>("render-failed", (e) => {
      const s = useUI.getState();
      if (e.payload.job === "preview") { useUI.setState({ render: null }); s.toast("error", `Preview render failed: ${e.payload.error}`); }
      else if (s.render) useUI.setState({ render: { ...s.render, error: e.payload.error } });
    }));
    offs.push(getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === "drop" && e.payload.paths.length) {
        const proj = e.payload.paths.find((p) => p.toLowerCase().endsWith(".pirosman"));
        if (proj) void openPathFlow(proj);
        else { useUI.setState({ screen: "editor" }); void importPaths(e.payload.paths); }
      }
    }));
    return () => { offs.forEach((p) => p.then((f) => f())); };
  }, []);
}

export default function App() {
  const view = useUI((s) => s.view);
  const page = useUI((s) => s.page);
  useGlobalKeys();
  useBackendEvents();
  useNativeContextMenu();
  useEffect(() => { api.getView().then((v) => useUI.getState().setView(v)).catch((e) => useUI.getState().toast("error", String(e))); }, []);
  // Describe installed OpenFX plugins in the background (cached; changed plugins are re-read).
  useEffect(() => { const t = setTimeout(() => void loadOfxCatalog(), 1500); return () => clearTimeout(t); }, []);
  // Start on the Project Hub, or reopen the last project if the user chose that in Settings.
  useEffect(() => {
    api.startupArgs().then(async (a) => {
      if (a.join && a.code) { useUI.setState({ screen: "editor" }); return; }
      if (prefs.get("startScreen", "hub") === "last") {
        const last = (await api.recentProjects().catch(() => []))[0];
        if (last?.exists) await openPathFlow(last.path);
      }
    }).catch(() => {});
  }, []);
  // FFmpeg ships with the app. If that copy is missing or damaged, explain it at start-up
  // instead of failing on the first import; a system fallback only earns a notice.
  useEffect(() => {
    api.ffmpegStatus().then((t) => {
      if (!t.found) { if (!useUI.getState().dialog) useUI.setState({ dialog: { kind: "ffmpeg" } }); }
      else if (t.warning) useUI.getState().toast("info", `Using a system FFmpeg because the bundled copy is unusable: ${t.warning}`);
    }).catch(() => {});
  }, []);
  // A second collaborator window is launched with --join/--code/--name: join straight away.
  useEffect(() => {
    api.startupArgs().then((a) => {
      if (!a.join || !a.code) return;
      if (a.name) setMyName(a.name);
      useCollab.setState({ mode: "live" });
      connect(a.join, a.code);
    }).catch(() => {});
  }, []);
  useEffect(() => {
    const fit = () => {
      const s = useUI.getState();
      const lanes = document.querySelector(".tl-lanes") as HTMLElement | null;
      const d = projectDuration(s.view);
      if (lanes && d > 0) s.set({ pxPerFrame: Math.max(0.05, Math.min(24, (lanes.clientWidth - 40) / d)) });
    };
    window.addEventListener("pirosman-fit", fit);
    return () => window.removeEventListener("pirosman-fit", fit);
  }, []);

  const screen = useUI((s) => s.screen);
  if (screen === "hub") {
    return (
      <>
        <ProjectHub />
        <Toasts />
        <DialogHost />
        <TooltipLayer />
      </>
    );
  }
  return (
    <div className="app screen-enter">
      <MenuBar />
      {!view ? <div className="page"><div className="panel" /></div> : (
        <>
          {page === "media" && <MediaWorkspace />}
          {page === "edit" && <EditWorkspace />}
          {page === "color" && <ColorWorkspace />}
          {page === "revisions" && <RevisionsWorkspace />}
          {page === "deliver" && <DeliverWorkspace />}
        </>
      )}
      <PageSwitcher />
      <Toasts />
      <DialogHost />
      <RemoteCursors />
      <DragGhost />
      <TooltipLayer />
    </div>
  );
}

// User-level flows that combine native dialogs with commands.
import { open, save, ask } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { attempt, errText, run, useUI } from "./store";

const MEDIA_EXT = ["mp4", "mov", "m4v", "mkv", "webm", "avi", "mxf", "mts", "m2ts", "wav", "mp3", "m4a", "aac", "flac", "ogg", "opus", "png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp"];

export async function confirmDiscard(): Promise<boolean> {
  const v = useUI.getState().view;
  if (!v?.unsaved || (v.project.media.length === 0 && !v.path)) return true;
  return ask("The current project has unsaved changes. Discard them?", { title: "Unsaved changes", kind: "warning", okLabel: "Discard", cancelLabel: "Keep editing" });
}

export async function openProjectFlow() {
  if (!(await confirmDiscard())) return;
  const path = await open({ multiple: false, filters: [{ name: "Pirosman project", extensions: ["pirosman"] }] });
  if (typeof path !== "string") return;
  if (await run(api.openProject(path))) useUI.setState({ playhead: 0, selection: [], previewFile: null, screen: "editor" });
}

export async function saveFlow(saveAs = false): Promise<boolean> {
  const v = useUI.getState().view;
  if (!v) return false;
  if (v.path && !saveAs) return run(api.saveProject(), "Project saved");
  const path = await save({ defaultPath: `${v.project.name}.pirosman`, filters: [{ name: "Pirosman project", extensions: ["pirosman"] }] });
  if (!path) return false;
  return run(api.saveProject(path), "Project saved");
}

export async function importFlow() {
  const paths = await open({ multiple: true, filters: [{ name: "Media", extensions: MEDIA_EXT }, { name: "All files", extensions: ["*"] }] });
  if (!paths) return;
  await importPaths(Array.isArray(paths) ? paths : [paths]);
}

export async function importPaths(paths: string[]) {
  if (!paths.length) return;
  const { toast, setView } = useUI.getState();
  toast("info", `Probing and hashing ${paths.length} file${paths.length > 1 ? "s" : ""}…`);
  try {
    const r = await api.importMedia(paths);
    setView(r.view);
    if (r.imported.length) toast("ok", `Imported ${r.imported.join(", ")}`);
    for (const e of r.errors) toast("error", e);
    const last = r.view.project.media[r.view.project.media.length - 1];
    if (r.imported.length && last) useUI.setState({ sourceMediaId: last.id, markIn: null, markOut: null });
  } catch (e) {
    toast("error", errText(e));
  }
}

export async function relinkFlow(mediaId: string) {
  const path = await open({ multiple: false, title: "Locate the original media file" });
  if (typeof path !== "string") return;
  try {
    useUI.getState().setView(await api.relinkMedia(mediaId, path, false));
    useUI.getState().toast("ok", "Media relinked; content hash matches");
  } catch (e) {
    const msg = errText(e);
    if (msg.includes("different content")) {
      const yes = await ask(`${msg}\n\nLink to this different file anyway?`, { title: "Different media", kind: "warning", okLabel: "Use different file", cancelLabel: "Cancel" });
      if (yes) await run(api.relinkMedia(mediaId, path, true), "Media relinked to a different file");
    } else useUI.getState().toast("error", msg);
  }
}

export async function exportBundleFlow(branches: string[], includeMedia: boolean) {
  const v = useUI.getState().view;
  if (!v) return;
  const path = await save({ defaultPath: `${v.project.name}-${branches.join("+").replace(/[^\w.-]+/g, "_")}.pirosman-bundle`, filters: [{ name: "Pirosman bundle", extensions: ["pirosman-bundle"] }] });
  if (!path) return;
  const r = await attempt(api.bundleExport(branches, includeMedia, path, useUI.getState().author));
  if (r) useUI.getState().toast("ok", `Bundle written: ${r.revisions} revision${r.revisions === 1 ? "" : "s"}, ${r.media_included} media file${r.media_included === 1 ? "" : "s"} (${(r.bytes / 1048576).toFixed(1)} MB)`);
}

export async function importBundleFlow() {
  const path = await open({ multiple: false, filters: [{ name: "Pirosman bundle", extensions: ["pirosman-bundle"] }] });
  if (typeof path !== "string") return;
  const r = await attempt(api.bundleImport(path));
  if (!r) return;
  useUI.getState().setView(r.view);
  const b = Object.keys(r.report.branches).join(", ");
  useUI.getState().toast("ok", `Imported ${r.report.new_revisions} new revision(s) from ${r.report.author} as ${b}${r.report.media_missing.length ? `. Missing media: ${r.report.media_missing.join(", ")}` : ""}`);
}

/** Open a project by path; if it is already the open project, just return to the editor. */
export async function openPathFlow(path: string) {
  const v = useUI.getState().view;
  if (v?.path && v.path.toLowerCase() === path.toLowerCase()) { useUI.setState({ screen: "editor" }); return true; }
  if (!(await confirmDiscard())) return false;
  const ok = await run(api.openProject(path));
  if (ok) useUI.setState({ playhead: 0, selection: [], previewFile: null, screen: "editor" });
  return ok;
}

/** Project Hub "Import archive": a .pirosman-bundle becomes a new project with its history. */
export async function importArchiveFlow() {
  const bundle = await open({ multiple: false, title: "Choose a revision bundle", filters: [{ name: "Pirosman bundle", extensions: ["pirosman-bundle"] }] });
  if (typeof bundle !== "string") return;
  const stem = bundle.replace(/^.*[\\/]/, "").replace(/\.pirosman-bundle$/i, "");
  const dest = await save({ title: "Save the new project as", defaultPath: `${stem}.pirosman`, filters: [{ name: "Pirosman project", extensions: ["pirosman"] }] });
  if (!dest) return;
  useUI.getState().toast("info", "Unpacking the bundle and its history…");
  const path = await attempt(api.hubImportArchive(bundle, dest));
  if (path && (await openPathFlow(path))) useUI.getState().toast("ok", "Project created from the bundle, with its revision history.");
}

export async function reloadFlow() {
  const v = useUI.getState().view;
  if (!v?.path) return;
  if (v.unsaved && !(await ask("Reloading discards unsaved changes. Continue?", { title: "Reload project", kind: "warning", okLabel: "Reload", cancelLabel: "Cancel" }))) return;
  await run(api.openProject(v.path), "Project reloaded from disk");
}

export async function importTimelineFlow() {
  if (!(await confirmDiscard())) return;
  const path = await open({ multiple: false, filters: [{ name: "Timelines", extensions: ["xml", "otio", "drp", "drt"] }] });
  if (typeof path !== "string") return;
  const { toast, setView } = useUI.getState();
  toast("info", "Reading timeline and probing its media…");
  try {
    const r = await api.importTimeline(path);
    setView(r.view);
    useUI.setState({ playhead: 0, selection: [], page: "edit", screen: "editor" });
    toast("ok", `Imported ${r.format}: ${r.view.project.tracks.reduce((n, t) => n + t.clips.length, 0)} clips, ${r.view.project.media.length} media. Save it to keep it.`);
    for (const w of r.warnings.slice(0, 4)) toast("info", w);
  } catch (e) {
    toast("error", errText(e));
  }
}

export async function exportOtioFlow() {
  const v = useUI.getState().view;
  if (!v) return;
  const path = await save({ defaultPath: `${v.project.name}.otio`, filters: [{ name: "OpenTimelineIO", extensions: ["otio"] }] });
  if (!path) return;
  const out = await attempt(api.exportOtio(path));
  if (out) useUI.getState().toast("ok", `OpenTimelineIO written: ${out}`);
}

export async function grabStillFlow(frame: number) {
  const v = useUI.getState().view;
  if (!v) return;
  const path = await save({ defaultPath: `${v.project.name}-frame${frame}.png`, filters: [{ name: "PNG image", extensions: ["png"] }] });
  if (!path) return;
  useUI.getState().toast("info", "Rendering the graded frame…");
  const out = await attempt(api.grabStill(frame, path));
  if (out) useUI.getState().toast("ok", `Still saved: ${out}`);
}
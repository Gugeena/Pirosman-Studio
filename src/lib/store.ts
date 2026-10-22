import { create } from "zustand";
import type { Clip, MediaInfo, MediaRef, OfxCatalog, OfxPlugin, RenderPlan, RenderProgress, RenderResult, Track, View, VfxKind } from "./api";

/** What is being dragged with the pointer (HTML5 drag-and-drop is unavailable while the
 *  window accepts file drops from Explorer, so drags inside the app are pointer-driven). */
export type DragPayload =
  | { kind: "media"; id: string; label: string; frames: number | null }
  | { kind: "ofx"; plugin: OfxPlugin; label: string }
  | { kind: "vfx"; vfx: VfxKind; label: string };
export interface DragState { payload: DragPayload; x: number; y: number }
import { timelineEnd } from "./clip";

export type Page = "media" | "edit" | "color" | "revisions" | "deliver";
export type ToastKind = "ok" | "error" | "info";
export interface Toast { id: number; kind: ToastKind; text: string }

export interface RenderJob {
  job: "export" | "preview";
  plan: RenderPlan;
  progress: RenderProgress | null;
  result: RenderResult | null;
  error: string | null;
  seconds?: number;
}

export type Screen = "hub" | "editor";
export type HubSection = "projects" | "revisions" | "storage" | "settings";

interface UI {
  view: View | null;
  /** Project Hub (start screen) or the editor workspaces. */
  screen: Screen;
  hubSection: HubSection;
  /** Shuttle speed for J/K/L: 0 stopped, ±1, ±2, ±4 (negative plays backwards). */
  shuttle: number;
  page: Page;
  selection: string[];
  playhead: number;
  playing: boolean;
  pxPerFrame: number;
  sourceMediaId: string | null;
  markIn: number | null;
  markOut: number | null;
  toasts: Toast[];
  render: RenderJob | null;
  previewFile: string | null;
  author: string;
  vcsTick: number;
  /** Which viewer I/O keys address. */
  activeMonitor: "source" | "timeline";
  drag: DragState | null;
  /** Installed OpenFX plugins (null until the first scan finishes). */
  ofx: OfxCatalog | null;
  ofxScanning: boolean;
  dialog:
    | null
    | { kind: "new"; preset?: string }
    | { kind: "diagnostics"; tab?: "parity" | "deps" | "ofx" | "resolve" }
    | { kind: "collab" }
    | { kind: "about" }
    | { kind: "dirty-switch"; branch: string }
    | { kind: "shortcuts" }
    | { kind: "commit" }
    | { kind: "ffmpeg" }
    | { kind: "prompt"; title: string; label: string; initial: string; confirm: string; multiline?: boolean; onSubmit: (value: string) => void };
  setView: (v: View) => void;
  set: (p: Partial<UI>) => void;
  toast: (kind: ToastKind, text: string) => void;
  dismiss: (id: number) => void;
}

let toastId = 0;

const readAuthor = () => {
  try { return localStorage.getItem("pirosman.author") ?? ""; } catch { return ""; }
};

/** Per-viewer preferences (localStorage; the app works without them). */
export const prefs = {
  get(key: string, fallback: string): string {
    try { return localStorage.getItem(`pirosman.${key}`) ?? fallback; } catch { return fallback; }
  },
  set(key: string, value: string) {
    try { localStorage.setItem(`pirosman.${key}`, value); } catch { /* convenience only */ }
  },
};

export const useUI = create<UI>((set, get) => ({
  view: null,
  screen: "hub",
  hubSection: "projects",
  shuttle: 1,
  page: "edit",
  selection: [],
  playhead: 0,
  playing: false,
  pxPerFrame: 4,
  sourceMediaId: null,
  markIn: null,
  markOut: null,
  toasts: [],
  render: null,
  previewFile: null,
  author: readAuthor(),
  vcsTick: 0,
  activeMonitor: "timeline",
  drag: null,
  ofx: null,
  ofxScanning: false,
  dialog: null,
  setView: (v) => {
    const ids = new Set(v.project.tracks.flatMap((t) => [...t.clips.map((c) => c.id), ...(t.titles ?? []).map((x) => x.id)]));
    set({
      view: v,
      selection: get().selection.filter((id) => ids.has(id)),
      vcsTick: get().vcsTick + 1,
      sourceMediaId: v.project.media.some((m) => m.id === get().sourceMediaId) ? get().sourceMediaId : null,
    });
  },
  set: (p) => set(p),
  toast: (kind, text) => {
    const id = ++toastId;
    set({ toasts: [...get().toasts.slice(-3), { id, kind, text }] });
    setTimeout(() => get().dismiss(id), kind === "error" ? 9000 : 4500);
  },
  dismiss: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),
}));

export const errText = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));

/** Run a command that returns a new View; surface failures as toasts. */
export async function run(p: Promise<View>, ok?: string): Promise<boolean> {
  try {
    const v = await p;
    useUI.getState().setView(v);
    if (ok) useUI.getState().toast("ok", ok);
    return true;
  } catch (e) {
    useUI.getState().toast("error", errText(e));
    return false;
  }
}

export async function attempt<T>(p: Promise<T>): Promise<T | undefined> {
  try {
    return await p;
  } catch (e) {
    useUI.getState().toast("error", errText(e));
    return undefined;
  }
}

// ---- derived helpers

export function mediaMap(v: View | null): Map<string, MediaRef> {
  return new Map((v?.project.media ?? []).map((m) => [m.id, m]));
}
export function infoMap(v: View | null): Map<string, MediaInfo> {
  return new Map((v?.media ?? []).map((m) => [m.id, m]));
}
export function findClip(v: View | null, id: string | null | undefined): { track: Track; clip: Clip } | null {
  if (!v || !id) return null;
  for (const t of v.project.tracks) {
    const c = t.clips.find((c) => c.id === id);
    if (c) return { track: t, clip: c };
  }
  return null;
}
export function projectDuration(v: View | null): number {
  return timelineEnd(v?.project);
}

/** Themed replacement for window.prompt. */
export function promptText(title: string, label: string, initial: string, confirm: string, onSubmit: (v: string) => void, multiline = false) {
  useUI.setState({ dialog: { kind: "prompt", title, label, initial, confirm, onSubmit, multiline } });
}
/** Short code shown under media-pool cells and in the inspector (M01, M02…). */
export function mediaCode(v: View | null, id: string): string {
  const i = v?.project.media.findIndex((m) => m.id === id) ?? -1;
  return `M${String(i + 1).padStart(2, "0")}`;
}

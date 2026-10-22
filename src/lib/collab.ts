// Live Sync client. Talks to the WebSocket hub (crates/pirosman-core/src/collab.rs):
// presence (cursor, page, playhead, selection) ~30 Hz, and whole-project
// snapshots after each local edit. Remote snapshots are applied through the
// Rust command layer, so the Rust model stays the single source of truth.
import { create } from "zustand";
import { api, type Project } from "./api";
import { errText, useUI } from "./store";

export interface Peer {
  id: number;
  name: string;
  color: string;
  x?: number;
  y?: number;
  page?: string;
  playhead?: number;
  selection?: string[];
  at?: number;
}

export type CollabMode = "revisions" | "live";

interface CollabState {
  mode: CollabMode;
  status: "off" | "connecting" | "live" | "error";
  url: string | null;
  code: string | null;
  hosting: { port: number; code: string; local_url: string; lan_urls: string[] } | null;
  me: { id: number | null; name: string; color: string };
  peers: Record<number, Peer>;
  error: string | null;
}

const PALETTE = ["#5f9be6", "#5fb59a", "#b48fe0", "#e8a33d", "#ff7a6b", "#4fc3d9"];

export const useCollab = create<CollabState>(() => ({
  mode: "revisions",
  status: "off",
  url: null,
  code: null,
  hosting: null,
  me: { id: null, name: localName(), color: PALETTE[Math.floor(Math.random() * PALETTE.length)] },
  peers: {},
  error: null,
}));

function localName() {
  try {
    return localStorage.getItem("pirosman.author") || "Editor";
  } catch {
    return "Editor";
  }
}

let ws: WebSocket | null = null;
let lastSent = "";
let lastApplied = "";
let unsubView: (() => void) | null = null;
let presenceTimer: number | null = null;
let pointer = { x: 0.5, y: 0.5 };

function send(msg: object) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
}

function onPointer(e: PointerEvent) {
  pointer = { x: e.clientX / window.innerWidth, y: e.clientY / window.innerHeight };
}

let lastPresence = "";
let lastPresenceAt = 0;
function presence() {
  const s = useUI.getState();
  const msg = JSON.stringify({ type: "presence", x: pointer.x, y: pointer.y, page: s.page, playhead: s.playhead, selection: s.selection });
  // Skip unchanged frames, but keep a 1 s heartbeat so peers see we are alive.
  if (msg === lastPresence && Date.now() - lastPresenceAt < 1000) return;
  lastPresence = msg;
  lastPresenceAt = Date.now();
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(msg);
}

/** Peers on the same page, for cursors / ghost playheads / clip presence. */
export function usePeersOn(page: string): Peer[] {
  const peers = useCollab((s) => s.peers);
  const me = useCollab((s) => s.me.id);
  return Object.values(peers).filter((p) => p.id !== me && p.page === page);
}

function projectKey(p: Project) {
  return JSON.stringify(p);
}

async function applyRemote(project: Project, author: string) {
  const key = projectKey(project);
  if (key === lastSent || key === lastApplied) return;
  lastApplied = key;
  try {
    const v = await api.collabApplyRemote(project, author);
    lastSent = projectKey(v.project);
    useUI.getState().setView(v);
  } catch (e) {
    useUI.getState().toast("error", `Could not apply ${author}'s edit: ${errText(e)}`);
  }
}

/** Join a hub (the host joins its own hub too). */
export function connect(url: string, code: string) {
  disconnect(false);
  const me = useCollab.getState().me;
  useCollab.setState({ status: "connecting", url, code, error: null, peers: {} });
  const sock = new WebSocket(url);
  ws = sock;
  sock.onopen = () => sock.send(JSON.stringify({ type: "hello", code, name: me.name, color: me.color }));
  sock.onmessage = (ev) => {
    let m: any;
    try { m = JSON.parse(String(ev.data)); } catch { return; }
    const st = useCollab.getState();
    switch (m.type) {
      case "welcome": {
        const peers: Record<number, Peer> = {};
        for (const p of m.peers ?? []) peers[p.id] = p;
        useCollab.setState({ status: "live", me: { ...st.me, id: m.id }, peers });
        // Late joiners ask for the current cut; anyone holding it answers.
        send({ type: "request_project" });
        break;
      }
      case "join":
        useCollab.setState({ peers: { ...st.peers, [m.peer.id]: m.peer } });
        useUI.getState().toast("info", `${m.peer.name} joined the live session`);
        // Bring the newcomer up to date.
        { const v = useUI.getState().view; if (v) { lastSent = projectKey(v.project); send({ type: "project", project: v.project, author: st.me.name }); } }
        break;
      case "leave": {
        const peers = { ...st.peers };
        const who = peers[m.id]?.name;
        delete peers[m.id];
        useCollab.setState({ peers });
        if (who) useUI.getState().toast("info", `${who} left`);
        break;
      }
      case "presence":
        if (st.peers[m.from]) useCollab.setState({ peers: { ...st.peers, [m.from]: { ...st.peers[m.from], ...m, id: m.from, at: Date.now() } } });
        break;
      case "request_project": {
        const v = useUI.getState().view;
        if (v) send({ type: "project", project: v.project, author: st.me.name });
        break;
      }
      case "project":
        void applyRemote(m.project, st.peers[m.from]?.name ?? m.author ?? "a collaborator");
        break;
      case "error":
        useCollab.setState({ status: "error", error: m.message });
        break;
    }
  };
  sock.onclose = () => {
    if (ws === sock) {
      ws = null;
      useCollab.setState((s) => ({ status: s.status === "error" ? "error" : "off", peers: {} }));
    }
  };
  sock.onerror = () => useCollab.setState({ status: "error", error: `Could not reach ${url}` });

  window.addEventListener("pointermove", onPointer);
  presenceTimer = window.setInterval(presence, 33);
  lastSent = projectKey(useUI.getState().view?.project ?? ({} as Project));
  // Broadcast every local edit (the Rust model already validated it).
  unsubView = useUI.subscribe((s, prev) => {
    if (!s.view || s.view === prev.view) return;
    const key = projectKey(s.view.project);
    if (key === lastSent || key === lastApplied) return;
    lastSent = key;
    send({ type: "project", project: s.view.project, author: useCollab.getState().me.name });
  });
}

export function disconnect(resetMode = true) {
  if (ws) {
    const w = ws;
    ws = null;
    w.close();
  }
  if (presenceTimer) window.clearInterval(presenceTimer);
  presenceTimer = null;
  window.removeEventListener("pointermove", onPointer);
  unsubView?.();
  unsubView = null;
  useCollab.setState({ status: "off", peers: {}, ...(resetMode ? { url: null, code: null } : {}) });
}

/** Switch collaboration mode. Revisions mode stops any hub and socket. */
export async function setMode(mode: CollabMode) {
  if (mode === "revisions") {
    disconnect();
    if (useCollab.getState().hosting) await api.collabHostStop().catch(() => false);
    useCollab.setState({ mode, hosting: null });
    return;
  }
  useCollab.setState({ mode });
}

export async function host() {
  const h = await api.collabHostStart();
  useCollab.setState({ hosting: h, mode: "live" });
  connect(h.local_url, h.code);
  return h;
}

export function setMyName(name: string) {
  useCollab.setState((s) => ({ me: { ...s.me, name: name || "Editor" } }));
}

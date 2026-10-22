import { useEffect, useState } from "react";
import { Copy, MonitorUp, Radio, X } from "lucide-react";
import { api, type Diagnostics, type ResolveReport } from "../lib/api";
import { connect, host, setMode, setMyName, useCollab } from "../lib/collab";
import { BrandMark } from "./Chrome";
import { FrameShape } from "./FrameShape";
import { PRESETS, RATES, fpsLabel } from "../lib/presets";
import { attempt, run, useUI } from "../lib/store";

function Dialog({ title, wide, onClose, children, foot }: { title: string; wide?: boolean; onClose: () => void; children: React.ReactNode; foot?: React.ReactNode }) {
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose]);
  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={`dialog ${wide ? "wide" : ""}`} role="dialog" aria-modal="true" aria-label={title}>
        <div className="dialog-head"><h2>{title}</h2><span className="spacer" /><button className="btn icon ghost" aria-label="Close" onClick={onClose}><X /></button></div>
        <div className="dialog-body">{children}</div>
        {foot && <div className="dialog-foot">{foot}</div>}
      </div>
    </div>
  );
}

function NewProjectDialog({ onClose, preset }: { onClose: () => void; preset?: string }) {
  const [name, setName] = useState("Untitled");
  const [pick, setPick] = useState(preset && PRESETS.some((p) => p.id === preset) ? preset : preset === "custom" ? "custom" : PRESETS[0].id);
  const [w, setW] = useState(1920);
  const [h, setH] = useState(1080);
  const [rate, setRate] = useState(2);
  const p = PRESETS.find((x) => x.id === pick);
  const width = p ? p.width : w;
  const height = p ? p.height : h;
  const [num, den] = p ? [p.num, p.den] : [RATES[rate][1], RATES[rate][2]];
  const sizeOk = [width, height].every((v) => Number.isInteger(v) && v >= 16 && v <= 8192 && v % 2 === 0);
  const create = async () => {
    if (!sizeOk || !name.trim()) return;
    if (await run(api.newProject({ name: name.trim(), width, height, fps_num: num, fps_den: den, sample_rate: 48000 }))) {
      useUI.setState({ playhead: 0, selection: [], previewFile: null, sourceMediaId: null, page: "media", screen: "editor" });
      onClose();
    }
  };
  return (
    <Dialog title="New project" onClose={onClose} foot={<><button className="btn ghost" onClick={onClose}>Cancel</button><button className="btn primary" onClick={create} disabled={!sizeOk || !name.trim()}>Create project</button></>}>
      <label className="field"><span>Name</span><input className="input" autoFocus value={name} onFocus={(e) => e.target.select()} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === "Enter" && create()} /></label>
      <div className="preset-row" role="radiogroup" aria-label="Timeline format">
        {PRESETS.map((x) => (
          <button key={x.id} role="radio" aria-checked={pick === x.id} className="preset" onClick={() => setPick(x.id)}>
            <FrameShape width={x.width} height={x.height} box={34} />
            <span><b>{x.name}</b><span className="num">{x.width}×{x.height} · {fpsLabel(x.num, x.den)} fps</span></span>
          </button>
        ))}
        <button role="radio" aria-checked={pick === "custom"} className="preset" onClick={() => setPick("custom")}>
          <FrameShape width={w} height={h} box={34} dashed />
          <span><b>Custom</b><span className="num">any size and rate</span></span>
        </button>
      </div>
      {pick === "custom" && (
        <div className="grid3">
          <label className="field"><span>Width</span><input className="input num" type="number" step={2} min={16} max={8192} value={w} onChange={(e) => setW(Number(e.target.value))} /></label>
          <label className="field"><span>Height</span><input className="input num" type="number" step={2} min={16} max={8192} value={h} onChange={(e) => setH(Number(e.target.value))} /></label>
          <label className="field"><span>Frame rate</span>
            <select className="select" value={rate} onChange={(e) => setRate(Number(e.target.value))}>{RATES.map(([l], i) => <option key={l} value={i}>{l} fps</option>)}</select>
          </label>
        </div>
      )}
      {!sizeOk && <p className="note err-text">Width and height must be even numbers between 16 and 8192.</p>}
      <p className="note">Frame rates are stored as exact fractions (24000/1001) and every edit counts whole frames. Audio is mixed at 48 kHz.</p>
    </Dialog>
  );
}
const statusClass = (s: string) =>
  s === "IMPLEMENTED AND TESTED" ? "st-tested" : s.startsWith("IMPLEMENTED,") ? "st-unverified" : s === "PROTOTYPE" ? "st-prototype" : s === "DEMO MODE" ? "st-demo" : s === "PLANNED" ? "st-planned" : "st-blocked";

function DiagnosticsDialog({ onClose, initial }: { onClose: () => void; initial?: "parity" | "deps" | "ofx" | "resolve" }) {
  const [d, setD] = useState<Diagnostics | null>(null);
  const [tab, setTab] = useState<"parity" | "deps" | "ofx" | "resolve">(initial ?? "parity");
  const [re, setRe] = useState<ResolveReport | null>(null);
  useEffect(() => { if (tab === "resolve" && !re) attempt(api.resolveInspect()).then((x) => x && setRe(x)); }, [tab]);
  useEffect(() => { attempt(api.diagnostics()).then((x) => x && setD(x)); }, []);
  return (
    <Dialog title="Diagnostics & feature status" wide onClose={onClose}>
      <div className="seg" role="tablist">
        <button aria-pressed={tab === "parity"} onClick={() => setTab("parity")}>Feature status</button>
        <button aria-pressed={tab === "deps"} onClick={() => setTab("deps")}>Dependencies</button>
        <button aria-pressed={tab === "ofx"} onClick={() => setTab("ofx")}>OpenFX plugins</button>
        <button aria-pressed={tab === "resolve"} onClick={() => setTab("resolve")}>Resolve binary &amp; interop inspector</button>
      </div>
      {!d && <p className="note">Checking FFmpeg, scanning plugin folders…</p>}
      {d && tab === "parity" && (
        <>
          <p className="note">Built-in feature registry (v{d.parity.registry_version}, updated {d.parity.updated}). Reference: {d.parity.reference}. A status describes Pirosman's own implementation of a workflow, based on code and test evidence; it is not a claim of parity with any other editor.</p>
          <div style={{ maxHeight: "56vh", overflow: "auto", border: "1px solid var(--ink-4)", borderRadius: 4 }}>
            <table className="tbl">
              <thead><tr><th>ID</th><th>Capability</th><th>Status</th><th>How Pirosman does it</th><th>Verification</th><th>Limitations</th></tr></thead>
              <tbody>
                {d.parity.features.map((f) => (
                  <tr key={f.id}>
                    <td className="num strong">{f.id}</td>
                    <td><div className="strong" style={{ color: "var(--bone)" }}>{f.subsystem}</div>{f.reference}<div className="note" style={{ marginTop: 4 }}>{f.evidence_type}: {f.evidence}</div></td>
                    <td><span className={`status-pill ${statusClass(f.status)}`}>{f.status}</span></td>
                    <td>{f.approach}<div className="note num" style={{ marginTop: 4 }}>{f.module}</div></td>
                    <td>{f.verification}</td>
                    <td>{f.limitations}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
      {d && tab === "deps" && (
        <dl className="kv" style={{ gridTemplateColumns: "160px 1fr" }}>
          <dt>FFmpeg</dt><dd className="text" style={{ textAlign: "left", whiteSpace: "normal" }}>{d.tools.found ? <span className="ok-text">Found</span> : <span className="err-text">{d.tools.error}</span>}</dd>
          <dt>ffmpeg</dt><dd style={{ textAlign: "left", whiteSpace: "normal" }}>{d.tools.ffmpeg_path ?? "—"}<br />{d.tools.ffmpeg_version}</dd>
          <dt>ffprobe</dt><dd style={{ textAlign: "left", whiteSpace: "normal" }}>{d.tools.ffprobe_path ?? "—"}</dd>
          <dt>Encoders available</dt><dd style={{ textAlign: "left" }}>{d.tools.encoders.join(", ") || "—"}</dd>
          <dt>App version</dt><dd style={{ textAlign: "left" }}>{d.app_version}</dd>
          <dt>Cache (proxies)</dt><dd style={{ textAlign: "left", whiteSpace: "normal" }}>{d.cache_dir}</dd>
          <dt>Revision store</dt><dd style={{ textAlign: "left", whiteSpace: "normal" }}>{d.history_dir ?? "Save the project to create one"}</dd>
        </dl>
      )}
      {tab === "resolve" && <ResolvePanel r={re} />}
      {d && tab === "ofx" && (
        <>
          <p className="note">{d.ofx.note}</p>
          <p className="note">Searched: {d.ofx.searched.join(" · ")}</p>
          {d.ofx.plugins.length === 0 ? <p className="note">No OpenFX bundles found in these folders.</p> : (
            <table className="tbl">
              <thead><tr><th>Plugin</th><th>Binaries (not loaded)</th><th>Status</th><th>Path</th></tr></thead>
              <tbody>{d.ofx.plugins.map((p) => <tr key={p.path}><td className="strong">{p.name}</td><td className="num">{p.binaries.join(", ") || "—"}</td><td>{p.status}</td><td className="num">{p.path}</td></tr>)}</tbody>
            </table>
          )}
        </>
      )}
    </Dialog>
  );
}

function DirtySwitchDialog({ branch, onClose }: { branch: string; onClose: () => void }) {
  return (
    <Dialog title={`Switch to ${branch}?`} onClose={onClose} foot={
      <>
        <button className="btn ghost" onClick={onClose}>Cancel</button>
        <button className="btn" onClick={() => { onClose(); useUI.setState({ page: "revisions" }); }}>Commit first</button>
        <button className="btn primary" onClick={async () => { onClose(); await run(api.switchBranch(branch, "stash"), `Switched to ${branch}; your changes were stashed`); }}>Stash changes and switch</button>
      </>
    }>
      <p>You have changes that are not in any revision. Switching replaces the working project with the head of <b>{branch}</b>.</p>
      <p className="note">Stashing writes the current working project to the history folder's <b>stash/</b> directory before switching, so nothing is lost.</p>
    </Dialog>
  );
}

const SHORTCUTS: [string, string][] = [
  ["Space / K", "Play or pause the timeline"], ["Left / Right", "Previous / next frame"], ["Shift+Left / Right", "Back / forward one second"],
  ["Home / End", "Start / end of timeline"], ["I / O", "Mark in / out on the active viewer (source clip or timeline range)"], ["Alt+X", "Clear the timeline in/out range"],
  ["F9 / F10", "Insert / overwrite the source range at the playhead"], ["M", "Add a marker at the playhead"],
  ["Ctrl+C / Ctrl+V / Ctrl+D", "Copy / paste at playhead / duplicate selected clips"],
  ["Ctrl+B", "Cut all tracks at the playhead"], ["B / A", "Blade tool / selection tool"], ["Delete", "Delete selected clips"], ["Shift+Delete", "Ripple delete"],
  ["Ctrl+Shift+C", "Commit a revision"], ["?", "This list"],
  ["Ctrl+Z / Ctrl+Shift+Z", "Undo / redo"], ["= / -", "Zoom timeline in / out"], ["Shift+Z", "Fit timeline"],
  ["Ctrl+S / Ctrl+Shift+S", "Save / save as"], ["Ctrl+O / Ctrl+N", "Open / new project"], ["Ctrl+I", "Import media"],
  ["Shift+1…5", "Media, Edit, Color, Revisions, Deliver"], ["Ctrl+Enter", "Commit (on the Revisions page)"],
];

export function DialogHost() {
  const dialog = useUI((s) => s.dialog);
  const close = () => useUI.setState({ dialog: null });
  if (!dialog) return null;
  if (dialog.kind === "new") return <NewProjectDialog onClose={close} preset={dialog.preset} />;
  if (dialog.kind === "diagnostics") return <DiagnosticsDialog onClose={close} initial={dialog.tab} />;
  if (dialog.kind === "prompt") return <PromptDialog d={dialog} onClose={close} />;
  if (dialog.kind === "commit") return <CommitDialog onClose={close} />;
  if (dialog.kind === "collab") return <CollabDialog onClose={close} />;
  if (dialog.kind === "about") return <AboutDialog onClose={close} />;
  if (dialog.kind === "dirty-switch") return <DirtySwitchDialog branch={dialog.branch} onClose={close} />;
  if (dialog.kind === "ffmpeg") return <FfmpegDialog onClose={close} />;
  return (
    <Dialog title="Keyboard shortcuts" onClose={close}>
      <table className="tbl"><tbody>{SHORTCUTS.map(([k, d]) => <tr key={k}><td className="num strong" style={{ whiteSpace: "nowrap" }}>{k}</td><td>{d}</td></tr>)}</tbody></table>
    </Dialog>
  );
}

/**
 * FFmpeg status. Pirosman Studio ships its own FFmpeg (in the app's ffmpeg
 * folder), so this only opens when that copy is missing or damaged, or from
 * Help → FFmpeg status. Nothing is installed from here.
 */
function FfmpegDialog({ onClose }: { onClose: () => void }) {
  const [t, setT] = useState<Diagnostics["tools"] | null>(null);
  const [busy, setBusy] = useState(false);
  const check = async () => {
    setBusy(true);
    setT((await attempt(api.ffmpegStatus())) ?? null);
    setBusy(false);
  };
  useEffect(() => { void check(); }, []);
  const ok = !!t?.found;
  const SOURCE = { bundled: "Bundled with Pirosman Studio", override: "PIROSMAN_FFMPEG_DIR (manual override)", system: "System installation (fallback)" } as const;
  return (
    <Dialog title={!t ? "Checking FFmpeg…" : ok ? "FFmpeg" : "FFmpeg could not be loaded"} onClose={onClose} foot={<>
      {t?.bundled_dir && <button className="btn ghost" onClick={() => attempt(api.revealFfmpegFolder())}>Open FFmpeg folder</button>}
      <button className="btn" onClick={check} disabled={busy}>{busy ? "Checking…" : "Check again"}</button>
      <button className="btn primary" onClick={onClose}>Close</button>
    </>}>
      {!t && <p className="note">Running the bundled ffmpeg and ffprobe self-check…</p>}
      {t && ok && (
        <dl className="kv" style={{ gridTemplateColumns: "120px 1fr" }}>
          <dt>Source</dt><dd className="text" style={{ textAlign: "left" }}>{t.source ? SOURCE[t.source] : "—"}</dd>
          <dt>Version</dt><dd className="text" style={{ textAlign: "left", whiteSpace: "normal" }}>{t.ffmpeg_version}</dd>
          <dt>ffmpeg</dt><dd className="text" style={{ textAlign: "left", whiteSpace: "normal", wordBreak: "break-all" }}>{t.ffmpeg_path}</dd>
          <dt>ffprobe</dt><dd className="text" style={{ textAlign: "left", whiteSpace: "normal", wordBreak: "break-all" }}>{t.ffprobe_path}</dd>
          <dt>Encoders</dt><dd className="text" style={{ textAlign: "left", whiteSpace: "normal" }}>{t.encoders.join(", ")}</dd>
        </dl>
      )}
      {t?.warning && <p className="note" style={{ color: "var(--warn)" }}>The bundled FFmpeg was not usable, so a system copy is in use: {t.warning}</p>}
      {t && !ok && (
        <>
          <p className="err-text" role="alert">{t.error}</p>
          <p>Importing, previews and rendering need FFmpeg. Projects, revisions and Live Sync still work.</p>
          <ul className="mark-list">
            <li>Pirosman Studio normally includes FFmpeg in{t.bundled_dir ? <> <code>{t.bundled_dir}</code></> : " its ffmpeg folder"}. Antivirus quarantine or an incomplete copy can remove it.</li>
            <li><b>Installer:</b> run the Pirosman Studio setup again (or Repair). <b>Portable:</b> extract the whole zip again, keeping the <code>ffmpeg</code> folder next to the exe.</li>
          </ul>
        </>
      )}
    </Dialog>
  );
}

function ResolvePanel({ r }: { r: ResolveReport | null }) {
  if (!r) return <p className="note">Reading PE headers and version resources of the local DaVinci Resolve install…</p>;
  if (!r.found) return <p className="note">No DaVinci Resolve installation found at {r.install_dir}. Set PIROSMAN_RESOLVE_DIR to inspect another location.</p>;
  return (
    <div style={{ display: "grid", gap: 12, maxHeight: "60vh", overflow: "auto" }}>
      <p className="note">{r.notes.join(" ")}</p>
      <dl className="kv" style={{ gridTemplateColumns: "190px 1fr" }}>
        <dt>Install</dt><dd style={{ textAlign: "left" }}>{r.install_dir}</dd>
        <dt>Product version</dt><dd style={{ textAlign: "left" }}>{r.product_version ?? "—"}</dd>
        <dt>DLLs in install root</dt><dd style={{ textAlign: "left" }}>{r.dll_count}</dd>
        <dt>Bundled libraries</dt><dd className="text" style={{ textAlign: "left", whiteSpace: "normal" }}>{r.bundled_libraries.join(", ")}</dd>
        <dt>Fusion plugins</dt><dd className="text" style={{ textAlign: "left", whiteSpace: "normal" }}>{r.fusion_plugins.join(", ") || "—"}</dd>
        <dt>.cube LUTs found</dt><dd style={{ textAlign: "left" }}>{r.lut_files} in {r.lut_dirs.length} folders</dd>
        <dt>OpenFX bundles</dt><dd className="text" style={{ textAlign: "left" }}>{r.ofx_bundles.join(", ") || "—"}</dd>
        <dt>Documented export types</dt><dd className="text" style={{ textAlign: "left", whiteSpace: "normal" }}>{r.documented_export_types.map((x) => x.replace("EXPORT_", "")).join(", ") || "—"}</dd>
      </dl>
      <table className="tbl">
        <thead><tr><th>Binary</th><th>Role</th><th>Size</th><th>Version</th><th>Linked</th><th>Exports</th><th>Export names (sample)</th></tr></thead>
        <tbody>
          {r.binaries.map((b) => (
            <tr key={b.file}>
              <td className="num strong">{b.file}</td>
              <td>{b.role}</td>
              <td className="num">{(b.size / 1048576).toFixed(1)} MB</td>
              <td className="num">{b.pe?.file_version ?? "—"}</td>
              <td className="num">{b.pe ? new Date(b.pe.timestamp * 1000).toISOString().slice(0, 10) : "—"}</td>
              <td className="num">{b.pe?.export_count ?? b.error}</td>
              <td className="num" style={{ maxWidth: 360, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={b.pe?.exports_sample.join("\n")}>{b.pe?.exports_sample.slice(0, 2).join("  ·  ")}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function PromptDialog({ d, onClose }: { d: { title: string; label: string; initial: string; confirm: string; multiline?: boolean; onSubmit: (v: string) => void }; onClose: () => void }) {
  const [v, setV] = useState(d.initial);
  const submit = () => { if (!v.trim()) return; onClose(); d.onSubmit(v); };
  return (
    <Dialog title={d.title} onClose={onClose} foot={<><button className="btn ghost" onClick={onClose}>Cancel</button><button className="btn primary" disabled={!v.trim()} onClick={submit}>{d.confirm}</button></>}>
      <label className="field"><span>{d.label}</span>
        {d.multiline ? (
          <textarea className="textarea" rows={3} autoFocus value={v} onChange={(e) => setV(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter" && e.ctrlKey) submit(); }} />
        ) : (
          <input className="input" autoFocus value={v} onChange={(e) => setV(e.target.value)} onKeyDown={(e) => e.key === "Enter" && submit()} onFocus={(e) => e.target.select()} />
        )}
      </label>
    </Dialog>
  );
}

function CommitDialog({ onClose }: { onClose: () => void }) {
  const view = useUI((s) => s.view);
  const author = useUI((s) => s.author);
  const [msg, setMsg] = useState("");
  const changes = view?.vcs?.head ? view.vcs.changes : [];
  const first = !view?.vcs?.head;
  const commit = async () => {
    if (!msg.trim()) return;
    if (await run(api.commit(msg, author), "Revision created")) onClose();
  };
  return (
    <Dialog title={`Commit to ${view?.vcs?.branch ?? "main"}`} onClose={onClose} foot={<><button className="btn ghost" onClick={onClose}>Cancel</button><button className="btn primary" disabled={!msg.trim() || (!first && changes.length === 0)} onClick={commit}>Commit</button></>}>
      <p className="note">{first ? "First revision: records the whole project." : `${changes.length} change${changes.length === 1 ? "" : "s"} since ${view?.vcs?.head?.slice(0, 7)}:`}</p>
      {!first && <ul className="mark-list" style={{ maxHeight: 180, overflow: "auto" }}>{changes.slice(0, 40).map((c, i) => <li key={i}>{c.summary}</li>)}</ul>}
      <label className="field"><span>Describe this cut</span>
        <textarea className="textarea" rows={2} autoFocus value={msg} onChange={(e) => setMsg(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter" && e.ctrlKey) commit(); }} />
      </label>
    </Dialog>
  );
}

function CollabDialog({ onClose }: { onClose: () => void }) {
  const st = useCollab();
  const [url, setUrl] = useState(st.url ?? "ws://127.0.0.1:");
  const [code, setCode] = useState(st.code ?? "");
  const [name, setName] = useState(st.me.name);
  const [busy, setBusy] = useState(false);
  const doHost = async () => {
    setBusy(true);
    setMyName(name);
    try { await host(); } catch (e) { useUI.getState().toast("error", String(e)); }
    setBusy(false);
  };
  const h = st.hosting;
  return (
    <Dialog title="Live Sync" onClose={onClose} foot={
      <>
        <button className="btn ghost" onClick={() => { setMode("revisions"); onClose(); }}>Back to Revisions mode</button>
        <span className="spacer" />
        <button className="btn" onClick={onClose}>Done</button>
      </>
    }>
      <p className="note">Live Sync shares every edit with everyone in the session as it happens, with live cursors and playheads. Edits travel as whole-project snapshots (the last edit wins); for reviewable history switch back to Revisions mode and commit.</p>
      <label className="field"><span>Your name</span><input className="input" value={name} onChange={(e) => setName(e.target.value)} onBlur={() => setMyName(name)} /></label>
      {st.status === "live" ? (
        <p className="ok-text" style={{ margin: 0 }}>Connected to {st.url} · {Object.keys(st.peers).length} other collaborator(s)</p>
      ) : st.error ? <p className="err-text" style={{ margin: 0 }}>{st.error}</p> : null}
      <section className="insp-sec">
        <span className="label">Host a session</span>
        {!h ? (
          <button className="btn primary" disabled={busy} onClick={doHost}><Radio />Start hosting</button>
        ) : (
          <>
            <dl className="kv" style={{ gridTemplateColumns: "120px 1fr" }}>
              <dt>Session code</dt><dd style={{ textAlign: "left", fontSize: 18, letterSpacing: ".2em", color: "var(--marker)" }}>{h.code}</dd>
              <dt>On this machine</dt><dd style={{ textAlign: "left" }}>{h.local_url}</dd>
              {h.lan_urls.map((u) => <><dt key={`d${u}`}>On your network</dt><dd key={u} style={{ textAlign: "left" }}>{u}</dd></>)}
            </dl>
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
              <button className="btn" onClick={() => navigator.clipboard.writeText(`${h.lan_urls[0] ?? h.local_url}  code ${h.code}`)}><Copy />Copy invite</button>
              <button className="btn" onClick={() => api.collabSpawnPeer("Collaborator 2").then(() => useUI.getState().toast("info", "Opening a second collaborator window…")).catch((e) => useUI.getState().toast("error", String(e)))}><MonitorUp />Open 2nd collaborator window</button>
            </div>
          </>
        )}
      </section>
      <section className="insp-sec">
        <span className="label">Join a session</span>
        <div className="grid2">
          <label className="field"><span>Host address</span><input className="input" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="ws://192.168.1.20:41234" /></label>
          <label className="field"><span>Session code</span><input className="input" value={code} onChange={(e) => setCode(e.target.value.toUpperCase())} placeholder="ABC234" /></label>
        </div>
        <div><button className="btn" disabled={!url.startsWith("ws") || code.length < 4} onClick={() => { setMyName(name); setMode("live"); connect(url.trim(), code.trim()); }}>Join</button></div>
      </section>
    </Dialog>
  );
}

function AboutDialog({ onClose }: { onClose: () => void }) {
  return (
    <Dialog title="About Pirosman Studio" onClose={onClose}>
      <div style={{ display: "flex", gap: 16, alignItems: "center" }}>
        <BrandMark size={96} />
        <div>
          <div style={{ fontSize: 18, fontWeight: 700 }}>Pirosman Studio</div>
          <p className="note">An open-source video editor with version control and live collaboration built into the cut. Named in honour of Niko Pirosmani. Not affiliated with Blackmagic Design.</p>
        </div>
      </div>
    </Dialog>
  );
}
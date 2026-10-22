import { useEffect, useMemo, useState } from "react";
import {
  ArrowRightLeft, Download, GitBranch, GitCommitHorizontal, GitMerge, History, MinusCircle, PencilLine, PlusCircle, RotateCcw, Save, Trash2, Upload,
} from "lucide-react";
import { api, type Conflict, type DiffEntry, type RevisionSummary, type Side, type VcsState } from "../lib/api";
import { ask } from "@tauri-apps/plugin-dialog";
import { exportBundleFlow, importBundleFlow, saveFlow } from "../lib/actions";
import { showContextMenu } from "../lib/contextMenu";
import { attempt, errText, run, useUI } from "../lib/store";
import { relTime } from "../lib/time";

const LANE_COLORS = ["#f2efe9", "#5fb59a", "#6f9fd8", "#d9a35a", "#b9a0d6", "#94a6c4"];
const ROW_H = 52;
const LANE_W = 16;

interface GraphRow { lane: number; before: (string | null)[]; after: (string | null)[]; merges: number[]; joins: number[] }

function layout(log: RevisionSummary[]): GraphRow[] {
  let lanes: (string | null)[] = [];
  const rows: GraphRow[] = [];
  for (const r of log) {
    const before = lanes.slice();
    let idx = before.indexOf(r.id);
    if (idx < 0) {
      idx = before.indexOf(null);
      if (idx < 0) { idx = before.length; before.push(null); }
    }
    const joins = before.map((x, j) => (x === r.id && j !== idx ? j : -1)).filter((j) => j >= 0);
    const after = before.slice();
    for (const j of joins) after[j] = null;
    after[idx] = r.parents[0] ?? null;
    const merges: number[] = [];
    for (const p of r.parents.slice(1)) {
      let k = after.indexOf(p);
      if (k < 0) {
        k = after.indexOf(null);
        if (k < 0) { k = after.length; after.push(null); }
        after[k] = p;
      }
      merges.push(k);
    }
    while (after.length && after[after.length - 1] === null) after.pop();
    rows.push({ lane: idx, before, after, merges, joins });
    lanes = after;
  }
  return rows;
}

function GraphCell({ row, width }: { row: GraphRow; width: number }) {
  const x = (j: number) => j * LANE_W + LANE_W / 2 + 4;
  const mid = ROW_H / 2;
  const col = (j: number) => LANE_COLORS[j % LANE_COLORS.length];
  const paths: JSX.Element[] = [];
  row.before.forEach((id, j) => {
    if (id === null) return;
    const to = row.joins.includes(j) || j === row.lane ? row.lane : j;
    if (j === row.lane && !row.before.includes(id)) return;
    paths.push(<path key={`t${j}`} d={`M${x(j)},0 C${x(j)},${mid * 0.6} ${x(to)},${mid * 0.4} ${x(to)},${mid}`} stroke={col(j)} />);
  });
  row.after.forEach((id, j) => {
    if (id === null) return;
    const from = j === row.lane || row.merges.includes(j) ? row.lane : j;
    if (from === j && j !== row.lane && row.before[j] === null) return;
    paths.push(<path key={`b${j}`} d={`M${x(from)},${mid} C${x(from)},${mid + mid * 0.6} ${x(j)},${mid + mid * 0.4} ${x(j)},${ROW_H}`} stroke={col(j)} />);
  });
  return (
    <svg width={width} height={ROW_H} className="graph" aria-hidden="true">
      <g fill="none" strokeWidth="2">{paths}</g>
      <circle cx={x(row.lane)} cy={mid} r={row.merges.length ? 6 : 5} fill="var(--ink-2)" stroke={col(row.lane)} strokeWidth="2.5" />
      {row.merges.length > 0 && <circle cx={x(row.lane)} cy={mid} r={2} fill={col(row.lane)} />}
    </svg>
  );
}

const OP_WORD: Record<string, string> = {
  ClipAdded: "added", ClipRemoved: "removed", ClipTrimmed: "trimmed", ClipMoved: "moved", ClipTrackChanged: "moved", ClipNameChanged: "renamed",
  ColorOrEffectModified: "graded", AudioGainModified: "gain", ClipMediaChanged: "relinked", TrackAdded: "track added", TrackRemoved: "track removed",
  TrackModified: "track", TrackReordered: "tracks reordered", ProjectSettingsModified: "settings", MediaReferenceChanged: "media", ExtraFieldModified: "other data",
};

function iconFor(op: string) {
  if (op.endsWith("Added")) return { cls: "diff-add", Icon: PlusCircle };
  if (op.endsWith("Removed")) return { cls: "diff-del", Icon: MinusCircle };
  return { cls: "diff-mod", Icon: PencilLine };
}

export function DiffList({ items, empty }: { items: DiffEntry[] | null; empty: string }) {
  if (!items) return <p className="note" style={{ padding: 14 }}>Comparing…</p>;
  if (!items.length) return <p className="note" style={{ padding: 14 }}>{empty}</p>;
  return (
    <ul className="diff-list">
      {items.map((d, i) => {
        const { cls, Icon } = iconFor(d.op);
        return (
          <li key={i} className={cls}>
            <Icon />
            <div>{d.summary}<span className="op">{OP_WORD[d.op] ?? "changed"}</span></div>
          </li>
        );
      })}
    </ul>
  );
}

const pretty = (v: unknown) => {
  if (v === "<absent>") return "(deleted)";
  if (v === null || v === undefined) return "—";
  if (typeof v === "object") {
    const o = v as Record<string, unknown>;
    if ("source_in" in o && "start" in o) return `start ${o.start}, in ${o.source_in}, out ${o.source_out}${o.track_id ? `\ntrack ${String(o.track_id).slice(0, 8)}` : ""}`;
    return JSON.stringify(v, null, 1).replace(/[{}"]/g, "").trim();
  }
  return String(v);
};

function ConflictCard({ c, pending, choose }: { c: Conflict; pending: Side | undefined; choose: (s: Side) => void }) {
  const side = pending ?? c.resolution;
  return (
    <div className="conflict">
      <div className="conflict-head"><b>{c.label}</b><span>{c.property}</span></div>
      <div className="conflict-vals">
        <div><span className="label">Base</span><pre>{pretty(c.base)}</pre></div>
        <div className={side === "ours" ? "chosen" : ""}><span className="label">Ours</span><pre>{pretty(c.ours)}</pre></div>
        <div className={side === "theirs" ? "chosen" : ""}><span className="label">Theirs</span><pre>{pretty(c.theirs)}</pre></div>
      </div>
      <div className="conflict-foot">
        <p className="note">{c.explanation}</p>
        <button className={`btn small ${side === "ours" ? "on" : ""}`} onClick={() => choose("ours")}>Keep ours</button>
        <button className={`btn small ${side === "theirs" ? "on" : ""}`} onClick={() => choose("theirs")}>Keep theirs</button>
      </div>
    </div>
  );
}

function MergePanel({ state, refresh }: { state: VcsState; refresh: () => void }) {
  const m = state.merge!;
  const author = useUI((s) => s.author);
  const [pending, setPending] = useState<Record<string, Side>>({});
  const conflicts = m.outcome.conflicts;
  const unresolved = conflicts.filter((c) => !c.resolution && !pending[c.id]).length;
  const apply = async () => { if (await run(api.mergeResolve(pending))) { setPending({}); refresh(); } };
  return (
    <div className="panel" aria-label="Merge">
      <div className="panel-head"><GitMerge size={14} color="var(--marker)" /><span className="label">Merging <b>{m.theirs_branch}</b> into <b>{state.current}</b></span></div>
      <div className="panel-body">
        <div className="insp">
          <section className="insp-sec">
            <span className="label">Incoming changes (base → {m.theirs_branch})</span>
            <DiffList items={m.incoming} empty="No changes on the incoming side." />
          </section>
          {m.outcome.auto_merged.length > 0 && (
            <section className="insp-sec">
              <span className="label">Merged automatically · {m.outcome.auto_merged.length}</span>
              <ul className="mark-list">{m.outcome.auto_merged.slice(0, 30).map((s, i) => <li key={i}>{s}</li>)}</ul>
            </section>
          )}
          <section className="insp-sec">
            <span className="label">Conflicts · {conflicts.length}{unresolved ? ` · ${unresolved} unresolved` : ""}</span>
            {conflicts.length === 0 && <p className="note">No conflicts. Review the result on the Edit page, then commit the merge.</p>}
            {conflicts.map((c) => <ConflictCard key={c.id} c={c} pending={pending[c.id]} choose={(s) => setPending({ ...pending, [c.id]: s })} />)}
          </section>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            {Object.keys(pending).length > 0 && <button className="btn" onClick={apply}>Apply {Object.keys(pending).length} choice{Object.keys(pending).length > 1 ? "s" : ""}</button>}
            <button className="btn primary" disabled={!m.outcome.merged || Object.keys(pending).length > 0} onClick={async () => { if (await run(api.mergeCommit("", author), "Merge committed")) refresh(); }} title={m.outcome.merged ? "Create a revision with two parents" : "Resolve every conflict first"}>
              <GitMerge />Commit merge
            </button>
            <button className="btn danger" onClick={async () => { if (await run(api.mergeAbort(), "Merge aborted")) refresh(); }}>Abort merge</button>
          </div>
          {!m.outcome.merged && conflicts.length > 0 && unresolved === 0 && Object.keys(pending).length === 0 && (
            <p className="note err-text">The chosen combination still produces an invalid timeline; new conflicts are listed above.</p>
          )}
        </div>
      </div>
    </div>
  );
}

export function RevisionsWorkspace() {
  const view = useUI((s) => s.view);
  const tick = useUI((s) => s.vcsTick);
  const author = useUI((s) => s.author);
  const set = useUI((s) => s.set);
  const [state, setState] = useState<VcsState | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [picked, setSelected] = useState<string>("working");
  const [diff, setDiff] = useState<DiffEntry[] | null>(null);
  const [message, setMessage] = useState("");
  const [newBranch, setNewBranch] = useState("");
  const [bundleSel, setBundleSel] = useState<string[]>([]);
  const [withMedia, setWithMedia] = useState(false);

  const refresh = () => { api.vcsState().then((s) => { setState(s); setErr(null); }).catch((e) => setErr(errText(e))); };
  useEffect(() => { if (view?.path) refresh(); }, [tick, view?.path]);

  const log = state?.log ?? [];
  const rows = useMemo(() => layout(log), [log]);
  const graphW = Math.max(1, ...rows.map((r) => Math.max(r.before.length, r.after.length, r.lane + 1))) * LANE_W + 8;
  const head = view?.vcs?.head ?? null;
  const dirty = !!view?.vcs && (!head || view.vcs.changes.length > 0);
  // With a clean working copy, show the head revision instead of an empty diff.
  const selected = picked === "working" && !dirty && head ? head : picked;
  const sel = log.find((r) => r.id === selected);

  useEffect(() => {
    if (!view?.path || !state) return;
    setDiff(null);
    if (selected === "working") {
      if (!head) { setDiff([]); return; }
      setDiff(view.vcs?.changes ?? []);
    } else if (sel) {
      if (sel.parents[0]) attempt(api.diff(sel.parents[0], sel.id)).then((d) => setDiff(d ?? []));
      else setDiff([]);
    }
  }, [selected, state, view?.vcs?.changes]);

  if (!view) return null;
  if (!view.path) {
    return (
      <div className="page"><div className="panel"><div className="empty">
        <History size={28} color="var(--marker)" />
        <h3>Save the project to start its history</h3>
        <p>Revisions live next to the project file in a <b>.history</b> folder: immutable snapshots, branches and stashes. Media is referenced by content hash, never duplicated.</p>
        <button className="btn primary" onClick={() => saveFlow(true)}><Save />Save project…</button>
      </div></div></div>
    );
  }

  const commit = async () => {
    if (!message.trim()) { useUI.getState().toast("error", "Describe the revision first."); return; }
    if (await run(api.commit(message, author), "Revision created")) { setMessage(""); setSelected("working"); }
  };
  const switchTo = (name: string) => {
    if (dirty && head) set({ dialog: { kind: "dirty-switch", branch: name } });
    else run(api.switchBranch(name, "require_clean"), `Switched to ${name}`);
  };
  const branches = state?.branches ?? [];
  const mergeBranch = async (name: string) => {
    const r = await attempt(api.mergeStart(name));
    if (!r) return;
    useUI.getState().setView(r.view);
    useUI.getState().toast(r.result === "conflicts" ? "info" : "ok", r.result === "up_to_date" ? `${state?.current} already contains ${name}` : r.result === "fast_forward" ? `Fast-forwarded to ${name}` : r.result === "merged" ? "Merged without conflicts; review and commit the merge" : "Merge has conflicts to resolve");
    refresh();
  };
  const deleteBranch = async (name: string) => {
    try { useUI.getState().setView(await api.deleteBranch(name, false)); }
    catch (e) {
      const msg = errText(e);
      if (!msg.includes("no other branch")) { useUI.getState().toast("error", msg); return; }
      if (await ask(`${msg}\n\nDelete ${name} anyway?`, { title: "Delete branch", kind: "warning", okLabel: "Delete branch", cancelLabel: "Keep it" })) await run(api.deleteBranch(name, true));
    }
  };
  const branchMenu = (e: React.MouseEvent, name: string, current: boolean, hasHead: boolean) =>
    showContextMenu(e, [
      { text: current ? `${name} (current)` : `Switch to ${name}`, enabled: !current && hasHead, action: () => switchTo(name) },
      { text: `Merge ${name} into ${state?.current}`, enabled: !current && hasHead && !!head && !state?.merge, action: () => mergeBranch(name) },
      { text: `Compare ${name} with working project`, enabled: hasHead, action: () => attempt(api.diff(name, "working")).then((d) => { if (d) { setSelected("working"); setDiff(d); } }) },
      "sep",
      { text: `Export ${name} as bundle…`, enabled: hasHead, action: () => exportBundleFlow([name], false) },
      { text: `Export ${name} with media…`, enabled: hasHead, action: () => exportBundleFlow([name], true) },
      "sep",
      { text: `Delete ${name}…`, enabled: !current, action: () => deleteBranch(name) },
    ]);
  const revisionMenu = (e: React.MouseEvent, r: RevisionSummary) => {
    setSelected(r.id);
    showContextMenu(e, [
      { text: "Restore into working project", enabled: !state?.merge, action: () => run(api.restore(r.id), `Restored ${r.id.slice(0, 7)} into the working project`) },
      { text: "Compare with working project", action: () => attempt(api.diff(r.id, "working")).then((d) => d && setDiff(d)) },
      { text: "Compare with parent", enabled: r.parents.length > 0, action: () => attempt(api.diff(r.parents[0], r.id)).then((d) => d && setDiff(d)) },
      "sep",
      { text: "New branch here…", action: async () => { const name = window.prompt("Name the new branch", `from-${r.id.slice(0, 7)}`); if (name) await run(api.createBranch(name, r.id, false), `Branch ${name} created at ${r.id.slice(0, 7)}`); } },
      { text: "Copy revision id", action: () => navigator.clipboard.writeText(r.id) },
    ]);
  };

  return (
    <div className="page" style={{ gridTemplateColumns: "270px minmax(420px, 1.1fr) minmax(380px, 1fr)" }}>
      {/* Branches & exchange */}
      <div className="panel" aria-label="Branches">
        <div className="panel-head"><span className="label">Branches · <b>{branches.length}</b></span></div>
        <div className="panel-body">
          <ul className="branch-list">
            {branches.map((b) => (
              <li key={b.name} className={b.current ? "current" : ""} onContextMenu={(e) => branchMenu(e, b.name, b.current, !!b.head)}>
                <GitBranch />
                <button className="btn ghost small" style={{ justifyContent: "flex-start", padding: 0, color: "inherit", fontWeight: b.current ? 700 : 400 }} disabled={b.current || !b.head} onClick={() => switchTo(b.name)} title={b.current ? "Current branch" : `Switch to ${b.name}`}>
                  {b.name}
                </button>
                <span style={{ display: "flex", gap: 2 }}>
                  {!b.current && b.head && head && <button className="btn icon small ghost" title={`Merge ${b.name} into ${state?.current}`} onClick={() => mergeBranch(b.name)}><GitMerge /></button>}
                  {!b.current && <button className="btn icon small ghost" title={`Delete ${b.name}`} onClick={() => deleteBranch(b.name)}><Trash2 /></button>}
                </span>
              </li>
            ))}
          </ul>
          <div className="insp" style={{ paddingTop: 4 }}>
            <section className="insp-sec">
              <span className="label">New branch from {state?.current}</span>
              <div style={{ display: "flex", gap: 6 }}>
                <input className="input" placeholder="alt-ending" value={newBranch} onChange={(e) => setNewBranch(e.target.value)} onKeyDown={(e) => e.key === "Enter" && newBranch && run(api.createBranch(newBranch, null, true), `Now on ${newBranch}`).then((ok) => ok && setNewBranch(""))} />
                <button className="btn" disabled={!newBranch || !head} onClick={() => run(api.createBranch(newBranch, null, true), `Now on ${newBranch}`).then((ok) => ok && setNewBranch(""))}>Create</button>
              </div>
              {!head && <p className="note">Commit a first revision to branch from it.</p>}
            </section>
            <section className="insp-sec">
              <span className="label">Exchange</span>
              <p className="note">A <b>.pirosman-bundle</b> carries revisions (and optionally media) to a collaborator. Imports arrive as <b>incoming/…</b> branches you can compare and merge.</p>
              <div style={{ display: "grid", gap: 4 }}>
                {branches.filter((b) => b.head).map((b) => (
                  <label className="check" key={b.name}><input type="checkbox" checked={bundleSel.includes(b.name)} onChange={(e) => setBundleSel(e.target.checked ? [...bundleSel, b.name] : bundleSel.filter((x) => x !== b.name))} />{b.name}</label>
                ))}
                <label className="check"><input type="checkbox" checked={withMedia} onChange={(e) => setWithMedia(e.target.checked)} />Include media files</label>
              </div>
              <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
                <button className="btn" disabled={!bundleSel.length} onClick={() => exportBundleFlow(bundleSel, withMedia)}><Upload />Export bundle…</button>
                <button className="btn" onClick={importBundleFlow}><Download />Import bundle…</button>
              </div>
            </section>
          </div>
        </div>
      </div>

      {/* History */}
      <div className="panel" aria-label="Revision history">
        <div className="panel-head"><span className="label">History · <b>{state?.current}</b></span><span className="spacer" /><span className="note">{log.length} revision{log.length === 1 ? "" : "s"}</span></div>
        <div style={{ padding: 10, borderBottom: "1px solid var(--ink-4)", display: "grid", gap: 8 }}>
          <textarea className="textarea" rows={2} placeholder={dirty ? "Describe what changed in this cut…" : "No changes to commit"} value={message} disabled={!dirty || !!state?.merge}
            onChange={(e) => setMessage(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter" && e.ctrlKey) commit(); }} aria-label="Revision description" />
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <input className="input" style={{ width: 180 }} placeholder="Author" value={author} aria-label="Author"
              onChange={(e) => { set({ author: e.target.value }); try { localStorage.setItem("pirosman.author", e.target.value); } catch { /* storage unavailable */ } }} />
            <span className="spacer" />
            <span className="note">{dirty ? (head ? `${view.vcs?.changes.length} change${view.vcs?.changes.length === 1 ? "" : "s"} since ${head.slice(0, 7)}` : "First revision") : "Working copy matches HEAD"}</span>
            <button className="btn primary" disabled={!dirty || !message.trim() || !!state?.merge} onClick={commit} title="Commit (Ctrl+Enter)"><GitCommitHorizontal />Commit</button>
          </div>
        </div>
        <div className="panel-body" role="listbox" aria-label="Revisions">
          {err && <p className="note err-text" style={{ padding: 12 }}>{err}</p>}
          {dirty && (
            <div className="revrow" role="option" aria-selected={selected === "working"} style={{ ["--graph-w" as string]: `${graphW}px` }} onClick={() => setSelected("working")}>
              <svg width={graphW} height={ROW_H} aria-hidden="true"><circle cx={LANE_W / 2 + 4} cy={ROW_H / 2} r={5} fill="none" stroke="var(--marker)" strokeWidth="2" strokeDasharray="3 2" />{head && <path d={`M${LANE_W / 2 + 4},${ROW_H / 2 + 6} V${ROW_H}`} stroke="var(--marker)" strokeWidth="2" strokeDasharray="3 3" />}</svg>
              <div style={{ minWidth: 0 }}>
                <div className="msg" style={{ color: "var(--marker)" }}>Uncommitted changes</div>
                <div className="meta"><span>working copy</span>{head && <span className="num">{view.vcs?.changes.length} change{view.vcs?.changes.length === 1 ? "" : "s"}</span>}</div>
              </div>
              <div className="tags" />
            </div>
          )}
          {log.length === 0 && !err && <div className="empty" style={{ height: "auto", paddingTop: 40 }}><h3>No revisions yet</h3><p>Write a description above and commit to create the first immutable snapshot of this cut.</p></div>}
          {log.map((r, i) => (
            <div key={r.id} className="revrow" role="option" aria-selected={selected === r.id} style={{ ["--graph-w" as string]: `${graphW}px` }} onClick={() => setSelected(r.id)} onContextMenu={(e) => revisionMenu(e, r)}>
              <GraphCell row={rows[i]} width={graphW} />
              <div style={{ minWidth: 0 }}>
                <div className="msg">{r.message}</div>
                <div className="meta"><span className="num">{r.id.slice(0, 7)}</span><span>{r.author}</span><span>{relTime(r.created_at)}</span>{r.parents.length > 1 && <span>merge</span>}</div>
              </div>
              <div className="tags">{r.branches.map((b) => <span key={b} className={`bref ${b === state?.current ? "current" : b.startsWith("incoming/") ? "incoming" : ""}`}>{b}</span>)}{r.id === head && !r.branches.includes(state?.current ?? "") && <span className="bref current">HEAD</span>}</div>
            </div>
          ))}
        </div>
      </div>

      {/* Details / diff / merge */}
      {state?.merge ? <MergePanel state={state} refresh={refresh} /> : (
        <div className="panel" aria-label="Revision details">
          <div className="panel-head">
            <span className="label">{selected === "working" ? <>Changes since <b>{head ? head.slice(0, 7) : "nothing"}</b></> : <>Revision <b>{selected.slice(0, 7)}</b></>}</span>
          </div>
          <div className="panel-body">
            {sel && (
              <div className="insp" style={{ paddingBottom: 6 }}>
                <section className="insp-sec">
                  <div style={{ fontSize: 15, fontWeight: 600 }}>{sel.message}</div>
                  <dl className="kv">
                    <dt>Author</dt><dd className="text">{sel.author}</dd>
                    <dt>Created</dt><dd className="text">{new Date(sel.created_at).toLocaleString()}</dd>
                    <dt>Revision</dt><dd title={sel.id}>{sel.id.slice(0, 16)}…</dd>
                    <dt>Parents</dt><dd>{sel.parents.map((p) => p.slice(0, 7)).join(" + ") || "none (root)"}</dd>
                    <dt>Clips</dt><dd>{sel.clip_count}</dd>
                  </dl>
                </section>
                <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
                  <button className="btn primary" disabled={!!state?.merge} onClick={() => run(api.restore(sel.id), `Restored ${sel.id.slice(0, 7)} into the working project`)} title="Copy this snapshot into the working project; history is untouched and uncommitted work is stashed">
                    <RotateCcw />Restore
                  </button>
                  <button className="btn" onClick={() => attempt(api.diff(sel.id, "working")).then((d) => d && setDiff(d))}><ArrowRightLeft />Compare with working</button>
                  <button className="btn" onClick={async () => {
                    const name = window.prompt("Name the new branch", `from-${sel.id.slice(0, 7)}`);
                    if (name) await run(api.createBranch(name, sel.id, false), `Branch ${name} created at ${sel.id.slice(0, 7)}`);
                  }}><GitBranch />Branch here</button>
                </div>
              </div>
            )}
            <div style={{ borderTop: sel ? "1px solid var(--ink-4)" : undefined }}>
              <DiffList items={diff} empty={selected === "working" ? (head ? "The working project matches the last revision." : "Nothing committed yet; your first commit records the whole project.") : sel && !sel.parents.length ? "Root revision: the starting point of this history." : "No differences."} />
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

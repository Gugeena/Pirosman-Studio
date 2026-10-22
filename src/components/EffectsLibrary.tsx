import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, ChevronDown, ChevronRight, Plus, Puzzle, RefreshCw, Search, Sparkles } from "lucide-react";
import type { OfxPlugin, VfxKind } from "../lib/api";
import { beginDrag } from "../lib/dnd";
import { applyEffect, loadOfxCatalog } from "../lib/ofx";
import { prefs, useUI, type DragPayload } from "../lib/store";
import { VFX_LABEL } from "./Inspector";

const BUILTIN = "Built-in";

interface Item { key: string; label: string; vendor: string; category: string; payload: DragPayload; plugin?: OfxPlugin }

/**
 * Effects library: built-in FFmpeg effects plus every installed OpenFX plugin,
 * one tab per vendor (Sapphire, BCC, …) and one section per plugin group.
 * Drag an effect onto a timeline clip, or double-click it to add it to the selected clips.
 */
export function EffectsLibrary({ switcher }: { switcher?: React.ReactNode }) {
  const cat = useUI((s) => s.ofx);
  const scanning = useUI((s) => s.ofxScanning);
  const selection = useUI((s) => s.selection);
  const [tab, setTab] = useState(prefs.get("fxTab", BUILTIN));
  const [q, setQ] = useState("");
  const [closed, setClosed] = useState<Record<string, boolean>>({});
  const [showFailures, setShowFailures] = useState(false);
  useEffect(() => { if (!cat) void loadOfxCatalog(); }, [cat]);

  const items = useMemo<Item[]>(() => {
    const builtin: Item[] = (Object.keys(VFX_LABEL) as VfxKind[]).map((k) => ({
      key: `vfx:${k}`, label: VFX_LABEL[k], vendor: BUILTIN, category: "Video effects", payload: { kind: "vfx", vfx: k, label: VFX_LABEL[k] },
    }));
    const ofx: Item[] = (cat?.plugins ?? []).map((p) => ({
      key: `ofx:${p.binary}:${p.index}`, label: p.label, vendor: p.vendor, category: p.category, plugin: p, payload: { kind: "ofx", plugin: p, label: p.label },
    }));
    return [...builtin, ...ofx];
  }, [cat]);

  const vendors = useMemo(() => {
    const m = new Map<string, number>();
    for (const it of items) if (it.vendor !== BUILTIN) m.set(it.vendor, (m.get(it.vendor) ?? 0) + 1);
    return [...m.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  }, [items]);
  const current = tab === BUILTIN || vendors.some(([v]) => v === tab) ? tab : BUILTIN;
  const pick = (t: string) => { setTab(t); prefs.set("fxTab", t); };

  const needle = q.trim().toLowerCase();
  const shown = needle
    ? items.filter((it) => `${it.label} ${it.vendor} ${it.category} ${it.plugin?.id ?? ""}`.toLowerCase().includes(needle))
    : items.filter((it) => it.vendor === current);
  const groups = useMemo(() => {
    const g = new Map<string, Item[]>();
    for (const it of shown) {
      const k = needle ? `${it.vendor} · ${it.category}` : it.category;
      g.set(k, [...(g.get(k) ?? []), it]);
    }
    return [...g.entries()];
  }, [shown, needle]);

  const ready = (cat?.plugins ?? []).filter((p) => p.supported).length;
  const failures = cat?.failures ?? [];

  return (
    <div className="panel fx-lib" aria-label="Effects library">
      <div className="panel-head">
        {switcher ?? <span className="label">Effects</span>}
        <span className="spacer" />
        <button className="btn icon small ghost" title="Rescan OpenFX plugins (after installing or updating plugins)" onClick={() => void loadOfxCatalog(true)} disabled={scanning} aria-label="Rescan plugins">
          <RefreshCw className={scanning ? "spin" : undefined} />
        </button>
      </div>
      <div className="pool-bar">
        <label className="search"><Search /><input className="input" placeholder="Search all effects" value={q} onChange={(e) => setQ(e.target.value)} aria-label="Search effects" /></label>
      </div>
      {!needle && (
        <div className="fx-tabs" role="tablist" aria-label="Effect vendors">
          <button role="tab" aria-selected={current === BUILTIN} onClick={() => pick(BUILTIN)}><Sparkles size={12} />{BUILTIN}</button>
          {vendors.map(([v, n]) => (
            <button key={v} role="tab" aria-selected={current === v} onClick={() => pick(v)} title={`${n} OpenFX effect${n === 1 ? "" : "s"}`}><Puzzle size={12} />{v}<span className="n">{n}</span></button>
          ))}
        </div>
      )}
      <div className="panel-body fx-body">
        {scanning && !cat && <p className="note" style={{ padding: 10 }}><RefreshCw size={12} className="spin" /> Looking for OpenFX plugins… each plugin is opened in its own process, so a faulty one cannot crash the editor.</p>}
        {groups.length === 0 && !scanning && <p className="note" style={{ padding: 10 }}>{needle ? "Nothing matches." : "No effects in this tab."}</p>}
        {groups.map(([g, list]) => {
          const isClosed = closed[`${current}/${g}`];
          return (
            <div key={g} className="fx-group">
              <button className="fx-group-head" onClick={() => setClosed((c) => ({ ...c, [`${current}/${g}`]: !isClosed }))} aria-expanded={!isClosed}>
                {isClosed ? <ChevronRight size={12} /> : <ChevronDown size={12} />}{g}<span className="n">{list.length}</span>
              </button>
              {!isClosed && list.map((it) => {
                const off = it.plugin && !it.plugin.supported;
                return (
                  <div
                    key={it.key} className={`fx-item ${off ? "off" : ""}`} role="button" tabIndex={0}
                    title={off ? `Cannot run in Pirosman: ${it.plugin!.reason}` : `${it.plugin ? `${it.plugin.id} v${it.plugin.version.join(".")}\n` : ""}Drag onto a clip, or double-click to add it to the selected clips`}
                    onPointerDown={(e) => !off && beginDrag(e, it.payload)}
                    onDoubleClick={() => !off && void applyEffect(it.payload, selection)}
                    onKeyDown={(e) => e.key === "Enter" && !off && void applyEffect(it.payload, selection)}
                  >
                    {it.plugin ? <Puzzle size={13} className="ic" /> : <Sparkles size={13} className="ic" />}
                    <span className="name">{it.label}</span>
                    {off && <AlertTriangle size={12} className="warn-text" />}
                    {!off && <button className="btn icon small ghost add" title="Add to selected clips" onPointerDown={(e) => e.stopPropagation()} onClick={() => void applyEffect(it.payload, selection)}><Plus /></button>}
                  </div>
                );
              })}
            </div>
          );
        })}
        {!needle && current === BUILTIN && cat && vendors.length <= 1 && (
          <p className="note" style={{ padding: "6px 10px 10px" }}>
            OpenFX plugins such as Boris FX Sapphire and Continuum (BCC) install to <span className="code">C:\Program Files\Common Files\OFX\Plugins</span>. Each vendor gets its own tab here after a rescan.
          </p>
        )}
      </div>
      <div className="fx-foot">
        <span className="note">{cat ? `${ready} OpenFX effect${ready === 1 ? "" : "s"} ready` : scanning ? "Scanning…" : ""}</span>
        <span className="spacer" />
        {failures.length > 0 && (
          <button className="btn small ghost" onClick={() => setShowFailures(!showFailures)} title="Plugin files that could not be described">
            <AlertTriangle size={12} className="warn-text" />{failures.length} failed
          </button>
        )}
      </div>
      {showFailures && failures.length > 0 && (
        <ul className="fx-failures">
          {failures.map((f, i) => <li key={i}><b>{f.bundle || "OpenFX host"}</b>: {f.error}</li>)}
        </ul>
      )}
    </div>
  );
}

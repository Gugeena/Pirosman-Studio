import { useEffect, useRef, useState } from "react";
import { EffectsLibrary } from "../components/EffectsLibrary";
import { Inspector, RevisionControl } from "../components/Inspector";
import { MediaPool } from "../components/MediaPool";
import { Mixer } from "../components/Mixer";
import { SourceMonitor, TimelineMonitor, type MonitorChrome } from "../components/Monitors";
import { Split } from "../components/Split";
import { Timeline } from "../components/Timeline";
import { useUI } from "../lib/store";

type Layout = { swapped: boolean; dual: boolean; floating: boolean; side: "inspector" | "mixer" | "revisions"; left: "media" | "effects" };
const DEFAULTS: Layout = { swapped: false, dual: true, floating: false, side: "inspector", left: "media" };
const readLayout = (): Layout => {
  try { return { ...DEFAULTS, ...JSON.parse(localStorage.getItem("pirosman.editLayout") ?? "{}") }; }
  catch { return DEFAULTS; }
};

/** In-app floating viewer window: drag by its header, resize from the corner, Dock to return. */
function Floating({ children, onDock }: { children: React.ReactNode; onDock: () => void }) {
  const [box, setBox] = useState({ x: 120, y: 90, w: 640, h: 420 });
  const ref = useRef(box);
  ref.current = box;
  const drag = (mode: "move" | "size") => (e: React.PointerEvent) => {
    if (mode === "move" && (e.target as HTMLElement).closest("button, input, select")) return;
    e.preventDefault();
    const s = { ...ref.current, px: e.clientX, py: e.clientY };
    const mv = (ev: PointerEvent) => {
      const dx = ev.clientX - s.px, dy = ev.clientY - s.py;
      setBox(mode === "move"
        ? { ...s, x: Math.max(0, Math.min(window.innerWidth - 120, s.x + dx)), y: Math.max(0, Math.min(window.innerHeight - 60, s.y + dy)) }
        : { ...s, w: Math.max(320, s.w + dx), h: Math.max(220, s.h + dy) });
    };
    const up = () => { window.removeEventListener("pointermove", mv); window.removeEventListener("pointerup", up); };
    window.addEventListener("pointermove", mv);
    window.addEventListener("pointerup", up);
  };
  return (
    <div className="float-win" style={{ left: box.x, top: box.y, width: box.w, height: box.h }} role="dialog" aria-label="Floating viewer">
      <div className="float-bar" onPointerDown={drag("move")}>
        <span className="label">Floating viewer</span>
        <span className="spacer" />
        <button className="btn small" onClick={onDock}>Dock</button>
      </div>
      <div className="float-body">{children}</div>
      <div className="float-grip" onPointerDown={drag("size")} aria-hidden="true" />
    </div>
  );
}

export function EditWorkspace() {
  const [l, setL] = useState<Layout>(readLayout);
  const update = (p: Partial<Layout>) => setL((o) => {
    const n = { ...o, ...p };
    try { localStorage.setItem("pirosman.editLayout", JSON.stringify(n)); } catch { /* convenience only */ }
    return n;
  });
  // Opening media in the source viewer brings a closed source viewer back.
  const sourceId = useUI((s) => s.sourceMediaId);
  const firstSource = useRef(true);
  useEffect(() => {
    if (firstSource.current) { firstSource.current = false; return; }
    if (sourceId) setL((o) => (o.dual ? o : { ...o, dual: true }));
  }, [sourceId]);

  const chrome: MonitorChrome = {
    onSwap: l.dual && !l.floating ? () => update({ swapped: !l.swapped }) : undefined,
    onToggleDual: () => update({ dual: !l.dual }),
    dual: l.dual,
    onFloat: () => update({ floating: !l.floating }),
    floating: l.floating,
  };
  const src = <SourceMonitor chrome={{ onClose: () => update({ dual: false }) }} />;
  const tl = <TimelineMonitor chrome={chrome} />;
  // Monitors that stay docked; the timeline viewer is the one that floats.
  const docked = l.floating ? (l.dual ? [src] : []) : !l.dual ? [tl] : l.swapped ? [tl, src] : [src, tl];
  const center =
    docked.length === 2 ? <Split id="edit-mon" dir="row" initial={[1, 1]} min={240}>{docked[0]}{docked[1]}</Split>
    : docked.length === 1 ? docked[0]
    : (
      <div className="panel monitor-closed">
        <p className="note">The timeline viewer is floating and the source viewer is closed.</p>
        <button className="btn small" onClick={() => update({ dual: true })}>Show source viewer</button>
      </div>
    );

  const switcher = (
    <div className="seg left-switch" role="tablist" aria-label="Left panel">
      <button aria-pressed={l.left === "media"} onClick={() => update({ left: "media" })}>Media Pool</button>
      <button aria-pressed={l.left === "effects"} onClick={() => update({ left: "effects" })}>Effects</button>
    </div>
  );

  const side = (
    <div className="panel side-tabs">
      <div className="seg side-switch" role="tablist" aria-label="Right panel">
        <button aria-pressed={l.side === "inspector"} onClick={() => update({ side: "inspector" })}>Inspector</button>
        <button aria-pressed={l.side === "mixer"} onClick={() => update({ side: "mixer" })}>Mixer</button>
        <button aria-pressed={l.side === "revisions"} onClick={() => update({ side: "revisions" })}>Revisions</button>
      </div>
      {l.side === "inspector" ? <Inspector /> : l.side === "mixer" ? <Mixer /> : (
        <div className="panel" aria-label="Revision control"><div className="panel-body"><div className="insp"><RevisionControl /></div></div></div>
      )}
    </div>
  );

  return (
    <div className="page edit-page">
      <Split id="edit-v" dir="col" initial={[60, 40]} min={180}>
        <Split id="edit-h" dir="row" initial={[18, 62, 20]} min={200}>
          {l.left === "effects" ? <EffectsLibrary switcher={switcher} /> : <MediaPool switcher={switcher} />}
          {center}
          {side}
        </Split>
        <Timeline />
      </Split>
      {l.floating && <Floating onDock={() => update({ floating: false })}>{tl}</Floating>}
    </div>
  );
}

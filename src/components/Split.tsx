import { Children, useRef, useState, type ReactNode } from "react";

const load = (key: string, n: number): number[] | null => {
  try {
    const v = JSON.parse(localStorage.getItem(`pirosman.split.${key}`) ?? "null");
    return Array.isArray(v) && v.length === n && v.every((x) => typeof x === "number") ? v : null;
  } catch { return null; }
};
const store = (key: string, v: number[]) => { try { localStorage.setItem(`pirosman.split.${key}`, JSON.stringify(v)); } catch { /* per-viewer convenience only */ } };

/**
 * Resizable row/column of panes separated by draggable gutters. Sizes are
 * fractions (flex-grow), remembered per `id`; double-click a gutter to reset.
 */
export function Split({ id, dir, initial, min = 120, children }: { id: string; dir: "row" | "col"; initial: number[]; min?: number; children: ReactNode }) {
  const kids = Children.toArray(children).filter(Boolean);
  const n = kids.length;
  const base = initial.length === n ? initial : Array(n).fill(1);
  const [sizes, setSizes] = useState<number[]>(() => load(id, n) ?? base);
  const ref = useRef<HTMLDivElement>(null);
  const cur = sizes.length === n ? sizes : base;

  const drag = (i: number) => (e: React.PointerEvent) => {
    e.preventDefault();
    const el = ref.current!;
    const panes = Array.from(el.children).filter((c) => (c as HTMLElement).dataset.pane) as HTMLElement[];
    const px = panes.map((p) => (dir === "row" ? p.offsetWidth : p.offsetHeight));
    const total = cur.reduce((a, b) => a + b, 0);
    const pxTotal = px.reduce((a, b) => a + b, 0);
    const start = dir === "row" ? e.clientX : e.clientY;
    const a = px[i], b = px[i + 1];
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    let next = cur;
    const mv = (ev: PointerEvent) => {
      const d = (dir === "row" ? ev.clientX : ev.clientY) - start;
      const na = Math.max(min, Math.min(a + b - min, a + d));
      const out = px.map((p) => (p / pxTotal) * total);
      out[i] = (na / pxTotal) * total;
      out[i + 1] = ((a + b - na) / pxTotal) * total;
      next = out;
      setSizes(out);
    };
    const up = () => { window.removeEventListener("pointermove", mv); window.removeEventListener("pointerup", up); store(id, next); };
    window.addEventListener("pointermove", mv);
    window.addEventListener("pointerup", up);
  };

  return (
    <div ref={ref} className={`split split-${dir}`}>
      {kids.map((k, i) => [
        <div key={`p${i}`} data-pane="1" className="split-pane" style={{ flex: `${cur[i]} 1 0` }}>{k}</div>,
        i < n - 1 && <div key={`g${i}`} className="gutter" role="separator" aria-orientation={dir === "row" ? "vertical" : "horizontal"} title="Drag to resize · double-click to reset" onPointerDown={drag(i)} onDoubleClick={() => { setSizes(base); store(id, base); }} />,
      ])}
    </div>
  );
}

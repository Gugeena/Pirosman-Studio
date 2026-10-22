// One styled tooltip for the whole app instead of the WebView's native ones.
// Any element with a `title` gets it: the title moves to `data-tip` on first hover (so
// the native tooltip never shows), and a trailing "(Shortcut)" renders as a key cap.
import { useEffect, useState } from "react";

interface Tip { text: string; key: string | null; x: number; y: number; below: boolean }

const SHORTCUT = /\s*\(((?:Ctrl|Shift|Alt|F\d+|Space|Del|Delete|Home|End|Enter|Esc|[A-Z0-9?=+\-/]|[IJKLMO](?: \/ [IJKLMO])?)(?:\+[^)]+)?)\)\s*$/;

function split(t: string): { text: string; key: string | null } {
  const m = t.match(SHORTCUT);
  return m ? { text: t.slice(0, m.index).trim(), key: m[1] } : { text: t, key: null };
}

export function TooltipLayer() {
  const [tip, setTip] = useState<Tip | null>(null);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let current: HTMLElement | null = null;
    const hide = () => { clearTimeout(timer); current = null; setTip(null); };
    const over = (e: MouseEvent) => {
      const el = (e.target as HTMLElement | null)?.closest?.("[title],[data-tip]") as HTMLElement | null;
      if (el === current) return;
      hide();
      if (!el) return;
      const t = el.getAttribute("title");
      if (t) { el.setAttribute("data-tip", t); el.removeAttribute("title"); }
      const text = el.getAttribute("data-tip");
      if (!text) return;
      current = el;
      timer = setTimeout(() => {
        if (current !== el || !el.isConnected) return;
        const r = el.getBoundingClientRect();
        const below = r.bottom + 40 < window.innerHeight;
        setTip({ ...split(text), x: Math.min(Math.max(8, r.left + r.width / 2), window.innerWidth - 8), y: below ? r.bottom + 6 : r.top - 6, below });
      }, 450);
    };
    const out = (e: MouseEvent) => { if (current && !current.contains(e.relatedTarget as Node | null)) hide(); };
    window.addEventListener("mouseover", over);
    window.addEventListener("mouseout", out);
    window.addEventListener("pointerdown", hide, true);
    window.addEventListener("keydown", hide, true);
    window.addEventListener("wheel", hide, { capture: true, passive: true });
    return () => {
      hide();
      window.removeEventListener("mouseover", over);
      window.removeEventListener("mouseout", out);
      window.removeEventListener("pointerdown", hide, true);
      window.removeEventListener("keydown", hide, true);
      window.removeEventListener("wheel", hide, true);
    };
  }, []);
  if (!tip) return null;
  return (
    <div className="tip" role="tooltip" style={{ left: tip.x, top: tip.y, transform: `translate(-50%, ${tip.below ? "0" : "-100%"})` }}>
      {tip.text}{tip.key && <kbd>{tip.key}</kbd>}
    </div>
  );
}

// Pointer-driven drag and drop inside the app. The window keeps native file drops
// (import from Explorer), which on Windows disables HTML5 drag events in the WebView,
// so pool → timeline and library → clip drags follow the pointer instead.
//
// A drag starts after the pointer moves 5 px with the button held. While it runs,
// `useUI().drag` holds the payload and pointer position (drop targets highlight from
// it); on release a `pirosman-drop` window event carries the payload and position.
import { useUI, type DragPayload } from "./store";

export interface DropDetail { payload: DragPayload; x: number; y: number }

export function beginDrag(e: React.PointerEvent, payload: DragPayload) {
  if (e.button !== 0) return;
  const x0 = e.clientX, y0 = e.clientY;
  let live = false;
  const set = useUI.getState().set;
  const mv = (ev: PointerEvent) => {
    if (!live && Math.hypot(ev.clientX - x0, ev.clientY - y0) < 5) return;
    live = true;
    set({ drag: { payload, x: ev.clientX, y: ev.clientY } });
  };
  const stop = () => {
    window.removeEventListener("pointermove", mv);
    window.removeEventListener("pointerup", up);
    window.removeEventListener("pointercancel", cancel);
    window.removeEventListener("keydown", esc, true);
    set({ drag: null });
  };
  const up = (ev: PointerEvent) => {
    stop();
    if (!live) return;
    // The click that ends a drag must not also select or open what was dragged.
    const swallow = (c: MouseEvent) => { c.stopPropagation(); c.preventDefault(); };
    window.addEventListener("click", swallow, { capture: true, once: true });
    setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
    window.dispatchEvent(new CustomEvent<DropDetail>("pirosman-drop", { detail: { payload, x: ev.clientX, y: ev.clientY } }));
  };
  const cancel = () => stop();
  const esc = (k: KeyboardEvent) => { if (k.key === "Escape") { k.stopPropagation(); stop(); live = false; } };
  window.addEventListener("pointermove", mv);
  window.addEventListener("pointerup", up);
  window.addEventListener("pointercancel", cancel);
  window.addEventListener("keydown", esc, true);
}

/** The element under the pointer, ignoring the drag ghost. */
export function hitAt(x: number, y: number): HTMLElement | null {
  return document.elementFromPoint(x, y) as HTMLElement | null;
}

/** The chip that follows the pointer while dragging. */
export function DragGhost() {
  const drag = useUI((s) => s.drag);
  if (!drag) return null;
  const p = drag.payload;
  const kind = p.kind === "media" ? "Media" : p.kind === "ofx" ? "OpenFX" : "Effect";
  return (
    <div className="drag-ghost" style={{ left: drag.x + 14, top: drag.y + 10 }} aria-hidden="true">
      <span className="k">{kind}</span>{p.label}
    </div>
  );
}

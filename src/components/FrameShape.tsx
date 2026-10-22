/** The frame's proportions drawn to scale inside a fixed box: how a preset or project is shaped. */
export function FrameShape({ width, height, box = 40, dashed = false }: { width: number; height: number; box?: number; dashed?: boolean }) {
  const r = width && height ? width / height : 16 / 9;
  const w = r >= 1 ? box : Math.round(box * r);
  const h = r >= 1 ? Math.round(box / r) : box;
  return (
    <span className="frame-shape" style={{ width: box, height: box }} aria-hidden="true">
      <span className={dashed ? "dashed" : ""} style={{ width: w, height: h }} />
    </span>
  );
}

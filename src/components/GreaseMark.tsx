// A hand-drawn china-marker loop around a clip that changed since the branch
// head. Deterministic per clip id so it doesn't wobble between renders; it
// draws itself on once when it first appears.

function seeded(seed: string) {
  let h = 2166136261;
  for (let i = 0; i < seed.length; i++) h = Math.imul(h ^ seed.charCodeAt(i), 16777619);
  return () => {
    h = Math.imul(h ^ (h >>> 15), 2246822507);
    h = Math.imul(h ^ (h >>> 13), 3266489909);
    return ((h ^= h >>> 16) >>> 0) / 4294967296;
  };
}

export function greasePath(w: number, h: number, seed: string): { d: string; tilt: number } {
  const rnd = seeded(seed);
  const cx = w / 2, cy = h / 2;
  const rx = w / 2 + 5, ry = h / 2 + 4;
  const n = 2.8; // superellipse exponent: an oval that hugs the clip, not a pill
  const steps = Math.max(24, Math.min(72, Math.round((w + h) / 10)));
  const t0 = Math.PI * (0.95 + rnd() * 0.3);
  const sweep = Math.PI * 2 + 0.5 + rnd() * 0.3; // the tail crosses the start
  // Low-frequency wobble: two slow sine waves with seeded phase.
  const p1 = rnd() * Math.PI * 2, p2 = rnd() * Math.PI * 2;
  const wob = (t: number) => 1.6 * Math.sin(t * 2 + p1) + 1.0 * Math.sin(t * 3 + p2);
  const pts: [number, number][] = [];
  for (let i = 0; i <= steps; i++) {
    const t = t0 + (sweep * i) / steps;
    const c = Math.cos(t), s = Math.sin(t);
    const k = i / steps;
    const drift = 1 + k * 0.08; // the loop opens outward as the hand comes round
    const r = wob(t);
    pts.push([
      cx + Math.sign(c) * Math.pow(Math.abs(c), 2 / n) * (rx * drift + r),
      cy + Math.sign(s) * Math.pow(Math.abs(s), 2 / n) * (ry * drift + r * 0.6) + k * 4,
    ]);
  }
  let d = `M${pts[0][0].toFixed(1)},${pts[0][1].toFixed(1)}`;
  for (let i = 0; i < pts.length - 1; i++) {
    const p0 = pts[Math.max(0, i - 1)], p1_ = pts[i], p2_ = pts[i + 1], p3 = pts[Math.min(pts.length - 1, i + 2)];
    const c1 = [p1_[0] + (p2_[0] - p0[0]) / 6, p1_[1] + (p2_[1] - p0[1]) / 6];
    const c2 = [p2_[0] - (p3[0] - p1_[0]) / 6, p2_[1] - (p3[1] - p1_[1]) / 6];
    d += ` C${c1[0].toFixed(1)},${c1[1].toFixed(1)} ${c2[0].toFixed(1)},${c2[1].toFixed(1)} ${p2_[0].toFixed(1)},${p2_[1].toFixed(1)}`;
  }
  const tilt = (rnd() < 0.5 ? -1 : 1) * (0.6 + rnd() * 0.6);
  return { d, tilt };
}

export function GreaseMark({ x, y, w, h, seed }: { x: number; y: number; w: number; h: number; seed: string }) {
  const pad = 14;
  const { d, tilt } = greasePath(w, h, seed);
  return (
    <svg className="gp-mark" style={{ left: x - pad, top: y - pad }} width={w + pad * 2} height={h + pad * 2} aria-hidden="true">
      <g transform={`translate(${pad},${pad}) rotate(${tilt} ${w / 2} ${h / 2})`}>
        <path className="gp-ghost" d={d} transform="translate(1 1)" pathLength={1} />
        <path className="gp-stroke" d={d} pathLength={1} />
      </g>
    </svg>
  );
}

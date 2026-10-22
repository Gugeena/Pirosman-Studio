import { usePeersOn } from "../lib/collab";
import { useUI } from "../lib/store";

/** Live Sync: other editors' pointers on the page you are looking at. */
export function RemoteCursors() {
  const page = useUI((s) => s.page);
  const peers = usePeersOn(page).filter((p) => p.x != null && p.y != null && Date.now() - (p.at ?? 0) < 15000);
  if (!peers.length) return null;
  return (
    <div className="remote-cursors" aria-hidden="true">
      {peers.map((p) => (
        <div key={p.id} className="rcursor" style={{ left: `${p.x! * 100}%`, top: `${p.y! * 100}%`, ["--peer" as string]: p.color }}>
          <svg width="18" height="20" viewBox="0 0 18 20"><path d="M1 1 L1 16 L5.2 12.2 L8.2 19 L11 17.8 L8 11.2 L14 11 Z" fill={p.color} stroke="#03060c" strokeWidth="1.4" strokeLinejoin="round" /></svg>
          <span>{p.name}</span>
        </div>
      ))}
    </div>
  );
}

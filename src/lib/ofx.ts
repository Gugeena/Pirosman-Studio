// OpenFX catalog loading and applying library effects to clips.
import { api, type OfxEffect, type OfxPlugin, type OfxValue, type TransitionKind, type VfxKind } from "./api";
import { clipDur, prevClip } from "./clip";
import { errText, findClip, run, useUI, type DragPayload } from "./store";

let inflight: Promise<void> | null = null;

/** Load the installed plugins (cached by the backend unless `rescan`). */
export function loadOfxCatalog(rescan = false): Promise<void> {
  if (inflight && !rescan) return inflight;
  useUI.setState({ ofxScanning: true });
  inflight = api.ofxCatalog(rescan)
    .then((c) => {
      useUI.setState({ ofx: c });
      if (rescan) useUI.getState().toast("ok", `OpenFX: ${c.plugins.filter((p) => p.supported).length} effects ready${c.failures.length ? `, ${c.failures.length} plugin file${c.failures.length === 1 ? "" : "s"} failed` : ""}`);
    })
    .catch((e) => useUI.getState().toast("error", `OpenFX scan failed: ${errText(e)}`))
    .finally(() => { useUI.setState({ ofxScanning: false }); inflight = null; });
  return inflight;
}

export function newOfxEffect(p: OfxPlugin): OfxEffect {
  return { id: crypto.randomUUID(), plugin_id: p.id, label: p.label, grouping: p.grouping, binary: p.binary, index: p.index, params: {}, enabled: true };
}

/** The installed plugin an effect on a clip refers to. */
export function pluginFor(fx: OfxEffect): OfxPlugin | undefined {
  const ps = useUI.getState().ofx?.plugins ?? [];
  return ps.find((p) => p.id === fx.plugin_id && p.binary === fx.binary) ?? ps.find((p) => p.id === fx.plugin_id);
}

export function paramValue(fx: OfxEffect, name: string, fallback: OfxValue | null): OfxValue | null {
  return name in fx.params ? fx.params[name] : fallback;
}

/** Add a library effect (OpenFX or built-in) to the end of each video clip's stack. */
export async function applyEffect(payload: DragPayload, clipIds: string[]) {
  if (payload.kind === "media") return;
  if (payload.kind === "transition") return applyTransition(payload.transition, payload.label, clipIds);
  const s = useUI.getState();
  if (payload.kind === "ofx" && !payload.plugin.supported) { s.toast("error", `${payload.label} cannot run here: ${payload.plugin.reason ?? "unsupported"}`); return; }
  const targets = clipIds.map((id) => findClip(s.view, id)).filter((x): x is NonNullable<typeof x> => !!x && x.track.kind === "video");
  if (!targets.length) { s.toast("info", "Select a video clip on the timeline, or drop the effect onto one."); return; }
  for (const { clip } of targets) {
    const patch = payload.kind === "ofx"
      ? { ofx: [...(clip.ofx ?? []), newOfxEffect(payload.plugin)] }
      : { vfx: [...(clip.vfx ?? []), { id: crypto.randomUUID(), kind: payload.vfx as VfxKind, amount: 0.35, enabled: true }] };
    if (!(await run(api.updateClip(clip.id, patch)))) return;
  }
  s.set({ selection: targets.map((t) => t.clip.id) });
  s.toast("ok", `${payload.label} added to ${targets.length === 1 ? targets[0].clip.name : `${targets.length} clips`}`);
}

/** Put a transition on the cut where each clip starts (1 s, or shorter when either clip is). */
async function applyTransition(kind: TransitionKind, label: string, clipIds: string[]) {
  const s = useUI.getState();
  const rate = s.view?.project.settings.frame_rate;
  const fps = rate ? Math.round(rate.num / rate.den) : 25;
  const targets = clipIds.map((id) => findClip(s.view, id)).flatMap((x) => {
    const prev = x && prevClip(x.track, x.clip);
    return x && prev ? [{ clip: x.clip, prev }] : [];
  });
  if (!targets.length) { s.toast("info", "Drop the transition onto a cut, or select a clip that starts where another ends."); return; }
  for (const { clip, prev } of targets) {
    const d = clip.transition_in?.duration_frames ?? Math.max(2, Math.min(fps, clipDur(clip), clipDur(prev)));
    if (!(await run(api.updateClip(clip.id, { transition_in: { kind, duration_frames: d } })))) return;
  }
  s.toast("ok", `${label} added at ${targets.length === 1 ? `the start of ${targets[0].clip.name}` : `${targets.length} cuts`}`);
}

// Live preview audio: one <audio> element per audio track routed through
// Web Audio (clip gain × fader × fade → low shelf → peak → high shelf →
// balance → meter). Approximates the export mixer; the export itself is
// rendered by FFmpeg (see crates/pirosman-core/src/render.rs).

interface Strip {
  el: HTMLAudioElement;
  gain: GainNode;
  low: BiquadFilterNode;
  mid: BiquadFilterNode;
  high: BiquadFilterNode;
  pan: StereoPannerNode;
  meter: AnalyserNode;
  buf: Float32Array;
  peak: number;
}

let ctx: AudioContext | null = null;
const strips = new Map<string, Strip>();

function context(): AudioContext {
  if (!ctx) ctx = new AudioContext({ latencyHint: "interactive" });
  return ctx;
}

/** Call from a user gesture (Play) so the browser allows audio. */
export function resumeAudio() {
  void context().resume();
}

export function strip(trackId: string): Strip {
  let s = strips.get(trackId);
  if (s) return s;
  const c = context();
  const el = new Audio();
  el.crossOrigin = "anonymous"; // the asset protocol answers with CORS, so Web Audio can read samples
  el.preload = "auto";
  const src = c.createMediaElementSource(el);
  const gain = c.createGain();
  const low = c.createBiquadFilter();
  low.type = "lowshelf";
  low.frequency.value = 100;
  const mid = c.createBiquadFilter();
  mid.type = "peaking";
  mid.frequency.value = 1000;
  mid.Q.value = 0.9;
  const high = c.createBiquadFilter();
  high.type = "highshelf";
  high.frequency.value = 8000;
  const pan = c.createStereoPanner();
  const meter = c.createAnalyser();
  meter.fftSize = 1024;
  src.connect(gain).connect(low).connect(mid).connect(high).connect(pan).connect(meter).connect(c.destination);
  s = { el, gain, low, mid, high, pan, meter, buf: new Float32Array(meter.fftSize), peak: 0 };
  strips.set(trackId, s);
  return s;
}

export function setStrip(trackId: string, p: { gainLinear: number; pan: number; low: number; mid: number; high: number }) {
  const s = strip(trackId);
  const t = context().currentTime;
  s.gain.gain.setTargetAtTime(p.gainLinear, t, 0.015);
  s.pan.pan.setTargetAtTime(Math.max(-1, Math.min(1, p.pan)), t, 0.015);
  s.low.gain.value = p.low;
  s.mid.gain.value = p.mid;
  s.high.gain.value = p.high;
}

/** Peak level in dBFS with a short release, for meters. */
export function levelDb(trackId: string): number {
  const s = strips.get(trackId);
  if (!s || s.el.paused) {
    if (s) s.peak *= 0.85;
    return s && s.peak > 1e-5 ? 20 * Math.log10(s.peak) : -90;
  }
  s.meter.getFloatTimeDomainData(s.buf as Float32Array<ArrayBuffer>);
  let pk = 0;
  for (let i = 0; i < s.buf.length; i++) pk = Math.max(pk, Math.abs(s.buf[i]));
  s.peak = Math.max(pk, s.peak * 0.88);
  return s.peak > 1e-5 ? 20 * Math.log10(s.peak) : -90;
}

export function dropStripsExcept(keep: Set<string>) {
  for (const [id, s] of strips) {
    if (!keep.has(id)) {
      s.el.pause();
      s.el.removeAttribute("src");
      strips.delete(id);
    }
  }
}

export const dbToLinear = (db: number) => Math.pow(10, db / 20);

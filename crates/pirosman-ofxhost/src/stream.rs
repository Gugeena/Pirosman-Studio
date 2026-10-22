//! Frame streaming for `pirosman-ofxhost render`.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::Arc;

use crate::host::{Frame, Instance, Library};
use crate::protocol::{RenderSpec, PROTOCOL};

/// Frames kept either side of the current one for plugins that read other times.
const TEMPORAL_WINDOW: u32 = 4;

fn decode(raw: &[u8]) -> Vec<f32> {
    raw.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]]) as f32 / 65535.0).collect()
}

fn encode(f: &[f32], out: &mut Vec<u8>) {
    out.clear();
    for v in f {
        out.extend_from_slice(&((v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).to_le_bytes());
    }
}

fn nearest(w: &[(u32, Frame)], t: f64) -> Frame {
    w.iter()
        .min_by(|a, b| (a.0 as f64 - t).abs().total_cmp(&(b.0 as f64 - t).abs()))
        .map(|x| x.1.clone())
        .expect("window is never empty while rendering")
}

/// Read `spec.frames` frames (`rgba64le`, top row first) from `input`, run them
/// through every effect in order, and write the results to `output`.
/// `on_frame` is called after each frame is written.
pub fn render_stream(spec: &RenderSpec, input: &mut dyn Read, output: &mut dyn Write, mut on_frame: impl FnMut(u32)) -> Result<(), String> {
    if spec.protocol != PROTOCOL {
        return Err(format!("render spec protocol {} does not match this host ({PROTOCOL})", spec.protocol));
    }
    let (w, h) = (spec.width as usize, spec.height as usize);
    if w == 0 || h == 0 || w > 16384 || h > 16384 || spec.frames == 0 {
        return Err(format!("bad frame size {w}x{h} or frame count {}", spec.frames));
    }
    let mut lib = Library::default();
    let mut insts: Vec<Instance> = vec![];
    for e in &spec.effects {
        let p = lib.open(&e.binary, e.index, &e.id)?;
        let ctx = p.pick_context().ok_or_else(|| format!("{} offers no filter or general context", p.label()))?;
        insts.push(Instance::create(p, &ctx, &e.params, w, h, spec.frame_rate, spec.frames)?);
    }
    for i in &insts {
        i.begin(spec.frames)?;
    }
    let reach = if insts.iter().any(|i| i.temporal) { TEMPORAL_WINDOW } else { 0 };
    let mut src: VecDeque<(u32, Frame)> = VecDeque::new();
    let mut hist: Vec<VecDeque<(u32, Frame)>> = vec![VecDeque::new(); insts.len()];
    let mut raw = vec![0u8; w * h * 8];
    let mut bytes = Vec::with_capacity(w * h * 8);
    let mut next = 0u32;
    let result = (|| -> Result<(), String> {
        for t in 0..spec.frames {
            while next < spec.frames && next <= t + reach {
                input.read_exact(&mut raw).map_err(|e| format!("input ended at frame {next} of {}: {e}", spec.frames))?;
                src.push_back((next, Arc::new(decode(&raw))));
                next += 1;
            }
            while src.len() > 1 && src.front().is_some_and(|f| f.0 + reach < t) {
                src.pop_front();
            }
            let mut cur = nearest(src.make_contiguous(), t as f64);
            for k in 0..insts.len() {
                let window: Vec<(u32, Frame)> = if k == 0 { src.iter().cloned().collect() } else { hist[k - 1].iter().cloned().collect() };
                let source = move |time: f64| nearest(&window, time);
                let out = Arc::new(insts[k].render(t as f64, &source)?);
                hist[k].push_back((t, out.clone()));
                while hist[k].len() as u32 > reach + 1 {
                    hist[k].pop_front();
                }
                cur = out;
            }
            encode(&cur, &mut bytes);
            output.write_all(&bytes).map_err(|e| format!("output closed at frame {t}: {e}"))?;
            on_frame(t);
        }
        output.flush().map_err(|e| e.to_string())
    })();
    for i in &insts {
        i.end(spec.frames);
    }
    result
}

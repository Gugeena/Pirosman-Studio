//! Video scopes (luma waveform + RGB parade) computed from a graded frame.
//!
//! The frame comes from FFmpeg with the clip's export grade applied (the same
//! `colorlevels`/`eq`/`lut3d` chain the render uses), so the scopes show what
//! the export will contain, not the on-screen approximation.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;

use crate::error::{CoreError, CoreResult};
use crate::media::{command, Tools};
use crate::model::Clip;
use crate::render::{color_filters, filter_path};

pub const SCOPE_W: usize = 256;
pub const SCOPE_H: usize = 144;
/// Columns and value levels in each scope grid.
pub const COLS: usize = 128;
pub const LEVELS: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scopes {
    pub cols: usize,
    pub levels: usize,
    /// `cols × levels` hit counts, column-major, level 0 = black.
    pub luma: Vec<u16>,
    pub red: Vec<u16>,
    pub green: Vec<u16>,
    pub blue: Vec<u16>,
    /// Fraction of pixels clipped to 0 or 255 in any channel.
    pub clipped_low: f64,
    pub clipped_high: f64,
}

/// Bin an RGB24 frame into waveform grids.
pub fn compute_scopes(width: usize, height: usize, rgb: &[u8]) -> CoreResult<Scopes> {
    if width == 0 || height == 0 || rgb.len() < width * height * 3 {
        return Err(CoreError::Invalid("frame buffer is smaller than its dimensions".into()));
    }
    let mut luma = vec![0u16; COLS * LEVELS];
    let mut red = vec![0u16; COLS * LEVELS];
    let mut green = vec![0u16; COLS * LEVELS];
    let mut blue = vec![0u16; COLS * LEVELS];
    let (mut lo, mut hi) = (0usize, 0usize);
    let lv = |v: u8| (v as usize * LEVELS) / 256;
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 3;
            let (r, g, b) = (rgb[i], rgb[i + 1], rgb[i + 2]);
            let col = x * COLS / width;
            // Rec.709 luma.
            let yv = (0.2126 * r as f64 + 0.7152 * g as f64 + 0.0722 * b as f64).round().clamp(0.0, 255.0) as u8;
            for (grid, v) in [(&mut luma, yv), (&mut red, r), (&mut green, g), (&mut blue, b)] {
                let cell = &mut grid[col * LEVELS + lv(v)];
                *cell = cell.saturating_add(1);
            }
            if r == 0 || g == 0 || b == 0 {
                lo += 1;
            }
            if r == 255 || g == 255 || b == 255 {
                hi += 1;
            }
        }
    }
    let n = (width * height) as f64;
    Ok(Scopes { cols: COLS, levels: LEVELS, luma, red, green, blue, clipped_low: lo as f64 / n, clipped_high: hi as f64 / n })
}

/// Grab one graded frame of `media_path` at `seconds` as RGB24 and analyse it.
pub fn scopes_for(tools: &Tools, media_path: &Path, seconds: f64, clip: Option<&Clip>) -> CoreResult<Scopes> {
    let mut vf = vec![format!("scale={SCOPE_W}:{SCOPE_H}:force_original_aspect_ratio=decrease,pad={SCOPE_W}:{SCOPE_H}:(ow-iw)/2:(oh-ih)/2")];
    if let Some(c) = clip {
        if let Some(cf) = color_filters(&c.color) {
            vf.push(cf);
        }
        if let Some(l) = &c.lut {
            if Path::new(l).is_file() {
                vf.push(format!("lut3d=file={}:interp=tetrahedral", filter_path(l)));
            }
        }
    }
    vf.push("format=rgb24".into());
    let out = command(&tools.ffmpeg)
        .args(["-v", "error", "-ss", &format!("{:.4}", seconds.max(0.0)), "-i"])
        .arg(media_path)
        .args(["-frames:v", "1", "-vf", &vf.join(","), "-f", "rawvideo", "pipe:1"])
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| CoreError::Media(format!("could not run ffmpeg: {e}")))?;
    if !out.status.success() || out.stdout.len() < SCOPE_W * SCOPE_H * 3 {
        return Err(CoreError::Media(format!("could not read a frame for scopes: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    compute_scopes(SCOPE_W, SCOPE_H, &out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bins_a_known_frame() {
        // Left half pure red, right half white.
        let (w, h) = (4usize, 2usize);
        let mut buf = vec![];
        for _ in 0..h {
            for x in 0..w {
                buf.extend(if x < 2 { [255, 0, 0] } else { [255, 255, 255] });
            }
        }
        let s = compute_scopes(w, h, &buf).unwrap();
        let at = |g: &Vec<u16>, col: usize, level: usize| g[col * LEVELS + level];
        // Column 0 (red): R at top level, G/B at 0, luma ≈ 0.2126 → level 13.
        assert_eq!(at(&s.red, 0, LEVELS - 1), 2);
        assert_eq!(at(&s.green, 0, 0), 2);
        assert_eq!(at(&s.luma, 0, (54 * LEVELS) / 256), 2);
        // x = 3 (white) lands in column 3·COLS/4: every channel at the top.
        assert_eq!(at(&s.luma, 3 * COLS / 4, LEVELS - 1), 2);
        assert_eq!((s.clipped_low, s.clipped_high), (0.5, 1.0));
        assert!(compute_scopes(4, 4, &buf).is_err(), "short buffer rejected");
    }
}

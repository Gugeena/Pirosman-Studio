//! Frame-accurate timing.
//!
//! All timeline positions and source in/out points are integer frames at the
//! project's rational frame rate. Conversions to seconds, audio samples, and
//! FFmpeg timestamps go through integer math with one explicit rounding rule:
//! **round half away from zero** at the final step only. Nothing accumulates
//! floating-point error across edits.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::error::{CoreError, CoreResult};

/// A rational number of frames per second, e.g. 24000/1001.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rational {
    pub num: u32,
    pub den: u32,
}

impl Rational {
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }

    pub fn validate(&self) -> CoreResult<()> {
        if self.num == 0 || self.den == 0 {
            return Err(CoreError::Invalid(format!("frame rate {}/{} must be positive", self.num, self.den)));
        }
        let fps = self.num as f64 / self.den as f64;
        if !(1.0..=240.0).contains(&fps) {
            return Err(CoreError::Invalid(format!("frame rate {fps:.3} fps is outside 1–240 fps")));
        }
        Ok(())
    }

    /// Reduce by the greatest common divisor so equal rates compare equal.
    pub fn reduced(&self) -> Self {
        let g = gcd(self.num as u64, self.den as u64) as u32;
        if g == 0 {
            return *self;
        }
        Self { num: self.num / g, den: self.den / g }
    }

    /// Parse FFprobe-style "30000/1001" or "25". Returns None for "0/0".
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let r = if let Some((a, b)) = s.split_once('/') {
            Self { num: a.trim().parse().ok()?, den: b.trim().parse().ok()? }
        } else if let Ok(n) = s.parse::<u32>() {
            Self { num: n, den: 1 }
        } else {
            return None;
        };
        if r.num == 0 || r.den == 0 {
            None
        } else {
            Some(r.reduced())
        }
    }

    pub fn as_f64(&self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// FFmpeg rational syntax, e.g. "24000/1001".
    pub fn ffmpeg(&self) -> String {
        format!("{}/{}", self.num, self.den)
    }

    /// Nominal integer frames-per-second used for timecode display
    /// (non-drop-frame), e.g. 30000/1001 → 30.
    pub fn nominal_fps(&self) -> u64 {
        div_round(self.num as i128, self.den as i128).max(1) as u64
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{:.3}", self.as_f64())
        }
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Integer division rounding half away from zero.
pub fn div_round(n: i128, d: i128) -> i128 {
    assert!(d != 0, "division by zero");
    let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
    if n >= 0 {
        (n + d / 2) / d
    } else {
        -((-n + d / 2) / d)
    }
}

/// Frames → microseconds (rounded).
pub fn frames_to_micros(frames: i64, rate: Rational) -> i64 {
    div_round(frames as i128 * rate.den as i128 * 1_000_000, rate.num as i128) as i64
}

/// Microseconds → nearest frame.
pub fn micros_to_frames(us: i64, rate: Rational) -> i64 {
    div_round(us as i128 * rate.num as i128, rate.den as i128 * 1_000_000) as i64
}

/// Seconds (as reported by FFprobe) → nearest frame. Only used at import
/// time to convert probe metadata; never during editing.
pub fn seconds_to_frames(seconds: f64, rate: Rational) -> i64 {
    micros_to_frames((seconds * 1_000_000.0).round() as i64, rate)
}

/// Frames → audio samples at `sample_rate` (rounded).
pub fn frames_to_samples(frames: i64, rate: Rational, sample_rate: u32) -> i64 {
    div_round(frames as i128 * rate.den as i128 * sample_rate as i128, rate.num as i128) as i64
}

/// Exact FFmpeg time string with microsecond precision, e.g. "4.004000".
pub fn ffmpeg_time(frames: i64, rate: Rational) -> String {
    let us = frames_to_micros(frames, rate);
    let sign = if us < 0 { "-" } else { "" };
    let us = us.abs();
    format!("{sign}{}.{:06}", us / 1_000_000, us % 1_000_000)
}

/// Non-drop-frame timecode HH:MM:SS:FF using the nominal integer rate.
pub fn timecode(frames: i64, rate: Rational) -> String {
    let fps = rate.nominal_fps() as i64;
    let neg = frames < 0;
    let f = frames.abs();
    let ff = f % fps;
    let total_s = f / fps;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    format!("{}{:02}:{:02}:{:02}:{:02}", if neg { "-" } else { "" }, h, m, s, ff)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NTSC: Rational = Rational::new(24000, 1001);

    #[test]
    fn parse_rates() {
        assert_eq!(Rational::parse("30000/1001"), Some(Rational::new(30000, 1001)));
        assert_eq!(Rational::parse("50/2"), Some(Rational::new(25, 1)));
        assert_eq!(Rational::parse("25"), Some(Rational::new(25, 1)));
        assert_eq!(Rational::parse("0/0"), None);
        assert_eq!(Rational::parse("abc"), None);
    }

    #[test]
    fn frames_seconds_roundtrip_has_no_drift() {
        // 1 hour of 23.976 frames: converting each frame index independently
        // must always round-trip exactly (no accumulation).
        for f in (0..86_400).step_by(37) {
            let us = frames_to_micros(f, NTSC);
            assert_eq!(micros_to_frames(us, NTSC), f, "frame {f}");
        }
        // 24000/1001: 24 frames = 1.001 s exactly.
        assert_eq!(frames_to_micros(24, NTSC), 1_001_000);
        assert_eq!(ffmpeg_time(24, NTSC), "1.001000");
    }

    #[test]
    fn samples_are_exact_for_integer_rates() {
        assert_eq!(frames_to_samples(25, Rational::new(25, 1), 48_000), 48_000);
        // 1001 frames @ 24000/1001 = 1001*1001/24000 s = 41.75041666 s
        assert_eq!(frames_to_samples(1001, NTSC, 48_000), 2_004_002);
    }

    #[test]
    fn rounding_is_half_away_from_zero() {
        assert_eq!(div_round(5, 2), 3);
        assert_eq!(div_round(-5, 2), -3);
        assert_eq!(div_round(4, 3), 1);
        assert_eq!(seconds_to_frames(1.0, Rational::new(25, 1)), 25);
        assert_eq!(seconds_to_frames(0.019, Rational::new(25, 1)), 0);
        assert_eq!(seconds_to_frames(0.020, Rational::new(25, 1)), 1);
    }

    #[test]
    fn timecode_formatting() {
        assert_eq!(timecode(0, Rational::new(25, 1)), "00:00:00:00");
        assert_eq!(timecode(25 * 61 + 3, Rational::new(25, 1)), "00:01:01:03");
        assert_eq!(timecode(30, Rational::new(30000, 1001)), "00:00:01:00");
    }

    #[test]
    fn rate_validation() {
        assert!(Rational::new(0, 1).validate().is_err());
        assert!(Rational::new(1000, 1).validate().is_err());
        assert!(NTSC.validate().is_ok());
    }
}

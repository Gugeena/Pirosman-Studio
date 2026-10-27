//! Timeline → FFmpeg render planning and execution.
//!
//! Composition model:
//! - A black canvas at the output size and project rate spans the timeline
//!   up to the end of the render range.
//! - Video tracks composite bottom-to-top in track-list order (V1 first).
//!   Each clip is an input seeked with `-ss`/`-t`, optionally reversed and
//!   retimed (`reverse`, `setpts=PTS/speed`), conformed to the project rate
//!   (`fps`), cut to its exact frame count (`tpad` + `trim=end_frame`),
//!   letterboxed, colour-corrected (`colorlevels`, `eq`, `lut3d`), faded
//!   (`fade`, alpha), offset to its timeline start (`setpts`) and laid over the
//!   canvas (`overlay`, `eof_action=pass`).
//! - Cross dissolves extend both clips by half the transition (handles) and
//!   fade the incoming clip's alpha in over the outgoing one; dip-to-black fades
//!   each side to/from black.
//! - Titles are transparent `color` sources with `drawtext` (text read from a
//!   file, so no user text enters the filtergraph).
//! - Audio clips are resampled, reversed (`areverse`), retimed (`atempo`),
//!   cut to an exact sample count, gained, faded (`afade`) and delayed by an
//!   exact sample offset. Each track is mixed into a bus with its fader, pan
//!   and 3-band EQ; buses are mixed with `amix normalize=0`. Solo is honoured.
//! - An in/out range trims the composite to `[in, out)` frames.
//!
//! Only numbers produced by this module enter the filtergraph; file paths are
//! passed as separate process arguments, or as quoted, escaped option values
//! for LUT/font/text files. No shell is involved.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use crate::error::{CoreError, CoreResult};
use crate::media::{command, ffprobe_json, media_status, MediaStatus, Tools};
use crate::model::{dissolve_halves, Clip, ColorParams, Keyframe, MediaKind, Project, TextAlign, TextStyle, TitleClip, TitlePosition, Track, TrackKind, Transition, TransitionKind, TITLE_FONTS};
use crate::time::{ffmpeg_time, frames_to_micros, frames_to_samples, Rational};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264,
    Hevc,
    Mpeg4,
    Prores,
    Dnxhr,
    /// A single PNG frame (Grab Still).
    Png,
    /// Audio-only export.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    Aac,
    Pcm,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderSettings {
    pub output: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: Rational,
    pub video_codec: VideoCodec,
    /// H.264/HEVC CRF (0–51) or MPEG-4 qscale (1–31); ignored for ProRes/DNxHR.
    pub quality: u32,
    pub audio_codec: AudioCodec,
    pub audio_bitrate_kbps: u32,
    pub sample_rate: u32,
    pub overwrite: bool,
    /// Render only `[range_in, range_out)` timeline frames.
    #[serde(default)]
    pub range_in: Option<i64>,
    #[serde(default)]
    pub range_out: Option<i64>,
}

impl RenderSettings {
    pub fn from_project(p: &Project, output: &str) -> Self {
        Self {
            output: output.into(),
            width: p.settings.width,
            height: p.settings.height,
            frame_rate: p.settings.frame_rate,
            video_codec: VideoCodec::H264,
            quality: 20,
            audio_codec: AudioCodec::Aac,
            audio_bitrate_kbps: 192,
            sample_rate: p.settings.sample_rate,
            overwrite: false,
            range_in: None,
            range_out: None,
        }
    }

    pub fn validate(&self) -> CoreResult<()> {
        self.frame_rate.validate()?;
        if !(16..=8192).contains(&self.width) || !(16..=8192).contains(&self.height) || self.width % 2 != 0 || self.height % 2 != 0 {
            return Err(CoreError::Invalid(format!("output size {}x{} must be even and within 16–8192", self.width, self.height)));
        }
        if !(8_000..=192_000).contains(&self.sample_rate) {
            return Err(CoreError::Invalid("sample rate must be 8000–192000 Hz".into()));
        }
        match self.video_codec {
            VideoCodec::H264 | VideoCodec::Hevc if self.quality > 51 => return Err(CoreError::Invalid("CRF must be 0–51".into())),
            VideoCodec::Mpeg4 if !(1..=31).contains(&self.quality) => return Err(CoreError::Invalid("MPEG-4 quality must be 1–31".into())),
            _ => {}
        }
        if self.video_codec == VideoCodec::None && self.audio_codec == AudioCodec::None {
            return Err(CoreError::Invalid("choose a video or an audio codec; both are off".into()));
        }
        if self.audio_codec == AudioCodec::Aac && !(32..=512).contains(&self.audio_bitrate_kbps) {
            return Err(CoreError::Invalid("AAC bitrate must be 32–512 kbps".into()));
        }
        let ext = Path::new(&self.output).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let allowed: &[&str] = match (self.video_codec, self.audio_codec) {
            (VideoCodec::H264, _) | (VideoCodec::Hevc, _) => &["mp4", "mov", "mkv"],
            (VideoCodec::Mpeg4, _) => &["mp4", "avi", "mkv"],
            (VideoCodec::Prores, _) => &["mov"],
            (VideoCodec::Dnxhr, _) => &["mov", "mxf"],
            (VideoCodec::Png, _) => &["png"],
            (VideoCodec::None, AudioCodec::Pcm) => &["wav"],
            (VideoCodec::None, _) => &["m4a", "aac"],
        };
        if !allowed.contains(&ext.as_str()) {
            return Err(CoreError::Invalid(format!("this format must be saved as .{}", allowed.join(" / ."))));
        }
        if self.video_codec == VideoCodec::Png && self.audio_codec != AudioCodec::None {
            return Err(CoreError::Invalid("a still has no audio; set the audio codec to none".into()));
        }
        if self.audio_codec == AudioCodec::Pcm && ext == "mp4" {
            return Err(CoreError::Invalid("PCM audio is not supported in .mp4; use .mov or .mkv".into()));
        }
        Ok(())
    }
}

/// A file the plan needs on disk before FFmpeg runs (title text).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuxFile {
    pub path: String,
    pub contents: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderPlan {
    pub args: Vec<String>,
    pub filtergraph: String,
    /// Frames rendered (the range length).
    pub duration_frames: i64,
    pub duration_us: i64,
    pub range: (i64, i64),
    pub output: String,
    /// Where FFmpeg writes before the final rename.
    pub partial_output: String,
    pub video_clips: usize,
    pub audio_clips: usize,
    pub titles: usize,
    pub has_video: bool,
    /// Single-frame PNG output.
    #[serde(default)]
    pub still: bool,
    pub warnings: Vec<String>,
    pub aux_files: Vec<AuxFile>,
}

fn num(v: f64) -> String {
    format!("{v:.4}")
}

/// FFmpeg filters for primary color. Returns None when neutral.
pub fn color_filters(c: &ColorParams) -> Option<String> {
    if c.is_neutral() {
        return None;
    }
    let mut parts = vec![];
    let mut lv = vec![];
    if c.lift > 0.0 {
        lv.push(format!("romin={0}:gomin={0}:bomin={0}", num(c.lift)));
    } else if c.lift < 0.0 {
        lv.push(format!("rimin={0}:gimin={0}:bimin={0}", num(-c.lift)));
    }
    if c.gain > 1.0 {
        lv.push(format!("rimax={0}:gimax={0}:bimax={0}", num(1.0 / c.gain)));
    } else if c.gain < 1.0 {
        lv.push(format!("romax={0}:gomax={0}:bomax={0}", num(c.gain)));
    }
    if !lv.is_empty() {
        parts.push(format!("colorlevels={}", lv.join(":")));
    }
    if c.contrast != 1.0 || c.saturation != 1.0 || c.gamma != 1.0 {
        parts.push(format!("eq=contrast={}:saturation={}:gamma={}", num(c.contrast), num(c.saturation), num(c.gamma)));
    }
    Some(parts.join(","))
}

/// Quote a file path as a filter option value: forward slashes, `:` and `'`
/// escaped, wrapped in single quotes (works for Windows drive letters).
pub fn filter_path(p: &str) -> String {
    let s = p.replace('\\', "/").replace('\'', "'\\''").replace(':', "\\:");
    format!("'{s}'")
}

/// `atempo` accepts 0.5–2.0 per instance; chain instances for 0.25x–4x.
pub fn atempo_chain(speed: f64) -> Vec<String> {
    let mut s = speed;
    let mut out = vec![];
    while s >= 2.0 - 1e-9 {
        out.push("atempo=2.0".to_string());
        s /= 2.0;
    }
    while s <= 0.5 + 1e-9 {
        out.push("atempo=0.5".to_string());
        s /= 0.5;
    }
    if (s - 1.0).abs() > 1e-9 {
        out.push(format!("atempo={}", num(s)));
    }
    out
}

/// A font that drawtext can open on this machine.
pub fn default_font() -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = vec![];
    if let Ok(w) = std::env::var("WINDIR") {
        let f = Path::new(&w).join("Fonts");
        for n in ["segoeuib.ttf", "segoeui.ttf", "arialbd.ttf", "arial.ttf"] {
            c.push(f.join(n));
        }
    }
    for n in [
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/Library/Fonts/Arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    ] {
        c.push(PathBuf::from(n));
    }
    c.into_iter().find(|p| p.is_file())
}

fn title_xy(pos: TitlePosition) -> (&'static str, &'static str) {
    match pos {
        TitlePosition::Top => ("(w-text_w)/2", "h*0.08"),
        TitlePosition::Center => ("(w-text_w)/2", "(h-text_h)/2"),
        TitlePosition::Bottom => ("(w-text_w)/2", "h-text_h-h*0.08"),
        TitlePosition::LowerThird => ("w*0.07", "h*0.72"),
    }
}

/// Handles a clip needs for cross dissolves: (`lead` frames before its start,
/// `tail` frames after its end).
pub(crate) fn handles(t: &Track, c: &Clip) -> (i64, i64) {
    let lead = match c.transition_in {
        Some(tr) if tr.kind.overlaps() => dissolve_halves(tr.duration_frames).0,
        _ => 0,
    };
    let tail = next_transition(t, c).filter(|tr| tr.kind.overlaps()).map(|tr| dissolve_halves(tr.duration_frames).1).unwrap_or(0);
    (lead, tail)
}

/// The transition on the cut where `c` ends (stored on the clip that follows it).
fn next_transition(t: &Track, c: &Clip) -> Option<Transition> {
    t.clips.iter().find(|o| o.id != c.id && o.start == c.end()).and_then(|o| o.transition_in)
}

/// Dip-to-black halves at the clip's head (incoming) and tail (outgoing).
fn dips(t: &Track, c: &Clip) -> (i64, i64) {
    let head = match c.transition_in {
        Some(tr) if !tr.kind.overlaps() => tr.duration_frames - tr.duration_frames / 2,
        _ => 0,
    };
    let tail = next_transition(t, c).filter(|tr| !tr.kind.overlaps()).map(|tr| tr.duration_frames / 2).unwrap_or(0);
    (head, tail)
}

/// A clip whose picture was pre-rendered through its OpenFX effects
/// ([`crate::ofx`]): the plan reads this file instead of the clip's media.
#[derive(Debug, Clone)]
pub struct Prerendered {
    pub path: String,
    /// Frames the file starts after the clip's first (handle-inclusive) frame;
    /// the first frame is repeated to fill them, and they fall outside the range.
    pub pad_front: i64,
}

/// Translate the project into FFmpeg arguments. Pure: no processes run.
/// `check_files` controls whether media/LUT/font existence is verified (tests pass false).
pub fn build_plan(p: &Project, s: &RenderSettings, check_files: bool) -> CoreResult<RenderPlan> {
    build_plan_with(p, s, check_files, &std::collections::HashMap::new())
}

/// [`build_plan`] with OpenFX pre-renders substituted for their clips' media.
pub fn build_plan_with(p: &Project, s: &RenderSettings, check_files: bool, pre: &std::collections::HashMap<uuid::Uuid, Prerendered>) -> CoreResult<RenderPlan> {
    p.validate()?;
    s.validate()?;
    let rate = p.settings.frame_rate;
    let total = p.duration();
    if total <= 0 {
        return Err(CoreError::Invalid("the timeline is empty; place at least one clip".into()));
    }
    let r0 = s.range_in.unwrap_or(0).max(0);
    let r1 = s.range_out.unwrap_or(total).min(total);
    if r1 <= r0 {
        return Err(CoreError::Invalid(format!("the render range {r0}..{r1} is empty; set In before Out inside the timeline")));
    }
    let has_video = s.video_codec != VideoCodec::None;
    let has_audio = s.audio_codec != AudioCodec::None;
    let out_path = PathBuf::from(&s.output);
    let mut warnings = vec![];
    let mut aux_files = vec![];
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-nostdin".into(), "-v".into(), "error".into()];
    let mut graph: Vec<String> = vec![];
    let mut input_idx = 0usize;
    let (w, h) = (s.width, s.height);
    let fr = rate.ffmpeg();
    let check_media = |name: &str, path: &str| -> CoreResult<()> {
        if check_files && !Path::new(path).is_file() {
            return Err(CoreError::NotFound(format!("media {name:?} is missing at {path}; relink it before rendering")));
        }
        if out_path == Path::new(path) {
            return Err(CoreError::Invalid("output file would overwrite a source media file".into()));
        }
        Ok(())
    };

    // ---------------------------------------------------------------- video
    let mut video_clips = 0;
    let mut titles = 0;
    if has_video {
        graph.push(format!("color=c=black:s={w}x{h}:r={fr}:d={},format=yuv420p[base]", ffmpeg_time(r1, rate)));
        let mut last = "base".to_string();
        let mut layer = 0usize;
        for t in p.tracks.iter().filter(|t| t.kind == TrackKind::Video) {
            if t.muted {
                if !t.clips.is_empty() || !t.titles.is_empty() {
                    warnings.push(format!("track {} is hidden and is not rendered", t.name));
                }
                continue;
            }
            for c in &t.clips {
                let m = p.media(c.media_id).expect("validated");
                let (lead, tail) = handles(t, c);
                let (dip_in, dip_out) = dips(t, c);
                let dur = c.duration() + lead + tail;
                let at = c.start - lead;
                if at >= r1 || at + dur <= r0 {
                    continue; // contributes nothing to the range
                }
                if check_files && media_status(m) == MediaStatus::Missing {
                    check_media(&m.name, &m.path)?;
                }
                check_media(&m.name, &m.path)?;
                let prerendered = pre.get(&c.id);
                if let Some(pr) = prerendered {
                    args.extend(["-i".into(), pr.path.clone()]);
                } else if m.kind == MediaKind::Image {
                    args.extend(["-loop".into(), "1".into(), "-framerate".into(), fr.clone(), "-t".into(), ffmpeg_time(dur, rate)]);
                } else {
                    let si = c.source_in - lead;
                    let sl = c.source_len() + lead + tail;
                    args.extend(["-ss".into(), ffmpeg_time(si, rate), "-t".into(), ffmpeg_time(sl, rate)]);
                }
                if prerendered.is_none() {
                    args.extend(["-i".into(), m.path.clone()]);
                }
                let mut f = vec!["setpts=PTS-STARTPTS".to_string()];
                if let Some(pr) = prerendered.filter(|pr| pr.pad_front > 0) {
                    f.push(format!("tpad=start={}:start_mode=clone", pr.pad_front));
                }
                if c.reversed {
                    f.push("reverse".into());
                }
                if c.speed != 1.0 {
                    f.push(format!("setpts=PTS/{}", num(c.speed)));
                }
                f.push(format!("fps={fr}"));
                f.push("tpad=stop_mode=clone:stop=4".into());
                f.push(format!("trim=end_frame={dur}"));
                if !c.crop.is_none() {
                    let cr = c.crop;
                    f.push(format!(
                        "crop=iw*{}:ih*{}:iw*{}:ih*{}",
                        num(1.0 - cr.left - cr.right),
                        num(1.0 - cr.top - cr.bottom),
                        num(cr.left),
                        num(cr.top)
                    ));
                }
                f.push(format!("scale={w}:{h}:force_original_aspect_ratio=decrease,setsar=1,format=yuva420p"));
                if let Some(cf) = color_filters(&c.color) {
                    f.push(cf);
                }
                if let Some(lut) = &c.lut {
                    if check_files && !Path::new(lut).is_file() {
                        return Err(CoreError::NotFound(format!("LUT {lut} for clip {:?} is missing", c.name)));
                    }
                    f.push(format!("lut3d=file={}:interp=tetrahedral", filter_path(lut)));
                }
                for fx in c.vfx.iter().filter(|v| v.enabled && v.amount > 0.0) {
                    f.push(vfx_filter(fx, layer, w));
                }
                // Dips go through a colour (colour fades); clip fades and dissolves use alpha.
                let white = |tr: Option<Transition>| if tr.is_some_and(|tr| tr.kind == TransitionKind::DipToWhite) { ":c=white" } else { "" };
                if dip_in > 0 {
                    f.push(format!("fade=t=in:s=0:n={dip_in}{}", white(c.transition_in)));
                }
                if dip_out > 0 {
                    f.push(format!("fade=t=out:s={}:n={dip_out}{}", dur - dip_out, white(next_transition(t, c))));
                }
                f.push("format=yuva420p".into());
                let animated = !c.keyframes.is_empty();
                // Source frame of the layer's n-th output frame (`n` is 1-based in sendcmd and perspective).
                let src = |n: &str| match c.reversed {
                    true => format!("({}-{}*({n}-1))", c.source_out - 1, num(c.speed)),
                    false => format!("({}+{}*({n}-1))", c.source_in - lead, num(c.speed)),
                };
                let key = |g: fn(&Keyframe) -> f64, n: &str| keyed(&c.keyframes.iter().map(|k| (k.frame, g(k))).collect::<Vec<_>>(), &src(n));
                if animated {
                    let path = std::env::temp_dir().join("pirosman-render").join(format!("opacity-{}.txt", c.id));
                    aux_files.push(AuxFile { path: path.to_string_lossy().into(), contents: format!("0-1000000 [expr] colorchannelmixer aa '{}';\n", key(|k| k.opacity, "N")) });
                    f.push(format!("sendcmd=f={},colorchannelmixer,format=yuva420p", filter_path(&path.to_string_lossy())));
                } else if c.opacity < 1.0 {
                    f.push(format!("lutyuv=a=val*{}", num(c.opacity)));
                }
                let tf = c.transform;
                if !animated && (tf.scale_x != 1.0 || tf.scale_y != 1.0) {
                    f.push(format!("scale=trunc(iw*{}/2)*2:trunc(ih*{}/2)*2", num(tf.scale_x), num(tf.scale_y)));
                }
                if !animated && tf.rotation_deg != 0.0 {
                    let a = num(tf.rotation_deg.to_radians());
                    f.push(format!("rotate={a}:ow=rotw({a}):oh=roth({a}):c=none"));
                }
                // Overlapping transitions: this clip starts `lead` early, on top of the outgoing one.
                let incoming = c.transition_in.filter(|_| lead > 0);
                if let Some(tr) = incoming {
                    let d = tr.duration_frames;
                    match tr.kind {
                        TransitionKind::Wipe => f.push(format!(
                            "geq=lum='lum(X,Y)':cb='cb(X,Y)':cr='cr(X,Y)':a='alpha(X,Y)*lte(X,W*(N+1)/{})':enable='lt(n,{d})'",
                            d + 1
                        )),
                        TransitionKind::Slide | TransitionKind::Push => {} // moved by the overlay below
                        _ => f.push(format!("fade=t=in:s=0:n={d}:alpha=1")),
                    }
                } else if c.fade_in_frames > 0 && dip_in == 0 {
                    f.push(format!("fade=t=in:s=0:n={}:alpha=1", c.fade_in_frames));
                }
                if tail == 0 && dip_out == 0 && c.fade_out_frames > 0 {
                    f.push(format!("fade=t=out:s={}:n={}:alpha=1", dur - c.fade_out_frames, c.fade_out_frames));
                }
                if animated {
                    // Keyframed: centre the layer on a canvas a little larger than the frame, then pin
                    // its corners where scale, rotation and position put them on each frame. The
                    // transparent margin matters: perspective repeats edge pixels outside the quad.
                    let (sx, sy) = (key(|k| k.transform.scale_x, "in"), key(|k| k.transform.scale_y, "in"));
                    let (px, py) = (key(|k| k.transform.position_x, "in"), key(|k| k.transform.position_y, "in"));
                    let a = key(|k| k.transform.rotation_deg.to_radians(), "in");
                    let corner = |i: usize, dx: &str, dy: &str| {
                        format!(
                            "x{i}='W/2+({px})*{w}+({sx})*({dx})*cos({a})-({sy})*({dy})*sin({a})':y{i}='H/2+({py})*{h}+({sx})*({dx})*sin({a})+({sy})*({dy})*cos({a})'"
                        )
                    };
                    f.push(format!("pad={}:{}:(ow-iw)/2:(oh-ih)/2:color=black@0", w + 4, h + 4));
                    f.push(format!(
                        "perspective={}:{}:{}:{}:sense=destination:eval=frame",
                        corner(0, "-W/2", "-H/2"),
                        corner(1, "W/2", "-H/2"),
                        corner(2, "-W/2", "H/2"),
                        corner(3, "W/2", "H/2")
                    ));
                }
                f.push(format!("setpts=PTS-STARTPTS+{}/TB", ffmpeg_time(at, rate)));
                graph.push(format!("[{input_idx}:v:0]{}[v{layer}]", f.join(",")));
                let next = format!("o{layer}");
                // Slide and push move whole layers across the frame during the transition.
                let secs = |frames: i64| frames as f64 * rate.den as f64 / rate.num as f64;
                let mut dx = String::new();
                if let Some(tr) = incoming.filter(|tr| matches!(tr.kind, TransitionKind::Slide | TransitionKind::Push)) {
                    dx = format!("+main_w*max(0\\,1-(t-{})/{})", num(secs(at)), num(secs(tr.duration_frames)));
                }
                if let Some(tr) = next_transition(t, c).filter(|tr| tr.kind == TransitionKind::Push && tail > 0) {
                    let t0 = c.end() - dissolve_halves(tr.duration_frames).0;
                    dx = format!("-main_w*min(1\\,max(0\\,(t-{})/{}))", num(secs(t0)), num(secs(tr.duration_frames)));
                }
                graph.push(format!(
                    "[{last}][v{layer}]overlay=x=(main_w-overlay_w)/2+{}*main_w{dx}:y=(main_h-overlay_h)/2+{}*main_h:eof_action=pass:repeatlast=0[{next}]",
                    num(if animated { 0.0 } else { c.transform.position_x }),
                    num(if animated { 0.0 } else { c.transform.position_y })
                ));
                last = next;
                layer += 1;
                video_clips += 1;
                input_idx += 1;
            }
            for x in &t.titles {
                if x.start >= r1 || x.end() <= r0 {
                    continue;
                }
                let font = if check_files {
                    title_font(&x.style).ok_or_else(|| CoreError::Media("no system font found for titles (looked for Segoe UI, Arial, DejaVu Sans)".into()))?
                } else {
                    PathBuf::from("/fonts/test.ttf")
                };
                let txt = title_text_path(x);
                aux_files.push(AuxFile { path: txt.to_string_lossy().into(), contents: x.text.clone() });
                graph.push(format!("{}[v{layer}]", title_chain(x, w, h, p.settings.height, rate, &font, &txt)));
                let next = format!("o{layer}");
                let at = match x.transform {
                    Some(tf) => format!("x=(main_w-overlay_w)/2+{}*main_w:y=(main_h-overlay_h)/2+{}*main_h:", num(tf.position_x), num(tf.position_y)),
                    None => String::new(),
                };
                graph.push(format!("[{last}][v{layer}]overlay={at}eof_action=pass:repeatlast=0[{next}]"));
                last = next;
                layer += 1;
                titles += 1;
            }
        }
        graph.push(format!("[{last}]trim=start_frame={r0}:end_frame={r1},setpts=PTS-STARTPTS,fps={},format=yuv420p[vout]", s.frame_rate.ffmpeg()));
    }

    // ---------------------------------------------------------------- audio
    let sr = s.sample_rate;
    let smp = |f: i64| frames_to_samples(f, rate, sr);
    let total_samples = smp(r1);
    let mut audio_clips = 0;
    if has_audio {
        let soloing = p.tracks.iter().any(|t| t.kind == TrackKind::Audio && t.solo && !t.muted);
        let mut buses = vec![];
        for t in p.tracks.iter().filter(|t| t.kind == TrackKind::Audio) {
            if t.muted || (soloing && !t.solo) {
                continue;
            }
            let mut labels = vec![];
            for c in &t.clips {
                let m = p.media(c.media_id).expect("validated");
                let (lead, tail) = handles(t, c);
                let (dip_in, dip_out) = dips(t, c);
                let dur = c.duration() + lead + tail;
                let at = c.start - lead;
                if at >= r1 || at + dur <= r0 {
                    continue;
                }
                check_media(&m.name, &m.path)?;
                args.extend([
                    "-ss".into(),
                    ffmpeg_time(c.source_in - lead, rate),
                    "-t".into(),
                    ffmpeg_time(c.source_len() + lead + tail, rate),
                    "-i".into(),
                    m.path.clone(),
                ]);
                let n = smp(dur);
                let mut f = vec!["asetpts=PTS-STARTPTS".to_string(), format!("aresample={sr}"), "aformat=sample_fmts=fltp:channel_layouts=stereo".into()];
                if c.reversed {
                    f.push("areverse".into());
                }
                if c.speed != 1.0 {
                    f.extend(atempo_chain(c.speed));
                }
                f.push("apad".into());
                f.push(format!("atrim=end_sample={n}"));
                f.push(format!("volume={}dB", num(c.gain_db)));
                if let Some(tr) = c.transition_in.filter(|t| t.kind.overlaps()) {
                    f.push(format!("afade=t=in:ss=0:ns={}", smp(tr.duration_frames)));
                } else if dip_in > 0 {
                    f.push(format!("afade=t=in:ss=0:ns={}", smp(dip_in)));
                } else if c.fade_in_frames > 0 {
                    f.push(format!("afade=t=in:ss=0:ns={}", smp(c.fade_in_frames)));
                }
                if tail > 0 {
                    // The crossfade spans the whole transition: the next clip's lead plus our tail.
                    let d = tail + lead_of_next(t, c);
                    f.push(format!("afade=t=out:ss={}:ns={}", n - smp(d), smp(d)));
                } else if dip_out > 0 {
                    f.push(format!("afade=t=out:ss={}:ns={}", n - smp(dip_out), smp(dip_out)));
                } else if c.fade_out_frames > 0 {
                    f.push(format!("afade=t=out:ss={}:ns={}", n - smp(c.fade_out_frames), smp(c.fade_out_frames)));
                }
                f.push(format!("adelay=delays={}S:all=1", smp(at)));
                let label = format!("a{audio_clips}");
                graph.push(format!("[{input_idx}:a:0]{}[{label}]", f.join(",")));
                labels.push(format!("[{label}]"));
                audio_clips += 1;
                input_idx += 1;
            }
            if labels.is_empty() {
                continue;
            }
            let b = buses.len();
            let mut bus = vec![format!("{}amix=inputs={}:duration=longest:normalize=0", labels.join(""), labels.len())];
            if t.volume_db != 0.0 {
                bus.push(format!("volume={}dB", num(t.volume_db)));
            }
            if t.pan != 0.0 {
                let l = (1.0 - t.pan).min(1.0);
                let r = (1.0 + t.pan).min(1.0);
                bus.push(format!("pan=stereo|c0={}*c0|c1={}*c1", num(l), num(r)));
            }
            if t.eq_low_db != 0.0 {
                bus.push(format!("bass=g={}:f=100", num(t.eq_low_db)));
            }
            if t.eq_mid_db != 0.0 {
                bus.push(format!("equalizer=f=1000:t=o:w=1.5:g={}", num(t.eq_mid_db)));
            }
            if t.eq_high_db != 0.0 {
                bus.push(format!("treble=g={}:f=8000", num(t.eq_high_db)));
            }
            graph.push(format!("{}[bus{b}]", bus.join(",")));
            buses.push(format!("[bus{b}]"));
        }
        let range = format!("atrim=start_sample={}:end_sample={total_samples},asetpts=PTS-STARTPTS", smp(r0));
        if buses.is_empty() {
            graph.push(format!("anullsrc=r={sr}:cl=stereo,atrim=end_sample={total_samples},{range}[aout]"));
        } else {
            graph.push(format!(
                "{}amix=inputs={}:duration=longest:normalize=0,apad=whole_len={total_samples},{range}[aout]",
                buses.join(""),
                buses.len()
            ));
        }
    }

    let filtergraph = graph.join(";\n");
    let flat = filtergraph.replace('\n', "");
    if flat.len() > 8000 {
        // Long graphs exceed the Windows command-line limit: pass them as a file.
        let gp = std::env::temp_dir().join("pirosman-render").join(format!("graph-{}.txt", std::process::id()));
        aux_files.push(AuxFile { path: gp.to_string_lossy().into(), contents: flat });
        args.extend(["-/filter_complex".into(), gp.to_string_lossy().into()]);
    } else {
        args.extend(["-filter_complex".into(), flat]);
    }
    if has_video {
        args.extend(["-map".into(), "[vout]".into()]);
    }
    if has_audio {
        args.extend(["-map".into(), "[aout]".into()]);
    }
    let q = s.quality.to_string();
    match s.video_codec {
        VideoCodec::H264 => args.extend(["-c:v", "libx264", "-preset", "medium", "-crf", &q, "-pix_fmt", "yuv420p"].map(String::from)),
        VideoCodec::Hevc => args.extend(["-c:v", "libx265", "-preset", "medium", "-crf", &q, "-pix_fmt", "yuv420p", "-tag:v", "hvc1"].map(String::from)),
        VideoCodec::Mpeg4 => args.extend(["-c:v", "mpeg4", "-q:v", &q, "-pix_fmt", "yuv420p"].map(String::from)),
        VideoCodec::Prores => args.extend(["-c:v", "prores_ks", "-profile:v", "2", "-pix_fmt", "yuv422p10le"].map(String::from)),
        VideoCodec::Dnxhr => args.extend(["-c:v", "dnxhd", "-profile:v", "dnxhr_hq", "-pix_fmt", "yuv422p"].map(String::from)),
        VideoCodec::Png => args.extend(["-c:v", "png", "-frames:v", "1", "-update", "1"].map(String::from)),
        VideoCodec::None => {}
    }
    match s.audio_codec {
        AudioCodec::Aac => args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), format!("{}k", s.audio_bitrate_kbps), "-ar".into(), sr.to_string()]),
        AudioCodec::Pcm => args.extend(["-c:a".into(), "pcm_s16le".into(), "-ar".into(), sr.to_string()]),
        AudioCodec::None => {}
    }
    let ext = out_path.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
    if ["mp4", "mov", "m4a"].iter().any(|e| ext.eq_ignore_ascii_case(e)) {
        args.extend(["-movflags".into(), "+faststart".into()]);
    }
    let partial = out_path.with_extension(format!("partial.{ext}"));
    args.extend(["-progress".into(), "pipe:1".into(), "-nostats".into(), "-y".into(), partial.to_string_lossy().to_string()]);

    let frames = r1 - r0;
    Ok(RenderPlan {
        args,
        filtergraph,
        duration_frames: frames,
        duration_us: frames_to_micros(frames, rate),
        range: (r0, r1),
        output: s.output.clone(),
        partial_output: partial.to_string_lossy().to_string(),
        video_clips,
        audio_clips,
        titles,
        has_video,
        still: s.video_codec == VideoCodec::Png,
        warnings,
        aux_files,
    })
}

/// FFmpeg filters for one effect in a clip's stack (amount 0–1).
pub fn vfx_filter(fx: &crate::model::Vfx, layer: usize, width: u32) -> String {
    use crate::model::VfxKind::*;
    let a = fx.amount.clamp(0.0, 1.0);
    match fx.kind {
        GaussianBlur => format!("gblur=sigma={}", num(0.5 + a * 24.5)),
        Glow => {
            // Halation: a blurred, warm copy screened back over the image.
            let (x, y, z) = (format!("gx{layer}a"), format!("gx{layer}b"), format!("gx{layer}c"));
            format!(
                "split[{x}][{y}];[{y}]gblur=sigma={},colorchannelmixer=rr=1:gg=0.55:bb=0.35[{z}];[{x}][{z}]blend=all_mode=screen:all_opacity={}",
                num(6.0 + a * 20.0),
                num(a * 0.8)
            )
        }
        Vignette => {
            // vignette has no alpha formats: darken the colour, then put the alpha back.
            let (c, k, al) = (format!("vg{layer}c"), format!("vg{layer}k"), format!("vg{layer}a"));
            format!("split[{c}][{k}];[{k}]alphaextract[{al}];[{c}]vignette=a={}[{c}v];[{c}v][{al}]alphamerge", num(0.2 + a * 1.2))
        }
        FilmGrain => {
            let n = (a * 35.0).round().max(1.0) as i64;
            format!("noise=c0s={n}:c1s={n}:c2s={n}:allf=t") // colour planes only, not alpha
        }
        ChromaticAberration => {
            let n = ((a * 16.0 * width as f64 / 1920.0).round() as i64).max(1);
            format!("format=gbrap,rgbashift=rh={}:bh={n},format=yuva420p", -n)
        }
    }
}

/// FFmpeg expression for a value keyed at source frames `(frame, value)`, at
/// source frame `s`: linear between keys, held before the first and after the last.
fn keyed(keys: &[(i64, f64)], s: &str) -> String {
    let mut e = num(keys.last().map_or(0.0, |k| k.1));
    for w in keys.windows(2).rev() {
        let ((f0, v0), (f1, v1)) = (w[0], w[1]);
        e = format!("if(lt({s},{f1}),{}+{}*clip(({s}-{f0})/{},0,1),{e})", num(v0), num(v1 - v0), f1 - f0);
    }
    e
}

/// Lead handle of the clip that follows `c` with a cross dissolve.
fn lead_of_next(t: &Track, c: &Clip) -> i64 {
    next_transition(t, c).map(|tr| dissolve_halves(tr.duration_frames).0).unwrap_or(0)
}

fn title_text_path(x: &TitleClip) -> PathBuf {
    std::env::temp_dir().join("pirosman-render").join(format!("title-{}.txt", x.id))
}

fn title_chain(x: &TitleClip, w: u32, h: u32, project_h: u32, rate: Rational, font: &Path, text: &Path) -> String {
    let st = &x.style;
    // Project pixels to output pixels, times the title's own (uniform) scale.
    let k = (h as f64) / (project_h as f64) * x.transform.map_or(1.0, |t| t.scale_x);
    let size = ((x.font_size as f64) * k).round().max(4.0) as i64;
    // Free placement draws the block centred; the overlay then offsets it.
    let (tx, ty) = if x.transform.is_some() { ("(w-text_w)/2", "(h-text_h)/2") } else { title_xy(x.position) };
    let mut dt = format!(
        "drawtext=fontfile={}:textfile={}:expansion=none:fontsize={size}:fontcolor=0x{}:x={tx}:y={ty}:line_spacing={}",
        filter_path(&font.to_string_lossy()),
        filter_path(&text.to_string_lossy()),
        &x.color_hex[1..],
        ((size as f64) * st.line_spacing).floor() as i64
    );
    if st.shadow {
        dt.push_str(":shadowcolor=black@0.55:shadowx=2:shadowy=2");
    }
    if st.outline_px > 0 {
        dt.push_str(&format!(":borderw={}:bordercolor=0x{}", ((st.outline_px as f64) * k).round().max(1.0) as i64, &st.outline_hex[1..]));
    }
    if let Some(b) = &st.background {
        dt.push_str(&format!(":box=1:boxcolor=0x{}:boxborderw={}", &b[1..], ((size as f64) * 0.25).round() as i64));
    }
    match st.align {
        TextAlign::Left => {}
        TextAlign::Center => dt.push_str(":text_align=center"),
        TextAlign::Right => dt.push_str(":text_align=right"),
    }
    let mut f = vec![format!("color=c=black@0.0:s={w}x{h}:r={}:d={}", rate.ffmpeg(), ffmpeg_time(x.duration, rate)), "format=yuva420p".into(), dt];
    if x.fade_in_frames > 0 {
        f.push(format!("fade=t=in:s=0:n={}:alpha=1", x.fade_in_frames));
    }
    if x.fade_out_frames > 0 {
        f.push(format!("fade=t=out:s={}:n={}:alpha=1", x.duration - x.fade_out_frames, x.fade_out_frames));
    }
    if st.opacity < 1.0 {
        f.push(format!("lutyuv=a=val*{}", num(st.opacity)));
    }
    if let Some(tf) = x.transform.filter(|t| t.rotation_deg != 0.0) {
        let a = num(tf.rotation_deg.to_radians());
        f.push(format!("rotate={a}:ow=rotw({a}):oh=roth({a}):c=none"));
    }
    f.push(format!("setpts=PTS-STARTPTS+{}/TB", ffmpeg_time(x.start, rate)));
    f.join(",")
}

/// The font file for a title style: the chosen family (bold or regular) from the
/// Windows fonts folder or, elsewhere, from fontconfig; else the default title font.
pub fn title_font(s: &TextStyle) -> Option<PathBuf> {
    if !s.font.is_empty() || !s.bold {
        let fam = if s.font.is_empty() { "Segoe UI" } else { s.font.as_str() };
        if let (Ok(win), Some(f)) = (std::env::var("WINDIR"), TITLE_FONTS.iter().find(|f| f.0 == fam)) {
            let p = Path::new(&win).join("Fonts").join(if s.bold { f.2 } else { f.1 });
            if p.is_file() {
                return Some(p);
            }
        }
        if !cfg!(windows) && TITLE_FONTS.iter().any(|f| f.0 == fam) {
            if let Some(p) = fontconfig_file(fam, s.bold) {
                return Some(p);
            }
        }
    }
    default_font()
}

/// fontconfig's file for a family (or its usual stand-in, e.g. Liberation Sans for Arial).
/// Remembered per family and weight: render plans are rebuilt on every settings change.
/// shortcut: fonts installed while the app runs are picked up after a restart.
fn fontconfig_file(fam: &str, bold: bool) -> Option<PathBuf> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<(String, bool), Option<PathBuf>>>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap().get(&(fam.to_string(), bold)) {
        return hit.clone();
    }
    let q = format!("{fam}:weight={}", if bold { "bold" } else { "regular" });
    let found = crate::media::command(Path::new("fc-match"))
        .args(["--format=%{file}", &q])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .filter(|p| p.is_file());
    cache.lock().unwrap().insert((fam.to_string(), bold), found.clone());
    found
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderProgress {
    pub out_time_us: i64,
    pub duration_us: i64,
    pub fraction: f64,
    pub speed: Option<String>,
    pub fps: Option<f64>,
    /// What runs before FFmpeg's final pass, e.g. "OpenFX: clip 2 of 3".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderResult {
    pub output: String,
    pub size: u64,
    pub duration_seconds: f64,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub frame_rate: Option<String>,
}

/// Run FFmpeg for a plan. Progress comes from FFmpeg's `-progress` stream.
/// Setting `cancel` kills the process and removes the partial file. The
/// output is renamed into place only after FFmpeg succeeds, then probed.
pub fn execute(tools: &Tools, plan: &RenderPlan, overwrite: bool, cancel: Arc<AtomicBool>, mut on_progress: impl FnMut(RenderProgress)) -> CoreResult<RenderResult> {
    let out = Path::new(&plan.output);
    if out.exists() && !overwrite {
        return Err(CoreError::Conflict(format!("{} already exists; choose another name or allow overwrite", out.display())));
    }
    for a in &plan.aux_files {
        let p = Path::new(&a.path);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, &a.contents)?;
    }
    let mut child = command(&tools.ffmpeg)
        .args(&plan.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CoreError::Media(format!("could not start ffmpeg: {e}")))?;
    let stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut cur = RenderProgress { out_time_us: 0, duration_us: plan.duration_us, fraction: 0.0, speed: None, fps: None, stage: None };
    let partial = PathBuf::from(&plan.partial_output);
    let cleanup = |plan: &RenderPlan| {
        for a in &plan.aux_files {
            let _ = std::fs::remove_file(&a.path);
        }
    };
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&partial);
            cleanup(plan);
            return Err(CoreError::Media("render cancelled".into()));
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if let Some((k, v)) = line.split_once('=') {
                    match k {
                        "out_time_us" | "out_time_ms" => {
                            // FFmpeg reports out_time_ms in microseconds too (historical quirk).
                            if let Ok(us) = v.trim().parse::<i64>() {
                                cur.out_time_us = us.max(0);
                                cur.fraction = (cur.out_time_us as f64 / plan.duration_us.max(1) as f64).clamp(0.0, 1.0);
                            }
                        }
                        "speed" => cur.speed = Some(v.trim().to_string()),
                        "fps" => cur.fps = v.trim().parse().ok(),
                        "progress" => on_progress(cur.clone()),
                        _ => {}
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait()?;
    let stderr = err_thread.join().unwrap_or_default();
    cleanup(plan);
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        let tail: Vec<&str> = stderr.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(CoreError::Media(format!("ffmpeg failed ({status}): {}", tail.join(" | "))));
    }
    if out.exists() {
        std::fs::remove_file(out)?;
    }
    std::fs::rename(&partial, out)?;
    if plan.still {
        let size = std::fs::metadata(out)?.len();
        return Ok(RenderResult { output: out.to_string_lossy().into(), size, duration_seconds: 0.0, video_codec: Some("png".into()), audio_codec: None, width: None, height: None, frame_rate: None });
    }
    verify_output(tools, out, plan.has_video)
}

/// Probe a rendered file to prove it is real, playable media.
pub fn verify_output(tools: &Tools, out: &Path, expect_video: bool) -> CoreResult<RenderResult> {
    let probe = ffprobe_json(tools, out)?;
    let streams = probe["streams"].as_array().cloned().unwrap_or_default();
    let v = streams.iter().find(|s| s["codec_type"] == "video");
    let a = streams.iter().find(|s| s["codec_type"] == "audio");
    let duration_seconds = probe["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
    if (expect_video && v.is_none()) || (!expect_video && a.is_none()) || duration_seconds <= 0.0 {
        return Err(CoreError::Media(format!("{} was written but has no playable {} stream", out.display(), if expect_video { "video" } else { "audio" })));
    }
    Ok(RenderResult {
        output: out.to_string_lossy().to_string(),
        size: std::fs::metadata(out)?.len(),
        duration_seconds,
        video_codec: v.and_then(|v| v["codec_name"].as_str()).map(String::from),
        audio_codec: a.and_then(|a| a["codec_name"].as_str()).map(String::from),
        width: v.and_then(|v| v["width"].as_u64()),
        height: v.and_then(|v| v["height"].as_u64()),
        frame_rate: v.and_then(|v| v["avg_frame_rate"].as_str()).map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixtures::*;
    use crate::model::{Transition, TransitionKind};
    use crate::timeline::*;
    use uuid::Uuid;

    fn edited() -> (Project, Uuid) {
        let (mut p, mid) = project();
        let v1 = p.tracks[0].id;
        let ids = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 25, source_in: Some(50), source_out: Some(100) }).unwrap();
        (p, ids[0])
    }

    #[test]
    fn plan_trims_offsets_and_maps_streams() {
        let (p, _) = edited();
        let s = RenderSettings::from_project(&p, "/out/cut.mp4");
        let plan = build_plan(&p, &s, false).unwrap();
        let a = plan.args.join(" ");
        assert!(a.contains("-ss 2.000000 -t 2.000000 -i /media/intro.mp4"), "{a}");
        assert!(plan.filtergraph.contains("trim=end_frame=50"));
        assert!(plan.filtergraph.contains("setpts=PTS-STARTPTS+1.000000/TB"));
        assert!(plan.filtergraph.contains("color=c=black:s=1920x1080:r=25/1:d=3.000000"));
        assert!(plan.filtergraph.contains("atrim=end_sample=96000"), "2s @ 48k");
        assert!(plan.filtergraph.contains("adelay=delays=48000S:all=1"), "1s offset");
        assert!(plan.filtergraph.contains("apad=whole_len=144000"));
        assert!(a.contains("-c:v libx264"));
        assert!(a.ends_with("cut.partial.mp4"));
        assert_eq!((plan.video_clips, plan.audio_clips, plan.duration_frames), (1, 1, 75));
    }

    #[test]
    fn plan_applies_color_and_gain_and_mutes() {
        let (mut p, v) = edited();
        let mut col = ColorParams::default();
        col.saturation = 1.2;
        col.lift = 0.05;
        update_clip(&mut p, v, ClipPatch { color: Some(col), ..Default::default() }).unwrap();
        let a_id = link_group(&p, v).into_iter().find(|i| *i != v).unwrap();
        update_clip(&mut p, a_id, ClipPatch { gain_db: Some(-6.0), ..Default::default() }).unwrap();
        let plan = build_plan(&p, &RenderSettings::from_project(&p, "/o.mp4"), false).unwrap();
        assert!(plan.filtergraph.contains("colorlevels=romin=0.0500:gomin=0.0500:bomin=0.0500"));
        assert!(plan.filtergraph.contains("eq=contrast=1.0000:saturation=1.2000:gamma=1.0000"));
        assert!(plan.filtergraph.contains("volume=-6.0000dB"));
        let a1 = p.tracks[2].id;
        update_track(&mut p, a1, TrackPatch { muted: Some(true), ..Default::default() }).unwrap();
        let plan = build_plan(&p, &RenderSettings::from_project(&p, "/o.mp4"), false).unwrap();
        assert_eq!(plan.audio_clips, 0);
        assert!(plan.filtergraph.contains("anullsrc"));
    }

    #[test]
    fn settings_validation() {
        let (p, _) = edited();
        let mut s = RenderSettings::from_project(&p, "/o.mp4");
        s.video_codec = VideoCodec::Prores;
        assert!(build_plan(&p, &s, false).is_err(), "prores needs .mov");
        s.output = "/o.mov".into();
        assert!(build_plan(&p, &s, false).is_ok());
        s.width = 1921;
        assert!(build_plan(&p, &s, false).is_err());
        let (empty, _) = project();
        assert!(build_plan(&empty, &RenderSettings::from_project(&empty, "/o.mp4"), false).is_err());
        let mut s = RenderSettings::from_project(&p, "/o.mxf");
        s.video_codec = VideoCodec::Dnxhr;
        assert!(build_plan(&p, &s, false).unwrap().args.join(" ").contains("-profile:v dnxhr_hq"));
        s.video_codec = VideoCodec::None;
        s.audio_codec = AudioCodec::None;
        assert!(build_plan(&p, &s, false).is_err(), "nothing to render");
    }

    #[test]
    fn neutral_color_emits_no_filter() {
        assert_eq!(color_filters(&ColorParams::default()), None);
        let mut c = ColorParams::default();
        c.gain = 2.0;
        assert_eq!(color_filters(&c).unwrap(), "colorlevels=rimax=0.5000:gimax=0.5000:bimax=0.5000");
    }

    #[test]
    fn fades_speed_reverse_and_lut() {
        let (mut p, v) = edited();
        update_clip(&mut p, v, ClipPatch { fade_in_frames: Some(10), fade_out_frames: Some(5), lut: Some(Some("C:\\luts\\film look.cube".into())), ..Default::default() }).unwrap();
        let a_id = link_group(&p, v).into_iter().find(|i| *i != v).unwrap();
        update_clip(&mut p, a_id, ClipPatch { fade_in_frames: Some(10), ..Default::default() }).unwrap();
        let g = build_plan(&p, &RenderSettings::from_project(&p, "/o.mp4"), false).unwrap().filtergraph;
        assert!(g.contains("fade=t=in:s=0:n=10:alpha=1"), "{g}");
        assert!(g.contains("fade=t=out:s=45:n=5:alpha=1"));
        assert!(g.contains("lut3d=file='C\\:/luts/film look.cube'"));
        assert!(g.contains("afade=t=in:ss=0:ns=19200"), "10 frames @ 48k/25");
        update_clip(&mut p, v, ClipPatch { speed: Some(2.0), reversed: Some(true), fade_out_frames: Some(0), ..Default::default() }).unwrap();
        let g = build_plan(&p, &RenderSettings::from_project(&p, "/o.mp4"), false).unwrap().filtergraph;
        assert!(g.contains("reverse,setpts=PTS/2.0000,fps=25/1,tpad=stop_mode=clone:stop=4,trim=end_frame=25"), "{g}");
        assert!(g.contains("areverse,atempo=2.0000") || g.contains("areverse,atempo=2.0"), "{g}");
        assert_eq!(atempo_chain(4.0), vec!["atempo=2.0", "atempo=2.0"]);
        assert_eq!(atempo_chain(0.25), vec!["atempo=0.5", "atempo=0.5"]);
        assert_eq!(atempo_chain(1.5), vec!["atempo=1.5000"]);
    }

    #[test]
    fn cross_dissolve_extends_handles() {
        let (mut p, mid) = project();
        let v1 = p.tracks[0].id;
        place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(50) }).unwrap();
        let b = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 50, source_in: Some(100), source_out: Some(150) }).unwrap()[0];
        update_clip(&mut p, b, ClipPatch { transition_in: Some(Some(Transition { kind: TransitionKind::CrossDissolve, duration_frames: 10 })), ..Default::default() }).unwrap();
        let plan = build_plan(&p, &RenderSettings::from_project(&p, "/o.mp4"), false).unwrap();
        let a = plan.args.join(" ");
        assert!(a.contains("-ss 0.000000 -t 2.200000"), "outgoing clip runs 5 frames late: {a}");
        assert!(a.contains("-ss 3.800000 -t 2.200000"), "incoming clip starts 5 frames early: {a}");
        assert!(plan.filtergraph.contains("trim=end_frame=55"));
        assert!(plan.filtergraph.contains("fade=t=in:s=0:n=10:alpha=1,setpts=PTS-STARTPTS+1.800000/TB"));
        assert!(plan.filtergraph.contains("afade=t=in:ss=0:ns=19200"));
        update_clip(&mut p, b, ClipPatch { transition_in: Some(Some(Transition { kind: TransitionKind::DipToBlack, duration_frames: 10 })), ..Default::default() }).unwrap();
        let g = build_plan(&p, &RenderSettings::from_project(&p, "/o.mp4"), false).unwrap().filtergraph;
        assert!(g.contains("fade=t=out:s=45:n=5,") && g.contains("fade=t=in:s=0:n=5,"), "{g}");
    }

    #[test]
    fn clip_layers_keep_alpha_and_apply_opacity() {
        use crate::model::{Vfx, VfxKind};
        let (mut p, c) = edited();
        let fx = |kind| Vfx { id: Uuid::new_v4(), kind, amount: 0.5, enabled: true };
        let vfx = vec![fx(VfxKind::Vignette), fx(VfxKind::FilmGrain), fx(VfxKind::ChromaticAberration)];
        update_clip(&mut p, c, ClipPatch { opacity: Some(0.5), vfx: Some(vfx), ..Default::default() }).unwrap();
        let g = build_plan(&p, &RenderSettings::from_project(&p, "/o.mp4"), false).unwrap().filtergraph;
        // Transparent PNGs keep their alpha up to the overlay.
        assert!(g.contains("setsar=1,format=yuva420p") && !g.contains("setsar=1,format=yuv420p"), "{g}");
        assert!(g.contains("format=yuva420p,lutyuv=a=val*0.5000"), "{g}");
        assert!(g.contains("alphaextract") && g.contains("alphamerge"), "vignette restores alpha: {g}");
        assert!(g.contains("noise=c0s=") && !g.contains("alls="), "grain leaves alpha alone: {g}");
        assert!(g.contains("format=gbrap,rgbashift") && !g.contains(",format=yuv420p,"), "{g}");
        assert!(update_clip(&mut p, c, ClipPatch { opacity: Some(1.5), ..Default::default() }).is_err());
    }

    #[test]
    fn styled_and_placed_titles() {
        use crate::model::{TextAlign, TextStyle, Transform};
        let (mut p, _) = edited();
        let v2 = p.tracks[1].id;
        let t = add_title(&mut p, NewTitle { track_id: v2, start: 0, duration: 25, text: "Hello".into(), font_size: Some(60), color_hex: None, position: None }).unwrap();
        // Default style: the original drawtext options, preset placement.
        let g = build_plan(&p, &RenderSettings::from_project(&p, "/o.mp4"), false).unwrap().filtergraph;
        assert!(g.contains("x=w*0.07:y=h*0.72:line_spacing=12:shadowcolor=black@0.55:shadowx=2:shadowy=2,"), "{g}");
        assert!(!g.contains("borderw") && !g.contains("box=1") && !g.contains("text_align"), "{g}");
        let style = TextStyle { align: TextAlign::Center, outline_px: 4, outline_hex: "#ff0000".into(), shadow: false, background: Some("#00000080".into()), opacity: 0.5, ..Default::default() };
        let tf = Transform { scale_x: 2.0, scale_y: 2.0, position_x: 0.1, position_y: -0.2, rotation_deg: 30.0 };
        update_title(&mut p, t, TitlePatch { style: Some(style), transform: Some(Some(tf)), ..Default::default() }).unwrap();
        let g = build_plan(&p, &RenderSettings::from_project(&p, "/o.mp4"), false).unwrap().filtergraph;
        for needle in [
            "fontsize=120:", // uniform scale multiplies the size
            "x=(w-text_w)/2:y=(h-text_h)/2",
            ":borderw=8:bordercolor=0xff0000",
            ":box=1:boxcolor=0x00000080:boxborderw=30",
            ":text_align=center",
            "lutyuv=a=val*0.5000",
            "rotate=0.5236:",
            "overlay=x=(main_w-overlay_w)/2+0.1000*main_w:y=(main_h-overlay_h)/2+-0.2000*main_h:",
        ] {
            assert!(g.contains(needle), "{needle} missing: {g}");
        }
        assert!(!g.contains("shadowcolor"), "{g}");
        // Back to the preset: the overlay loses its offsets.
        update_title(&mut p, t, TitlePatch { transform: Some(None), ..Default::default() }).unwrap();
        assert!(build_plan(&p, &RenderSettings::from_project(&p, "/o.mp4"), false).unwrap().filtergraph.contains("x=w*0.07:y=h*0.72"));
        let bad = |s: TextStyle| update_title(&mut p.clone(), t, TitlePatch { style: Some(s), ..Default::default() }).is_err();
        assert!(bad(TextStyle { font: "Wingdings Pro".into(), ..Default::default() }));
        assert!(bad(TextStyle { background: Some("#000000".into()), ..Default::default() }));
        assert!(bad(TextStyle { opacity: 2.0, ..Default::default() }));
        let skew = Transform { scale_x: 1.0, scale_y: 2.0, ..Default::default() };
        assert!(update_title(&mut p, t, TitlePatch { transform: Some(Some(skew)), ..Default::default() }).is_err());
    }

    #[test]
    fn titles_mixer_range_and_formats() {
        let (mut p, _) = edited();
        let v2 = p.tracks[1].id;
        add_title(&mut p, NewTitle { track_id: v2, start: 30, duration: 25, text: "Harbour: 'night' 100%".into(), font_size: Some(90), color_hex: Some("#F4C430".into()), position: Some(crate::model::TitlePosition::Center) }).unwrap();
        let a1 = p.tracks[2].id;
        update_track(&mut p, a1, TrackPatch { pan: Some(-0.5), eq_low_db: Some(3.0), eq_high_db: Some(-2.0), volume_db: Some(-1.5), ..Default::default() }).unwrap();
        let mut s = RenderSettings::from_project(&p, "/o.mp4");
        s.range_in = Some(30);
        s.range_out = Some(60);
        let plan = build_plan(&p, &s, false).unwrap();
        let g = &plan.filtergraph;
        assert_eq!((plan.titles, plan.duration_frames, plan.range), (1, 30, (30, 60)));
        assert!(g.contains("drawtext=fontfile='/fonts/test.ttf':textfile="), "{g}");
        assert!(!g.contains("Harbour"), "title text never enters the filtergraph");
        assert_eq!(plan.aux_files[0].contents, "Harbour: 'night' 100%");
        assert!(g.contains("fontsize=90:fontcolor=0xf4c430") || g.contains("fontsize=90:fontcolor=0xF4C430"));
        assert!(g.contains("pan=stereo|c0=1.0000*c0|c1=0.5000*c1"));
        assert!(g.contains("bass=g=3.0000:f=100") && g.contains("treble=g=-2.0000:f=8000") && g.contains("volume=-1.5000dB"));
        assert!(g.contains("trim=start_frame=30:end_frame=60"));
        assert!(g.contains("atrim=start_sample=57600:end_sample=115200"));
        let mut s = RenderSettings::from_project(&p, "/o.wav");
        s.video_codec = VideoCodec::None;
        s.audio_codec = AudioCodec::Pcm;
        let plan = build_plan(&p, &s, false).unwrap();
        assert!(!plan.has_video && !plan.filtergraph.contains("[vout]") && !plan.args.contains(&"[vout]".to_string()));
        s.range_in = Some(70);
        s.range_out = Some(60);
        assert!(build_plan(&p, &s, false).is_err());
        let mut s = RenderSettings::from_project(&p, "/o.mov");
        s.video_codec = VideoCodec::Hevc;
        assert!(build_plan(&p, &s, false).unwrap().args.join(" ").contains("-c:v libx265"));
    }

    #[test]
    fn solo_isolates_tracks() {
        let (mut p, _) = edited();
        let a2 = p.tracks[3].id;
        update_track(&mut p, a2, TrackPatch { solo: Some(true), ..Default::default() }).unwrap();
        let plan = build_plan(&p, &RenderSettings::from_project(&p, "/o.mp4"), false).unwrap();
        assert_eq!(plan.audio_clips, 0, "A1 is not soloed, A2 is empty");
    }

    #[test]
    fn transform_crop_vfx_and_still() {
        use crate::model::{Crop, Transform, Vfx, VfxKind};
        let (mut p, v) = edited();
        let all = [VfxKind::GaussianBlur, VfxKind::Glow, VfxKind::Vignette, VfxKind::FilmGrain, VfxKind::ChromaticAberration];
        update_clip(
            &mut p,
            v,
            ClipPatch {
                transform: Some(Transform { scale_x: 0.5, scale_y: 0.5, position_x: 0.25, position_y: -0.1, rotation_deg: 90.0 }),
                crop: Some(Crop { left: 0.1, right: 0.0, top: 0.2, bottom: 0.0 }),
                vfx: Some(all.iter().map(|k| Vfx { id: Uuid::new_v4(), kind: *k, amount: 0.5, enabled: true }).collect()),
                ..Default::default()
            },
        )
        .unwrap();
        let g = build_plan(&p, &RenderSettings::from_project(&p, "/o.mp4"), false).unwrap().filtergraph;
        assert!(g.contains("crop=iw*0.9000:ih*0.8000:iw*0.1000:ih*0.2000"), "{g}");
        assert!(g.contains("scale=trunc(iw*0.5000/2)*2:trunc(ih*0.5000/2)*2"));
        assert!(g.contains("rotate=1.5708:ow=rotw(1.5708):oh=roth(1.5708):c=none"));
        assert!(g.contains("overlay=x=(main_w-overlay_w)/2+0.2500*main_w:y=(main_h-overlay_h)/2+-0.1000*main_h"));
        for needle in ["gblur=sigma=12.7500", "blend=all_mode=screen:all_opacity=0.4000", "vignette=a=0.8000", "noise=c0s=18:c1s=18:c2s=18:allf=t", "rgbashift=rh=-8:bh=8"] {
            assert!(g.contains(needle), "{needle} missing: {g}");
        }
        let mut s = RenderSettings::from_project(&p, "/still.png");
        s.video_codec = VideoCodec::Png;
        assert!(build_plan(&p, &s, false).is_err(), "a still must not carry audio");
        s.audio_codec = AudioCodec::None;
        s.range_in = Some(30);
        s.range_out = Some(31);
        let plan = build_plan(&p, &s, false).unwrap();
        assert!(plan.still && plan.args.join(" ").contains("-c:v png -frames:v 1"));
    }

    #[test]
    fn filter_paths_are_escaped() {
        assert_eq!(filter_path("C:\\Users\\a b\\it's.cube"), "'C\\:/Users/a b/it'\\''s.cube'");
    }

    /// Filter names in a filtergraph: split on `,`/`;` outside quotes and escapes,
    /// drop `[label]`s, keep what precedes `=`.
    fn filter_names(graph: &str) -> std::collections::BTreeSet<String> {
        let (mut out, mut cur, mut quoted, mut chars) = (std::collections::BTreeSet::new(), String::new(), false, graph.chars());
        let mut flush = |s: &mut String| {
            let mut t = s.trim();
            while let Some(rest) = t.strip_prefix('[') {
                t = rest.split_once(']').map_or("", |x| x.1).trim_start();
            }
            let name = t.split(['=', '[']).next().unwrap_or("").trim();
            if !name.is_empty() {
                out.insert(name.to_string());
            }
            s.clear();
        };
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    cur.push(c);
                    cur.extend(chars.next());
                }
                '\'' => {
                    quoted = !quoted;
                    cur.push(c);
                }
                ',' | ';' if !quoted => flush(&mut cur),
                _ => cur.push(c),
            }
        }
        flush(&mut cur);
        out
    }

    /// Every filter any render plan can use must be in scripts/ffmpeg-requirements.txt,
    /// which both fetch scripts check the pinned FFmpeg builds against.
    #[test]
    fn render_filters_are_all_required_of_the_pinned_ffmpeg() {
        use crate::model::{Keyframe, TextAlign, TextStyle, Transform, Vfx, VfxKind};
        let required: std::collections::BTreeSet<&str> = include_str!("../../../scripts/ffmpeg-requirements.txt")
            .lines()
            .filter_map(|l| l.strip_prefix("filter "))
            .map(str::trim)
            .collect();
        let mut used = std::collections::BTreeSet::new();
        let mut plan = |p: &Project| used.extend(filter_names(&build_plan(p, &RenderSettings::from_project(p, "/o.mp4"), false).unwrap().filtergraph));
        // Grade, LUT, every effect, opacity, crop, transform, fades, speed, reverse, mixer, titles.
        let (mut p, c) = edited();
        let fx = |kind| Vfx { id: Uuid::new_v4(), kind, amount: 0.5, enabled: true };
        let mut col = crate::model::ColorParams::default();
        col.saturation = 1.3;
        col.lift = 0.05;
        update_clip(&mut p, c, ClipPatch {
            color: Some(col),
            lut: Some(Some("/luts/look.cube".into())),
            vfx: Some([VfxKind::GaussianBlur, VfxKind::Glow, VfxKind::Vignette, VfxKind::FilmGrain, VfxKind::ChromaticAberration].map(fx).to_vec()),
            opacity: Some(0.5),
            crop: Some(crate::model::Crop { left: 0.1, ..Default::default() }),
            transform: Some(Transform { scale_x: 1.5, scale_y: 1.2, rotation_deg: 10.0, ..Default::default() }),
            fade_in_frames: Some(5),
            fade_out_frames: Some(5),
            ..Default::default()
        })
        .unwrap();
        let (v2, a1) = (p.tracks[1].id, p.tracks[2].id);
        update_track(&mut p, a1, TrackPatch { pan: Some(-0.5), eq_low_db: Some(3.0), eq_mid_db: Some(-2.0), eq_high_db: Some(1.0), volume_db: Some(-1.0), ..Default::default() }).unwrap();
        let t = add_title(&mut p, NewTitle { track_id: v2, start: 25, duration: 25, text: "A, b; 'c'".into(), font_size: Some(60), color_hex: None, position: None }).unwrap();
        let style = TextStyle { align: TextAlign::Center, outline_px: 2, background: Some("#00000080".into()), opacity: 0.5, ..Default::default() };
        update_title(&mut p, t, TitlePatch { style: Some(style), transform: Some(Some(Transform { rotation_deg: 15.0, ..Default::default() })), fade_in_frames: Some(3), ..Default::default() }).unwrap();
        plan(&p);
        let mut fast = p.clone();
        update_clip(&mut fast, c, ClipPatch { speed: Some(2.0), reversed: Some(true), ..Default::default() }).unwrap();
        plan(&fast);
        // Keyframes.
        let k = |frame, x| Keyframe { frame, transform: Transform { position_x: x, ..Default::default() }, opacity: 0.5 };
        let mut keyed = p.clone();
        update_clip(&mut keyed, c, ClipPatch { keyframes: Some(vec![k(50, -0.2), k(90, 0.2)]), ..Default::default() }).unwrap();
        plan(&keyed);
        // Every transition, across a cut with handles on both sides.
        let (mut cut, mid) = project();
        let v1 = cut.tracks[0].id;
        place_clip(&mut cut, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(50), source_out: Some(100) }).unwrap();
        let b = place_clip(&mut cut, PlaceClip { media_id: mid, track_id: v1, start: 50, source_in: Some(120), source_out: Some(170) }).unwrap()[0];
        for kind in [TransitionKind::CrossDissolve, TransitionKind::DipToBlack, TransitionKind::DipToWhite, TransitionKind::Wipe, TransitionKind::Slide, TransitionKind::Push] {
            update_clip(&mut cut, b, ClipPatch { transition_in: Some(Some(Transition { kind, duration_frames: 10 })), ..Default::default() }).unwrap();
            plan(&cut);
        }
        // The extractor sees through quotes and escapes (sanity: these are real filters).
        assert!(used.contains("perspective") && used.contains("geq") && used.contains("drawtext") && used.contains("bass"), "{used:?}");
        let missing: Vec<_> = used.iter().filter(|f| !required.contains(f.as_str())).collect();
        assert!(missing.is_empty(), "render plans use filters scripts/ffmpeg-requirements.txt does not list: {missing:?}");
    }
}

//! The authoritative project model.
//!
//! Every persistent entity carries a stable UUID. Unknown JSON fields are kept
//! in `extra` maps (via `#[serde(flatten)]`) so that a project written by a
//! newer build survives a load/save cycle in this build.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};
use crate::time::Rational;

pub const PROJECT_FORMAT: &str = "pirosman-project";
/// v2 adds optional fields (fades, transitions, speed, titles, markers, mixer,
/// LUTs, in/out range). Every v2 field is omitted from JSON while it holds its
/// default, so v1 files and v1 revision objects serialize byte-for-byte as
/// before (revision ids are content hashes and must not change).
pub const SCHEMA_VERSION: u32 = 2;

fn one() -> f64 {
    1.0
}
fn is_one(v: &f64) -> bool {
    *v == 1.0
}
fn is_zero_f(v: &f64) -> bool {
    *v == 0.0
}
fn is_zero_i(v: &i64) -> bool {
    *v == 0
}
fn is_false(v: &bool) -> bool {
    !*v
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub format: String,
    pub schema_version: u32,
    pub id: Uuid,
    pub name: String,
    pub settings: ProjectSettings,
    #[serde(default)]
    pub media: Vec<MediaRef>,
    #[serde(default)]
    pub tracks: Vec<Track>,
    /// Timeline markers (v2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Timeline in/out range used for range rendering (v2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_in: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_out: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: Uuid,
    pub frame: i64,
    pub name: String,
    /// One of the named marker colors (`yellow`, `red`, `blue`, `green`, `purple`).
    pub color: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

pub const MARKER_COLORS: [&str; 5] = ["yellow", "red", "blue", "green", "purple"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSettings {
    pub width: u32,
    pub height: u32,
    pub frame_rate: Rational,
    pub sample_rate: u32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self { width: 1920, height: 1080, frame_rate: Rational::new(25, 1), sample_rate: 48_000, extra: Map::new() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Video,
    Audio,
    Image,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaRef {
    pub id: Uuid,
    pub name: String,
    /// Current filesystem location. Not an identity: media can be relinked.
    pub path: String,
    /// SHA-256 of the complete file contents (hex), when computed.
    pub sha256: Option<String>,
    pub size: u64,
    /// Modification time (unix seconds) observed when the hash was computed.
    pub modified: Option<i64>,
    pub kind: MediaKind,
    /// Duration in project frames (None for still images).
    pub duration_frames: Option<i64>,
    pub has_video: bool,
    pub has_audio: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<Rational>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub container: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: Uuid,
    pub kind: TrackKind,
    pub name: String,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub clips: Vec<Clip>,
    /// Text / title overlays on a video track (v2). They occupy the track like clips.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub titles: Vec<TitleClip>,
    /// Mixer (audio tracks, v2): solo, fader, balance and 3-band EQ.
    #[serde(default, skip_serializing_if = "is_false")]
    pub solo: bool,
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub volume_db: f64,
    /// Stereo balance −1 (left) … +1 (right).
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub pan: f64,
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub eq_low_db: f64,
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub eq_mid_db: f64,
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub eq_high_db: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

pub const MIX_DB_RANGE: (f64, f64) = (-60.0, 12.0);
pub const EQ_DB_RANGE: (f64, f64) = (-18.0, 18.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    CrossDissolve,
    DipToBlack,
    DipToWhite,
    /// Edge wipe, left to right.
    Wipe,
    /// The incoming clip slides in from the right over the outgoing one.
    Slide,
    /// The incoming clip pushes the outgoing one out to the left.
    Push,
}

impl TransitionKind {
    /// Blends the two clips across the cut (both play during it, so it needs
    /// source handles); the other kinds dip through a colour instead.
    pub fn overlaps(self) -> bool {
        !matches!(self, TransitionKind::DipToBlack | TransitionKind::DipToWhite)
    }

    pub fn label(self) -> &'static str {
        match self {
            TransitionKind::CrossDissolve => "Cross dissolve",
            TransitionKind::DipToBlack => "Dip to black",
            TransitionKind::DipToWhite => "Dip to white",
            TransitionKind::Wipe => "Edge wipe",
            TransitionKind::Slide => "Slide",
            TransitionKind::Push => "Push",
        }
    }
}

/// A transition across the cut where this clip begins: it blends from the
/// clip that ends exactly at this clip's start on the same track. Stored only
/// on the incoming clip, so the two sides of a cut can never disagree.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub kind: TransitionKind,
    pub duration_frames: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: Uuid,
    pub media_id: Uuid,
    pub name: String,
    /// Timeline position of the first frame (project frames).
    pub start: i64,
    /// Source in point, inclusive (project frames from media start).
    pub source_in: i64,
    /// Source out point, exclusive (project frames from media start).
    pub source_out: i64,
    #[serde(default)]
    pub gain_db: f64,
    #[serde(default)]
    pub color: ColorParams,
    /// Clips sharing a link id (a video clip and its sync audio) are edited together.
    #[serde(default)]
    pub link_id: Option<Uuid>,
    /// Fades in timeline frames (v2).
    #[serde(default, skip_serializing_if = "is_zero_i")]
    pub fade_in_frames: i64,
    #[serde(default, skip_serializing_if = "is_zero_i")]
    pub fade_out_frames: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_in: Option<Transition>,
    /// Playback speed 0.25…4.0 (v2). Timeline duration = round(source frames / speed).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub speed: f64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reversed: bool,
    /// Path to a `.cube` 3D LUT applied after the primary grade (v2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lut: Option<String>,
    /// Inspector transform (v3): scale, position, rotation.
    #[serde(default, skip_serializing_if = "Transform::is_identity")]
    pub transform: Transform,
    /// Source crop as fractions of each edge (v3).
    #[serde(default, skip_serializing_if = "Crop::is_none")]
    pub crop: Crop,
    /// Layer opacity 0–1 over the tracks below (video only).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f64,
    /// Animated transform and opacity, sorted by frame; when present they
    /// replace `transform` and `opacity` (video only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keyframes: Vec<Keyframe>,
    /// Effects stack applied after the grade, in order (v3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vfx: Vec<Vfx>,
    /// Third-party OpenFX effects, applied to the source frames in order (v4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ofx: Vec<OfxEffect>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One OpenFX plugin instance on a clip. The plugin is found again by
/// `plugin_id` on any machine; `binary` and `index` are where it was last seen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfxEffect {
    pub id: Uuid,
    pub plugin_id: String,
    pub label: String,
    /// The plugin's group path, e.g. "Sapphire Lighting" or "BCC/Blur & Sharpen".
    #[serde(default)]
    pub grouping: String,
    #[serde(default)]
    pub binary: String,
    #[serde(default)]
    pub index: u32,
    /// Parameter values by name; parameters left out use the plugin default.
    #[serde(default)]
    pub params: Map<String, Value>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

pub const SPEED_RANGE: (f64, f64) = (0.25, 4.0);

/// Most keyframes one clip may carry (each adds a step to the export's expressions).
pub const MAX_KEYFRAMES: usize = 64;

/// The layer's transform and opacity at `frame`, a *source* frame, so keyframes
/// stay with the picture through trims, slips and splits. Values between
/// keyframes are linear; before the first and after the last they hold.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub frame: i64,
    pub transform: Transform,
    #[serde(default = "one")]
    pub opacity: f64,
}

/// Position is a fraction of the frame (0 = centred, ±1 = one frame width/height).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub scale_x: f64,
    pub scale_y: f64,
    pub position_x: f64,
    pub position_y: f64,
    pub rotation_deg: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Self { scale_x: 1.0, scale_y: 1.0, position_x: 0.0, position_y: 0.0, rotation_deg: 0.0 }
    }
}

impl Transform {
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }
    pub fn validate(&self) -> CoreResult<()> {
        let ok = |v: f64, lo: f64, hi: f64| v.is_finite() && v >= lo && v <= hi;
        if !ok(self.scale_x, 0.05, 8.0) || !ok(self.scale_y, 0.05, 8.0) {
            return Err(CoreError::Invalid("scale must be 0.05–8".into()));
        }
        if !ok(self.position_x, -2.0, 2.0) || !ok(self.position_y, -2.0, 2.0) {
            return Err(CoreError::Invalid("position must be within ±2 frames".into()));
        }
        if !ok(self.rotation_deg, -360.0, 360.0) {
            return Err(CoreError::Invalid("rotation must be within ±360°".into()));
        }
        Ok(())
    }
}

/// Fractions of the source cut away from each edge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

impl Crop {
    pub fn is_none(&self) -> bool {
        *self == Self::default()
    }
    pub fn validate(&self) -> CoreResult<()> {
        for v in [self.left, self.right, self.top, self.bottom] {
            if !v.is_finite() || !(0.0..=0.45).contains(&v) {
                return Err(CoreError::Invalid("each crop edge must be 0–45%".into()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VfxKind {
    GaussianBlur,
    Glow,
    Vignette,
    FilmGrain,
    ChromaticAberration,
}

/// One effect in a clip's stack. `amount` is 0–1 for every kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vfx {
    pub id: Uuid,
    pub kind: VfxKind,
    pub amount: f64,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl Clip {
    pub fn new(media_id: Uuid, name: &str, start: i64, source_in: i64, source_out: i64) -> Self {
        Self {
            id: Uuid::new_v4(),
            media_id,
            name: name.into(),
            start,
            source_in,
            source_out,
            gain_db: 0.0,
            color: ColorParams::default(),
            link_id: None,
            fade_in_frames: 0,
            fade_out_frames: 0,
            transition_in: None,
            speed: 1.0,
            reversed: false,
            lut: None,
            transform: Transform::default(),
            crop: Crop::default(),
            opacity: 1.0,
            keyframes: Vec::new(),
            vfx: vec![],
            ofx: vec![],
            extra: Map::new(),
        }
    }
    /// Source frames used (out − in).
    pub fn source_len(&self) -> i64 {
        self.source_out - self.source_in
    }
    /// Timeline frames occupied: source frames divided by speed, rounded.
    pub fn duration(&self) -> i64 {
        if self.speed == 1.0 {
            self.source_len()
        } else {
            ((self.source_len() as f64) / self.speed).round().max(1.0) as i64
        }
    }
    pub fn end(&self) -> i64 {
        self.start + self.duration()
    }
    /// Source frames covered by `timeline_frames` of this clip at its speed.
    pub fn source_frames_for(&self, timeline_frames: i64) -> i64 {
        if self.speed == 1.0 {
            timeline_frames
        } else {
            ((timeline_frames as f64) * self.speed).round() as i64
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitlePosition {
    Top,
    Center,
    Bottom,
    LowerThird,
}

/// A text overlay on a video track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TitleClip {
    pub id: Uuid,
    pub start: i64,
    pub duration: i64,
    pub text: String,
    /// Font size in pixels at the project's resolution.
    pub font_size: u32,
    /// `#RRGGBB`.
    pub color_hex: String,
    pub position: TitlePosition,
    #[serde(default, skip_serializing_if = "is_zero_i")]
    pub fade_in_frames: i64,
    #[serde(default, skip_serializing_if = "is_zero_i")]
    pub fade_out_frames: i64,
    /// Free placement: the text block's centre at `position_*`, uniform scale
    /// (`scale_x`) and rotation. None keeps the `position` preset layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<Transform>,
    #[serde(default, skip_serializing_if = "TextStyle::is_default")]
    pub style: TextStyle,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Fonts a title can use: (family, regular file, bold file) in the Windows fonts folder.
pub const TITLE_FONTS: &[(&str, &str, &str)] = &[
    ("Segoe UI", "segoeui.ttf", "segoeuib.ttf"),
    ("Arial", "arial.ttf", "arialbd.ttf"),
    ("Calibri", "calibri.ttf", "calibrib.ttf"),
    ("Georgia", "georgia.ttf", "georgiab.ttf"),
    ("Times New Roman", "times.ttf", "timesbd.ttf"),
    ("Verdana", "verdana.ttf", "verdanab.ttf"),
    ("Tahoma", "tahoma.ttf", "tahomabd.ttf"),
    ("Trebuchet MS", "trebuc.ttf", "trebucbd.ttf"),
    ("Consolas", "consola.ttf", "consolab.ttf"),
    ("Courier New", "cour.ttf", "courbd.ttf"),
    ("Impact", "impact.ttf", "impact.ttf"),
];

/// Typography of a title. The defaults reproduce the original title look.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    /// A family from [`TITLE_FONTS`]; empty uses the default title font.
    #[serde(default)]
    pub font: String,
    #[serde(default = "yes")]
    pub bold: bool,
    #[serde(default)]
    pub align: TextAlign,
    /// Gap between lines as a fraction of the font size.
    #[serde(default = "line_gap")]
    pub line_spacing: f64,
    /// Outline width in pixels at the project resolution (0 = none).
    #[serde(default)]
    pub outline_px: u32,
    #[serde(default = "black_hex")]
    pub outline_hex: String,
    #[serde(default = "yes")]
    pub shadow: bool,
    /// Box behind the text as `#RRGGBBAA`; None = no box.
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default = "one")]
    pub opacity: f64,
}

fn line_gap() -> f64 {
    0.2
}
fn black_hex() -> String {
    "#000000".into()
}

impl Default for TextStyle {
    fn default() -> Self {
        Self { font: String::new(), bold: true, align: TextAlign::Left, line_spacing: 0.2, outline_px: 0, outline_hex: black_hex(), shadow: true, background: None, opacity: 1.0 }
    }
}

impl TextStyle {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl TitleClip {
    pub fn end(&self) -> i64 {
        self.start + self.duration
    }
    pub fn label(&self) -> String {
        let first = self.text.lines().next().unwrap_or("").trim();
        let short: String = first.chars().take(32).collect();
        if short.is_empty() {
            "Title".into()
        } else {
            format!("Title “{short}”")
        }
    }
}

pub fn valid_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Primary color correction. Ranges are enforced by [`ColorParams::validate`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColorParams {
    /// Shadow offset, -0.5..=0.5 (0 = neutral).
    pub lift: f64,
    /// Midtone gamma, 0.2..=4.0 (1 = neutral).
    pub gamma: f64,
    /// Highlight gain, 0.25..=4.0 (1 = neutral).
    pub gain: f64,
    /// Contrast, 0.0..=2.0 (1 = neutral).
    pub contrast: f64,
    /// Saturation, 0.0..=3.0 (1 = neutral).
    pub saturation: f64,
}

impl Default for ColorParams {
    fn default() -> Self {
        Self { lift: 0.0, gamma: 1.0, gain: 1.0, contrast: 1.0, saturation: 1.0 }
    }
}

pub const COLOR_RANGES: [(&str, f64, f64); 5] = [
    ("lift", -0.5, 0.5),
    ("gamma", 0.2, 4.0),
    ("gain", 0.25, 4.0),
    ("contrast", 0.0, 2.0),
    ("saturation", 0.0, 3.0),
];

impl ColorParams {
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    pub fn fields(&self) -> [(&'static str, f64); 5] {
        [
            ("lift", self.lift),
            ("gamma", self.gamma),
            ("gain", self.gain),
            ("contrast", self.contrast),
            ("saturation", self.saturation),
        ]
    }

    pub fn validate(&self) -> CoreResult<()> {
        for ((name, v), (_, lo, hi)) in self.fields().iter().zip(COLOR_RANGES.iter()) {
            if !v.is_finite() || *v < *lo || *v > *hi {
                return Err(CoreError::Invalid(format!("{name} = {v} is outside {lo}..={hi}")));
            }
        }
        Ok(())
    }
}

pub const GAIN_DB_RANGE: (f64, f64) = (-60.0, 12.0);

impl Project {
    pub fn new(name: &str, settings: ProjectSettings) -> Self {
        let mut p = Self {
            format: PROJECT_FORMAT.into(),
            schema_version: SCHEMA_VERSION,
            id: Uuid::new_v4(),
            name: name.into(),
            settings,
            media: vec![],
            tracks: vec![],
            markers: vec![],
            mark_in: None,
            mark_out: None,
            extra: Map::new(),
        };
        p.tracks.push(Track::new(TrackKind::Video, "V1"));
        p.tracks.push(Track::new(TrackKind::Video, "V2"));
        p.tracks.push(Track::new(TrackKind::Audio, "A1"));
        p.tracks.push(Track::new(TrackKind::Audio, "A2"));
        p
    }

    /// A new project as the app creates it: only V1 and A1. Titles and linked
    /// sync audio add further tracks when they need room.
    pub fn new_minimal(name: &str, settings: ProjectSettings) -> Self {
        let mut p = Self::new(name, settings);
        p.tracks.retain(|t| t.name == "V1" || t.name == "A1");
        p
    }

    pub fn media(&self, id: Uuid) -> Option<&MediaRef> {
        self.media.iter().find(|m| m.id == id)
    }

    pub fn track(&self, id: Uuid) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_mut(&mut self, id: Uuid) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// Locate a clip: (track index, clip index).
    pub fn find_clip(&self, clip_id: Uuid) -> Option<(usize, usize)> {
        for (ti, t) in self.tracks.iter().enumerate() {
            if let Some(ci) = t.clips.iter().position(|c| c.id == clip_id) {
                return Some((ti, ci));
            }
        }
        None
    }

    pub fn clip(&self, clip_id: Uuid) -> Option<(&Track, &Clip)> {
        let (ti, ci) = self.find_clip(clip_id)?;
        Some((&self.tracks[ti], &self.tracks[ti].clips[ci]))
    }

    /// Timeline length in frames: end of the last clip or title on any track.
    pub fn duration(&self) -> i64 {
        let clips = self.tracks.iter().flat_map(|t| t.clips.iter()).map(|c| c.end());
        let titles = self.tracks.iter().flat_map(|t| t.titles.iter()).map(|t| t.end());
        clips.chain(titles).max().unwrap_or(0)
    }

    pub fn sort_clips(&mut self) {
        for t in &mut self.tracks {
            t.clips.sort_by_key(|c| (c.start, c.id));
            t.titles.sort_by_key(|c| (c.start, c.id));
        }
        self.markers.sort_by_key(|m| (m.frame, m.id));
    }

    /// Locate a title: (track index, title index).
    pub fn find_title(&self, id: Uuid) -> Option<(usize, usize)> {
        for (ti, t) in self.tracks.iter().enumerate() {
            if let Some(i) = t.titles.iter().position(|c| c.id == id) {
                return Some((ti, i));
            }
        }
        None
    }

    /// The clip that ends exactly where `clip_id` starts on the same track.
    pub fn prev_adjacent(&self, clip_id: Uuid) -> Option<&Clip> {
        let (t, c) = self.clip(clip_id)?;
        t.clips.iter().find(|o| o.id != c.id && o.end() == c.start)
    }

    /// Drop transitions whose outgoing neighbour no longer touches the clip
    /// (after a move, delete or trim opened a gap). Returns how many were dropped.
    pub fn prune_dangling_transitions(&mut self) -> usize {
        let mut n = 0;
        for t in &mut self.tracks {
            let ends: Vec<i64> = t.clips.iter().map(|c| c.end()).collect();
            for c in &mut t.clips {
                if c.transition_in.is_some() && !ends.contains(&c.start) {
                    c.transition_in = None;
                    n += 1;
                }
            }
        }
        n
    }

    /// Full structural validation. Called on load, after every edit, and
    /// before committing or merging.
    pub fn validate(&self) -> CoreResult<()> {
        if self.format != PROJECT_FORMAT {
            return Err(CoreError::Format(format!("unexpected format tag {:?}", self.format)));
        }
        if self.schema_version > SCHEMA_VERSION {
            return Err(CoreError::Format(format!(
                "project schema v{} is newer than this build supports (v{SCHEMA_VERSION})",
                self.schema_version
            )));
        }
        let s = &self.settings;
        s.frame_rate.validate()?;
        if !(16..=8192).contains(&s.width) || !(16..=8192).contains(&s.height) || s.width % 2 != 0 || s.height % 2 != 0 {
            return Err(CoreError::Invalid(format!("resolution {}x{} must be even and within 16–8192", s.width, s.height)));
        }
        if !(8_000..=192_000).contains(&s.sample_rate) {
            return Err(CoreError::Invalid(format!("sample rate {} is outside 8000–192000", s.sample_rate)));
        }

        let mut ids: HashSet<Uuid> = HashSet::new();
        let mut media_by_id: HashMap<Uuid, &MediaRef> = HashMap::new();
        for m in &self.media {
            if !ids.insert(m.id) {
                return Err(CoreError::Invalid(format!("duplicate id {}", m.id)));
            }
            media_by_id.insert(m.id, m);
        }
        for t in &self.tracks {
            if !ids.insert(t.id) {
                return Err(CoreError::Invalid(format!("duplicate id {}", t.id)));
            }
            let mut prev_end: Option<(i64, &str)> = None;
            let mut sorted: Vec<&Clip> = t.clips.iter().collect();
            sorted.sort_by_key(|c| c.start);
            for c in sorted {
                if !ids.insert(c.id) {
                    return Err(CoreError::Invalid(format!("duplicate id {}", c.id)));
                }
                let m = media_by_id
                    .get(&c.media_id)
                    .ok_or_else(|| CoreError::Invalid(format!("clip {:?} references unknown media {}", c.name, c.media_id)))?;
                validate_clip_against_media(c, m, t.kind)?;
                c.color.validate()?;
                if !c.gain_db.is_finite() || c.gain_db < GAIN_DB_RANGE.0 || c.gain_db > GAIN_DB_RANGE.1 {
                    return Err(CoreError::Invalid(format!("clip {:?} gain {} dB is outside -60..=12", c.name, c.gain_db)));
                }
                if let Some((end, name)) = prev_end {
                    if c.start < end {
                        return Err(CoreError::Conflict(format!(
                            "clips {:?} and {:?} overlap on track {}",
                            name, c.name, t.name
                        )));
                    }
                }
                prev_end = Some((c.end(), &c.name));
                validate_clip_v2(c, t, m, &media_by_id)?;
            }
            validate_mixer(t)?;
            // Titles: video tracks only, never overlapping clips or each other.
            if !t.titles.is_empty() && t.kind != TrackKind::Video {
                return Err(CoreError::Invalid(format!("titles can only sit on video tracks, not {}", t.name)));
            }
            let mut spans: Vec<(i64, i64, String)> = t.clips.iter().map(|c| (c.start, c.end(), c.name.clone())).collect();
            for ti in &t.titles {
                if !ids.insert(ti.id) {
                    return Err(CoreError::Invalid(format!("duplicate id {}", ti.id)));
                }
                validate_title(ti, s.height)?;
                spans.push((ti.start, ti.end(), ti.label()));
            }
            spans.sort_by_key(|x| x.0);
            for w in spans.windows(2) {
                if w[1].0 < w[0].1 {
                    return Err(CoreError::Conflict(format!("{} and {} overlap on track {}", w[0].2, w[1].2, t.name)));
                }
            }
        }
        for mk in &self.markers {
            if !ids.insert(mk.id) {
                return Err(CoreError::Invalid(format!("duplicate id {}", mk.id)));
            }
            if mk.frame < 0 {
                return Err(CoreError::Invalid("a marker cannot sit before frame 0".into()));
            }
            if mk.name.trim().is_empty() || mk.name.len() > 120 || mk.note.len() > 2000 {
                return Err(CoreError::Invalid("marker name must be 1–120 characters (note up to 2000)".into()));
            }
            if !MARKER_COLORS.contains(&mk.color.as_str()) {
                return Err(CoreError::Invalid(format!("marker color {:?} is not one of {}", mk.color, MARKER_COLORS.join(", "))));
            }
        }
        match (self.mark_in, self.mark_out) {
            (Some(a), Some(b)) if a < 0 || b <= a => {
                return Err(CoreError::Invalid(format!("in/out range {a}..{b} is empty or negative")));
            }
            (Some(a), None) | (None, Some(a)) if a < 0 => return Err(CoreError::Invalid("in/out point before frame 0".into())),
            _ => {}
        }
        Ok(())
    }
}

fn validate_clip_v2(c: &Clip, t: &Track, m: &MediaRef, media: &HashMap<Uuid, &MediaRef>) -> CoreResult<()> {
    if !c.speed.is_finite() || c.speed < SPEED_RANGE.0 || c.speed > SPEED_RANGE.1 {
        return Err(CoreError::Invalid(format!("clip {:?} speed {}x is outside 0.25x–4x", c.name, c.speed)));
    }
    if c.reversed && m.kind == MediaKind::Image {
        return Err(CoreError::Invalid(format!("still image {:?} cannot be reversed", c.name)));
    }
    let d = c.duration();
    if c.fade_in_frames < 0 || c.fade_out_frames < 0 || c.fade_in_frames + c.fade_out_frames > d {
        return Err(CoreError::Invalid(format!(
            "clip {:?} fades ({} + {} frames) do not fit its {d} frames",
            c.name, c.fade_in_frames, c.fade_out_frames
        )));
    }
    c.transform.validate()?;
    c.crop.validate()?;
    if !c.opacity.is_finite() || !(0.0..=1.0).contains(&c.opacity) {
        return Err(CoreError::Invalid(format!("clip {:?} opacity must be 0–1", c.name)));
    }
    if c.keyframes.len() > MAX_KEYFRAMES {
        return Err(CoreError::Invalid(format!("clip {:?} has more than {MAX_KEYFRAMES} keyframes", c.name)));
    }
    for (i, k) in c.keyframes.iter().enumerate() {
        k.transform.validate()?;
        if !k.opacity.is_finite() || !(0.0..=1.0).contains(&k.opacity) {
            return Err(CoreError::Invalid(format!("clip {:?} keyframe opacity must be 0–1", c.name)));
        }
        if i > 0 && c.keyframes[i - 1].frame >= k.frame {
            return Err(CoreError::Invalid(format!("clip {:?} keyframes must be in frame order, one per frame", c.name)));
        }
    }
    for fx in &c.vfx {
        if !fx.amount.is_finite() || !(0.0..=1.0).contains(&fx.amount) {
            return Err(CoreError::Invalid(format!("clip {:?}: effect amount must be 0–1", c.name)));
        }
    }
    if c.ofx.len() > 32 {
        return Err(CoreError::Invalid(format!("clip {:?}: at most 32 OpenFX effects", c.name)));
    }
    for fx in &c.ofx {
        if fx.plugin_id.trim().is_empty() || fx.plugin_id.len() > 512 || fx.label.len() > 512 || fx.params.len() > 2048 {
            return Err(CoreError::Invalid(format!("clip {:?}: OpenFX effect {:?} is malformed", c.name, fx.label)));
        }
        for (k, v) in &fx.params {
            let ok = match v {
                Value::Bool(_) => true,
                Value::Number(n) => n.as_f64().is_some_and(f64::is_finite),
                Value::String(s) => s.len() <= 65_536,
                Value::Array(a) => a.len() <= 4 && a.iter().all(|x| x.as_f64().is_some_and(f64::is_finite)),
                _ => false,
            };
            if !ok || k.len() > 512 {
                return Err(CoreError::Invalid(format!("clip {:?}: OpenFX parameter {k:?} of {:?} has an unusable value", c.name, fx.label)));
            }
        }
    }
    if (!c.transform.is_identity() || !c.crop.is_none() || c.opacity != 1.0 || !c.keyframes.is_empty() || !c.vfx.is_empty() || !c.ofx.is_empty()) && t.kind != TrackKind::Video {
        return Err(CoreError::Invalid("transform, crop, opacity and effects apply to video clips only".into()));
    }
    if let Some(lut) = &c.lut {
        if !lut.to_lowercase().ends_with(".cube") || lut.len() > 4096 {
            return Err(CoreError::Invalid(format!("clip {:?}: LUT must be a .cube file", c.name)));
        }
        if t.kind != TrackKind::Video {
            return Err(CoreError::Invalid("LUTs apply to video clips only".into()));
        }
    }
    if let Some(tr) = c.transition_in {
        let prev = t
            .clips
            .iter()
            .find(|o| o.id != c.id && o.end() == c.start)
            .ok_or_else(|| CoreError::Invalid(format!("clip {:?} has a transition but no clip ends where it starts", c.name)))?;
        if tr.duration_frames < 2 || tr.duration_frames > d.min(prev.duration()) {
            return Err(CoreError::Invalid(format!(
                "a {}-frame transition must be at least 2 frames and no longer than either clip ({} / {} frames)",
                tr.duration_frames,
                prev.duration(),
                d
            )));
        }
        if tr.kind.overlaps() {
            let (lead, tail) = dissolve_halves(tr.duration_frames);
            let name = tr.kind.label();
            if c.speed != 1.0 || prev.speed != 1.0 || c.reversed || prev.reversed {
                return Err(CoreError::Invalid(format!("{name} needs both clips at 1x and not reversed; use a dip")));
            }
            if c.source_in < lead {
                return Err(CoreError::Invalid(format!(
                    "{name} needs {lead} frames of handle before {:?} (it has {})",
                    c.name, c.source_in
                )));
            }
            let pm = media.get(&prev.media_id).copied();
            let avail = pm.and_then(|m| m.duration_frames).map(|d| d - prev.source_out);
            if let Some(a) = avail {
                if a < tail {
                    return Err(CoreError::Invalid(format!(
                        "{name} needs {tail} frames of handle after {:?} (it has {a})",
                        prev.name
                    )));
                }
            }
        }
    }
    Ok(())
}

/// A cross dissolve of `d` frames centred on the cut: the incoming clip starts
/// `lead` frames early and the outgoing clip runs `tail` frames late.
pub fn dissolve_halves(d: i64) -> (i64, i64) {
    let lead = d / 2;
    (lead, d - lead)
}

fn validate_mixer(t: &Track) -> CoreResult<()> {
    let in_range = |v: f64, r: (f64, f64)| v.is_finite() && v >= r.0 && v <= r.1;
    if !in_range(t.volume_db, MIX_DB_RANGE) {
        return Err(CoreError::Invalid(format!("track {} volume {} dB is outside −60..+12", t.name, t.volume_db)));
    }
    if !in_range(t.pan, (-1.0, 1.0)) {
        return Err(CoreError::Invalid(format!("track {} pan {} is outside −1..+1", t.name, t.pan)));
    }
    for (n, v) in [("low", t.eq_low_db), ("mid", t.eq_mid_db), ("high", t.eq_high_db)] {
        if !in_range(v, EQ_DB_RANGE) {
            return Err(CoreError::Invalid(format!("track {} EQ {n} {v} dB is outside −18..+18", t.name)));
        }
    }
    Ok(())
}

fn validate_title(t: &TitleClip, height: u32) -> CoreResult<()> {
    if t.start < 0 || t.duration < 1 {
        return Err(CoreError::Invalid("a title needs a start ≥ 0 and at least 1 frame".into()));
    }
    if t.text.trim().is_empty() || t.text.chars().count() > 500 {
        return Err(CoreError::Invalid("title text must be 1–500 characters".into()));
    }
    if t.font_size < 8 || t.font_size > height.max(16) {
        return Err(CoreError::Invalid(format!("title font size {} must be 8–{} px", t.font_size, height)));
    }
    if !valid_hex_color(&t.color_hex) {
        return Err(CoreError::Invalid(format!("title color {:?} must be #RRGGBB", t.color_hex)));
    }
    if t.fade_in_frames < 0 || t.fade_out_frames < 0 || t.fade_in_frames + t.fade_out_frames > t.duration {
        return Err(CoreError::Invalid("title fades do not fit its duration".into()));
    }
    if let Some(tf) = &t.transform {
        tf.validate()?;
        if tf.scale_x != tf.scale_y {
            return Err(CoreError::Invalid("title scale must be uniform".into()));
        }
    }
    let s = &t.style;
    if !s.font.is_empty() && !TITLE_FONTS.iter().any(|f| f.0 == s.font) {
        return Err(CoreError::Invalid(format!("title font {:?} is not available", s.font)));
    }
    if !s.line_spacing.is_finite() || !(0.0..=3.0).contains(&s.line_spacing) {
        return Err(CoreError::Invalid("title line spacing must be 0–3".into()));
    }
    if s.outline_px > height.max(16) / 10 || !valid_hex_color(&s.outline_hex) {
        return Err(CoreError::Invalid("title outline must be a #RRGGBB color up to a tenth of the frame height".into()));
    }
    if let Some(b) = &s.background {
        if b.len() != 9 || !valid_hex_color(&b[..7]) || !b[7..].chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CoreError::Invalid(format!("title background {b:?} must be #RRGGBBAA")));
        }
    }
    if !s.opacity.is_finite() || !(0.0..=1.0).contains(&s.opacity) {
        return Err(CoreError::Invalid("title opacity must be 0–1".into()));
    }
    Ok(())
}

pub fn validate_clip_against_media(c: &Clip, m: &MediaRef, kind: TrackKind) -> CoreResult<()> {
    if c.start < 0 {
        return Err(CoreError::Invalid(format!("clip {:?} starts before frame 0", c.name)));
    }
    if c.source_in < 0 || c.source_out <= c.source_in {
        return Err(CoreError::Invalid(format!(
            "clip {:?} source range {}..{} is empty or negative",
            c.name, c.source_in, c.source_out
        )));
    }
    if let Some(d) = m.duration_frames {
        if c.source_out > d {
            return Err(CoreError::Invalid(format!(
                "clip {:?} source out {} exceeds media length {} frames",
                c.name, c.source_out, d
            )));
        }
    }
    match kind {
        TrackKind::Video if !m.has_video => {
            Err(CoreError::Invalid(format!("{:?} has no video stream; place it on an audio track", m.name)))
        }
        TrackKind::Audio if !m.has_audio => {
            Err(CoreError::Invalid(format!("{:?} has no audio stream; place it on a video track", m.name)))
        }
        _ => Ok(()),
    }
}

impl Track {
    pub fn new(kind: TrackKind, name: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            name: name.into(),
            muted: false,
            locked: false,
            clips: vec![],
            titles: vec![],
            solo: false,
            volume_db: 0.0,
            pan: 0.0,
            eq_low_db: 0.0,
            eq_mid_db: 0.0,
            eq_high_db: 0.0,
            extra: Map::new(),
        }
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub fn media(name: &str, frames: i64, video: bool, audio: bool) -> MediaRef {
        MediaRef {
            id: Uuid::new_v4(),
            name: name.into(),
            path: format!("/media/{name}"),
            sha256: Some("ab".repeat(32)),
            size: 1000,
            modified: Some(0),
            kind: if video { MediaKind::Video } else { MediaKind::Audio },
            duration_frames: Some(frames),
            has_video: video,
            has_audio: audio,
            width: video.then_some(1920),
            height: video.then_some(1080),
            frame_rate: video.then_some(Rational::new(25, 1)),
            video_codec: video.then(|| "h264".into()),
            audio_codec: audio.then(|| "aac".into()),
            sample_rate: audio.then_some(48_000),
            channels: audio.then_some(2),
            container: Some("mp4".into()),
            extra: Map::new(),
        }
    }

    /// Project with one 250-frame A/V media item and nothing on the timeline.
    pub fn project() -> (Project, Uuid) {
        let mut p = Project::new("Test", ProjectSettings::default());
        let m = media("intro.mp4", 250, true, true);
        let id = m.id;
        p.media.push(m);
        (p, id)
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    #[test]
    fn serde_roundtrip_preserves_ids_and_unknown_fields() {
        let (mut p, mid) = project();
        let v1 = p.tracks[0].id;
        p.tracks[0].clips.push(Clip::new(mid, "intro", 0, 10, 60));
        let mut json: Value = serde_json::to_value(&p).unwrap();
        json["future_field"] = Value::from("kept");
        json["tracks"][0]["future_track_field"] = Value::from(42);
        let back: Project = serde_json::from_value(json).unwrap();
        assert_eq!(back.tracks[0].id, v1);
        assert_eq!(back.tracks[0].clips[0].id, p.tracks[0].clips[0].id);
        assert_eq!(back.extra.get("future_field"), Some(&Value::from("kept")));
        let again = serde_json::to_value(&back).unwrap();
        assert_eq!(again["tracks"][0]["future_track_field"], Value::from(42));
        back.validate().unwrap();
    }

    #[test]
    fn validation_rejects_bad_state() {
        let (mut p, mid) = project();
        let mk = |start, si, so| Clip::new(mid, "c", start, si, so);
        p.tracks[0].clips = vec![mk(0, 0, 100), mk(50, 0, 100)];
        assert!(matches!(p.validate(), Err(CoreError::Conflict(_))));
        p.tracks[0].clips = vec![mk(0, 0, 300)];
        assert!(p.validate().is_err(), "out past media end");
        p.tracks[0].clips = vec![mk(0, 50, 50)];
        assert!(p.validate().is_err(), "empty range");
        p.tracks[0].clips = vec![mk(0, 0, 100)];
        p.tracks[0].clips[0].color.saturation = 9.0;
        assert!(p.validate().is_err(), "saturation range");
        p.tracks[0].clips[0].color.saturation = 1.0;
        p.validate().unwrap();
        p.schema_version = 99;
        assert!(matches!(p.validate(), Err(CoreError::Format(_))));
    }

    /// v1 revision objects are content-hashed; adding v2 fields must not
    /// change how a project without v2 features serializes.
    #[test]
    fn v2_defaults_serialize_like_v1() {
        let (mut p, mid) = project();
        p.tracks[0].clips.push(Clip::new(mid, "intro", 0, 0, 50));
        let json = serde_json::to_string(&p).unwrap();
        for key in ["fade_in_frames", "transition_in", "speed", "reversed", "lut", "titles", "markers", "mark_in", "pan", "solo", "eq_low_db", "volume_db"] {
            assert!(!json.contains(&format!("\"{key}\"")), "{key} leaked into a v1-shaped project");
        }
        // And a v1 document (no v2 keys) loads with neutral v2 values.
        let back: Project = serde_json::from_str(&json).unwrap();
        let c = &back.tracks[0].clips[0];
        assert_eq!((c.speed, c.reversed, c.fade_in_frames, c.transition_in), (1.0, false, 0, None));
    }

    #[test]
    fn layer_and_text_fields_stay_out_of_old_files() {
        let (_, mid) = project();
        let c = Clip::new(mid, "x", 0, 0, 10);
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("opacity").is_none() && v.get("keyframes").is_none());
        let old = serde_json::json!({ "id": Uuid::new_v4(), "start": 0, "duration": 10, "text": "Hi", "font_size": 64, "color_hex": "#ffffff", "position": "center" });
        let t: TitleClip = serde_json::from_value(old.clone()).unwrap();
        assert!(t.transform.is_none() && t.style.is_default());
        assert_eq!(serde_json::to_value(&t).unwrap(), old, "an unedited title saves exactly as before");
        let styled = TitleClip {
            transform: Some(Transform { position_x: 0.25, rotation_deg: -10.0, ..Default::default() }),
            style: TextStyle { font: "Georgia".into(), bold: false, align: TextAlign::Right, outline_px: 3, background: Some("#11223380".into()), opacity: 0.7, ..Default::default() },
            ..t
        };
        let back: TitleClip = serde_json::from_value(serde_json::to_value(&styled).unwrap()).unwrap();
        assert_eq!(back, styled);
    }

    #[test]
    fn speed_changes_timeline_duration() {
        let (_, mid) = project();
        let mut c = Clip::new(mid, "c", 10, 0, 100);
        c.speed = 2.0;
        assert_eq!((c.duration(), c.end()), (50, 60));
        c.speed = 0.25;
        assert_eq!(c.duration(), 400);
        c.speed = 3.0;
        assert_eq!(c.duration(), 33); // 33.33 rounds down
    }

    #[test]
    fn v2_validation() {
        let (mut p, mid) = project();
        let mut a = Clip::new(mid, "a", 0, 0, 100);
        let mut b = Clip::new(mid, "b", 100, 100, 200);
        a.fade_in_frames = 60;
        a.fade_out_frames = 50;
        p.tracks[0].clips = vec![a.clone(), b.clone()];
        assert!(p.validate().is_err(), "fades longer than the clip");
        a.fade_in_frames = 10;
        a.fade_out_frames = 0;
        b.transition_in = Some(Transition { kind: TransitionKind::CrossDissolve, duration_frames: 20 });
        p.tracks[0].clips = vec![a.clone(), b.clone()];
        p.validate().unwrap();
        b.source_in = 5;
        b.source_out = 105;
        p.tracks[0].clips = vec![a.clone(), b.clone()];
        assert!(p.validate().is_err(), "not enough handle before the incoming clip");
        b.transition_in = Some(Transition { kind: TransitionKind::DipToBlack, duration_frames: 20 });
        p.tracks[0].clips = vec![a.clone(), b.clone()];
        p.validate().unwrap();
        b.speed = 9.0;
        p.tracks[0].clips = vec![a.clone(), b.clone()];
        assert!(p.validate().is_err(), "speed range");
        p.tracks[0].clips = vec![a];
        p.tracks[2].pan = 1.5;
        assert!(p.validate().is_err(), "pan range");
        p.tracks[2].pan = 0.5;
        p.tracks[2].titles.push(TitleClip {
            id: Uuid::new_v4(),
            start: 0,
            duration: 50,
            text: "Hi".into(),
            font_size: 64,
            color_hex: "#ffffff".into(),
            position: TitlePosition::Center,
            fade_in_frames: 0,
            fade_out_frames: 0,
            transform: None,
            style: TextStyle::default(),
            extra: Map::new(),
        });
        assert!(p.validate().is_err(), "titles not allowed on audio tracks");
        let t = p.tracks[2].titles.pop().unwrap();
        p.tracks[1].titles.push(TitleClip { color_hex: "white".into(), ..t.clone() });
        assert!(p.validate().is_err(), "bad colour");
        p.tracks[1].titles = vec![t.clone()];
        p.validate().unwrap();
        p.tracks[1].clips.push(Clip::new(mid, "over", 10, 0, 20));
        assert!(matches!(p.validate(), Err(CoreError::Conflict(_))), "title overlaps a clip");
        p.tracks[1].clips.clear();
        p.markers.push(Marker { id: Uuid::new_v4(), frame: 5, name: "Beat".into(), color: "orange".into(), note: String::new(), extra: Map::new() });
        assert!(p.validate().is_err(), "unknown marker colour");
        p.markers[0].color = "red".into();
        p.mark_in = Some(50);
        p.mark_out = Some(40);
        assert!(p.validate().is_err(), "empty in/out range");
        p.mark_out = Some(80);
        p.validate().unwrap();
    }
}

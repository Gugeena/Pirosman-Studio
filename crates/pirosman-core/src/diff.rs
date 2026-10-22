//! Semantic project diffs keyed by stable ids.
//!
//! Objects are matched by UUID, never by array position or text lines. Each
//! change carries before/after values, and unknown (`extra`) fields are
//! compared too, so data this build does not understand is never silently
//! dropped from a comparison.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;

use crate::model::{Clip, Project, TitleClip, Track, Transition, TransitionKind, VfxKind};
use crate::time::timecode;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Change {
    ClipAdded { clip_id: Uuid, name: String, track_id: Uuid, track: String, start: i64, source_in: i64, source_out: i64 },
    ClipRemoved { clip_id: Uuid, name: String, track_id: Uuid, track: String, start: i64 },
    ClipTrimmed { clip_id: Uuid, name: String, before_in: i64, before_out: i64, after_in: i64, after_out: i64 },
    ClipMoved { clip_id: Uuid, name: String, track_id: Uuid, track: String, before_start: i64, after_start: i64 },
    ClipTrackChanged { clip_id: Uuid, name: String, before_track_id: Uuid, before_track: String, after_track_id: Uuid, after_track: String, before_start: i64, after_start: i64 },
    ClipNameChanged { clip_id: Uuid, before: String, after: String },
    ColorOrEffectModified { clip_id: Uuid, name: String, param: String, before: f64, after: f64 },
    AudioGainModified { clip_id: Uuid, name: String, before_db: f64, after_db: f64 },
    ClipMediaChanged { clip_id: Uuid, name: String, before_media: Uuid, after_media: Uuid },
    TrackAdded { track_id: Uuid, name: String, kind: String },
    TrackRemoved { track_id: Uuid, name: String },
    TrackModified { track_id: Uuid, name: String, field: String, before: Value, after: Value },
    TrackReordered { before: Vec<String>, after: Vec<String> },
    ProjectSettingsModified { field: String, before: Value, after: Value },
    MediaReferenceChanged { media_id: Uuid, name: String, field: String, before: Value, after: Value },
    ExtraFieldModified { object: String, object_id: String, key: String, before: Value, after: Value },
    // ---- v2
    ClipSlipped { clip_id: Uuid, name: String, track: String, delta: i64, before_in: i64, after_in: i64 },
    ClipFadeModified { clip_id: Uuid, name: String, track: String, edge: String, before: i64, after: i64 },
    TransitionModified { clip_id: Uuid, name: String, track: String, before: Option<Transition>, after: Option<Transition> },
    ClipSpeedModified { clip_id: Uuid, name: String, track: String, before_speed: f64, after_speed: f64, before_reversed: bool, after_reversed: bool },
    LutModified { clip_id: Uuid, name: String, before: Option<String>, after: Option<String> },
    TitleAdded { title_id: Uuid, label: String, track: String, start: i64 },
    TitleRemoved { title_id: Uuid, label: String, track: String, start: i64 },
    TitleMoved { title_id: Uuid, label: String, before_track: String, after_track: String, before_start: i64, after_start: i64 },
    TitleModified { title_id: Uuid, label: String, field: String, before: Value, after: Value },
    MarkerAdded { marker_id: Uuid, name: String, frame: i64 },
    MarkerRemoved { marker_id: Uuid, name: String, frame: i64 },
    MarkerModified { marker_id: Uuid, name: String, field: String, before: Value, after: Value },
    // ---- v3
    ClipTransformModified { clip_id: Uuid, name: String, track: String, field: String, before: f64, after: f64 },
    VfxModified { clip_id: Uuid, name: String, track: String, effect: String, field: String, before: Value, after: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffEntry {
    #[serde(flatten)]
    pub change: Change,
    /// Human-readable description built from the actual values.
    pub summary: String,
    /// Clip id this entry concerns, for highlighting on the timeline.
    pub clip_id: Option<Uuid>,
}

fn clip_index(p: &Project) -> HashMap<Uuid, (&Track, &Clip)> {
    p.tracks.iter().flat_map(|t| t.clips.iter().map(move |c| (c.id, (t, c)))).collect()
}

fn diff_extra(object: &str, id: &str, a: &Map<String, Value>, b: &Map<String, Value>, out: &mut Vec<Change>) {
    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    for k in keys {
        let (x, y) = (a.get(k).cloned().unwrap_or(Value::Null), b.get(k).cloned().unwrap_or(Value::Null));
        if x != y {
            out.push(Change::ExtraFieldModified { object: object.into(), object_id: id.into(), key: k.clone(), before: x, after: y });
        }
    }
}

/// Compute typed changes from `a` (before) to `b` (after).
pub fn diff(a: &Project, b: &Project) -> Vec<Change> {
    let mut out = vec![];

    // Project settings.
    let (sa, sb) = (&a.settings, &b.settings);
    let mut setting = |f: &str, x: Value, y: Value| {
        if x != y {
            out.push(Change::ProjectSettingsModified { field: f.into(), before: x, after: y });
        }
    };
    setting("name", a.name.clone().into(), b.name.clone().into());
    setting("resolution", format!("{}x{}", sa.width, sa.height).into(), format!("{}x{}", sb.width, sb.height).into());
    setting("frame_rate", sa.frame_rate.ffmpeg().into(), sb.frame_rate.ffmpeg().into());
    setting("sample_rate", sa.sample_rate.into(), sb.sample_rate.into());
    let range = |p: &Project| -> Value {
        match (p.mark_in, p.mark_out) {
            (None, None) => Value::Null,
            (i, o) => format!("{}..{}", i.map(|v| v.to_string()).unwrap_or_default(), o.map(|v| v.to_string()).unwrap_or_default()).into(),
        }
    };
    setting("in/out range", range(a), range(b));
    diff_extra("settings", "settings", &sa.extra, &sb.extra, &mut out);
    diff_extra("project", &a.id.to_string(), &a.extra, &b.extra, &mut out);

    // Media references.
    let ma: HashMap<Uuid, _> = a.media.iter().map(|m| (m.id, m)).collect();
    let mb: HashMap<Uuid, _> = b.media.iter().map(|m| (m.id, m)).collect();
    for m in &b.media {
        match ma.get(&m.id) {
            None => out.push(Change::MediaReferenceChanged { media_id: m.id, name: m.name.clone(), field: "added".into(), before: Value::Null, after: m.path.clone().into() }),
            Some(old) => {
                let fields: [(&str, Value, Value); 3] = [
                    ("path", old.path.clone().into(), m.path.clone().into()),
                    ("sha256", old.sha256.clone().into(), m.sha256.clone().into()),
                    ("name", old.name.clone().into(), m.name.clone().into()),
                ];
                for (f, x, y) in fields {
                    if x != y {
                        out.push(Change::MediaReferenceChanged { media_id: m.id, name: m.name.clone(), field: f.into(), before: x, after: y });
                    }
                }
                diff_extra("media", &m.id.to_string(), &old.extra, &m.extra, &mut out);
            }
        }
    }
    for m in &a.media {
        if !mb.contains_key(&m.id) {
            out.push(Change::MediaReferenceChanged { media_id: m.id, name: m.name.clone(), field: "removed".into(), before: m.path.clone().into(), after: Value::Null });
        }
    }

    // Tracks.
    let ta: HashMap<Uuid, &Track> = a.tracks.iter().map(|t| (t.id, t)).collect();
    let tb: HashMap<Uuid, &Track> = b.tracks.iter().map(|t| (t.id, t)).collect();
    for t in &b.tracks {
        match ta.get(&t.id) {
            None => out.push(Change::TrackAdded { track_id: t.id, name: t.name.clone(), kind: format!("{:?}", t.kind).to_lowercase() }),
            Some(old) => {
                for (f, x, y) in [
                    ("name", Value::from(old.name.clone()), Value::from(t.name.clone())),
                    ("muted", old.muted.into(), t.muted.into()),
                    ("locked", old.locked.into(), t.locked.into()),
                    ("solo", old.solo.into(), t.solo.into()),
                    ("volume_db", old.volume_db.into(), t.volume_db.into()),
                    ("pan", old.pan.into(), t.pan.into()),
                    ("eq_low_db", old.eq_low_db.into(), t.eq_low_db.into()),
                    ("eq_mid_db", old.eq_mid_db.into(), t.eq_mid_db.into()),
                    ("eq_high_db", old.eq_high_db.into(), t.eq_high_db.into()),
                ] {
                    if x != y {
                        out.push(Change::TrackModified { track_id: t.id, name: t.name.clone(), field: f.into(), before: x, after: y });
                    }
                }
                diff_extra("track", &t.id.to_string(), &old.extra, &t.extra, &mut out);
            }
        }
    }
    for t in &a.tracks {
        if !tb.contains_key(&t.id) {
            out.push(Change::TrackRemoved { track_id: t.id, name: t.name.clone() });
        }
    }
    // Order of tracks that exist on both sides.
    let oa: Vec<&Track> = a.tracks.iter().filter(|t| tb.contains_key(&t.id)).collect();
    let ob: Vec<&Track> = b.tracks.iter().filter(|t| ta.contains_key(&t.id)).collect();
    if oa.iter().map(|t| t.id).ne(ob.iter().map(|t| t.id)) {
        out.push(Change::TrackReordered { before: oa.iter().map(|t| t.name.clone()).collect(), after: ob.iter().map(|t| t.name.clone()).collect() });
    }

    // Clips.
    let ca = clip_index(a);
    let cb = clip_index(b);
    for t in &b.tracks {
        for c in &t.clips {
            match ca.get(&c.id) {
                None => out.push(Change::ClipAdded {
                    clip_id: c.id,
                    name: c.name.clone(),
                    track_id: t.id,
                    track: t.name.clone(),
                    start: c.start,
                    source_in: c.source_in,
                    source_out: c.source_out,
                }),
                Some((ot, oc)) => diff_clip(ot, oc, t, c, &mut out),
            }
        }
    }
    for t in &a.tracks {
        for c in &t.clips {
            if !cb.contains_key(&c.id) {
                out.push(Change::ClipRemoved { clip_id: c.id, name: c.name.clone(), track_id: t.id, track: t.name.clone(), start: c.start });
            }
        }
    }
    diff_titles(a, b, &mut out);
    diff_markers(a, b, &mut out);
    out
}

fn title_index(p: &Project) -> HashMap<Uuid, (&Track, &TitleClip)> {
    p.tracks.iter().flat_map(|t| t.titles.iter().map(move |x| (x.id, (t, x)))).collect()
}

fn diff_titles(a: &Project, b: &Project, out: &mut Vec<Change>) {
    let xa = title_index(a);
    let xb = title_index(b);
    for t in &b.tracks {
        for x in &t.titles {
            match xa.get(&x.id) {
                None => out.push(Change::TitleAdded { title_id: x.id, label: x.label(), track: t.name.clone(), start: x.start }),
                Some((ot, ox)) => {
                    if ot.id != t.id || ox.start != x.start {
                        out.push(Change::TitleMoved {
                            title_id: x.id,
                            label: x.label(),
                            before_track: ot.name.clone(),
                            after_track: t.name.clone(),
                            before_start: ox.start,
                            after_start: x.start,
                        });
                    }
                    for (f, before, after) in [
                        ("text", Value::from(ox.text.clone()), Value::from(x.text.clone())),
                        ("duration", ox.duration.into(), x.duration.into()),
                        ("font_size", ox.font_size.into(), x.font_size.into()),
                        ("color", ox.color_hex.clone().into(), x.color_hex.clone().into()),
                        ("position", serde_json::to_value(ox.position).unwrap(), serde_json::to_value(x.position).unwrap()),
                        ("fade_in_frames", ox.fade_in_frames.into(), x.fade_in_frames.into()),
                        ("fade_out_frames", ox.fade_out_frames.into(), x.fade_out_frames.into()),
                    ] {
                        if before != after {
                            out.push(Change::TitleModified { title_id: x.id, label: x.label(), field: f.into(), before, after });
                        }
                    }
                    diff_extra("title", &x.id.to_string(), &ox.extra, &x.extra, out);
                }
            }
        }
    }
    for t in &a.tracks {
        for x in &t.titles {
            if !xb.contains_key(&x.id) {
                out.push(Change::TitleRemoved { title_id: x.id, label: x.label(), track: t.name.clone(), start: x.start });
            }
        }
    }
}

fn diff_markers(a: &Project, b: &Project, out: &mut Vec<Change>) {
    let ma: HashMap<Uuid, _> = a.markers.iter().map(|m| (m.id, m)).collect();
    let mb: HashMap<Uuid, _> = b.markers.iter().map(|m| (m.id, m)).collect();
    for m in &b.markers {
        match ma.get(&m.id) {
            None => out.push(Change::MarkerAdded { marker_id: m.id, name: m.name.clone(), frame: m.frame }),
            Some(o) => {
                for (f, before, after) in [
                    ("frame", Value::from(o.frame), Value::from(m.frame)),
                    ("name", o.name.clone().into(), m.name.clone().into()),
                    ("color", o.color.clone().into(), m.color.clone().into()),
                    ("note", o.note.clone().into(), m.note.clone().into()),
                ] {
                    if before != after {
                        out.push(Change::MarkerModified { marker_id: m.id, name: m.name.clone(), field: f.into(), before, after });
                    }
                }
                diff_extra("marker", &m.id.to_string(), &o.extra, &m.extra, out);
            }
        }
    }
    for m in &a.markers {
        if !mb.contains_key(&m.id) {
            out.push(Change::MarkerRemoved { marker_id: m.id, name: m.name.clone(), frame: m.frame });
        }
    }
}

fn diff_clip(ot: &Track, oc: &Clip, t: &Track, c: &Clip, out: &mut Vec<Change>) {
    if ot.id != t.id {
        out.push(Change::ClipTrackChanged {
            clip_id: c.id,
            name: c.name.clone(),
            before_track_id: ot.id,
            before_track: ot.name.clone(),
            after_track_id: t.id,
            after_track: t.name.clone(),
            before_start: oc.start,
            after_start: c.start,
        });
    }
    let din = c.source_in - oc.source_in;
    let slipped = din != 0 && din == c.source_out - oc.source_out && oc.start == c.start;
    // A move shifts where source frame 0 would sit on the timeline (the
    // origin); trims keep it (to within a frame at non-1x speeds).
    let origin = |c: &Clip| if c.reversed { c.start as f64 + c.source_out as f64 / c.speed } else { c.start as f64 - c.source_in as f64 / c.speed };
    let retimed = oc.speed != c.speed || oc.reversed != c.reversed;
    let moved = ot.id == t.id && oc.start != c.start && !slipped && (retimed || (origin(oc) - origin(c)).abs() >= 1.0 - 1e-9);
    if moved {
        out.push(Change::ClipMoved { clip_id: c.id, name: c.name.clone(), track_id: t.id, track: t.name.clone(), before_start: oc.start, after_start: c.start });
    }
    if slipped {
        out.push(Change::ClipSlipped { clip_id: c.id, name: c.name.clone(), track: t.name.clone(), delta: din, before_in: oc.source_in, after_in: c.source_in });
    } else if oc.source_in != c.source_in || oc.source_out != c.source_out {
        out.push(Change::ClipTrimmed {
            clip_id: c.id,
            name: c.name.clone(),
            before_in: oc.source_in,
            before_out: oc.source_out,
            after_in: c.source_in,
            after_out: c.source_out,
        });
    }
    if oc.name != c.name {
        out.push(Change::ClipNameChanged { clip_id: c.id, before: oc.name.clone(), after: c.name.clone() });
    }
    if oc.media_id != c.media_id {
        out.push(Change::ClipMediaChanged { clip_id: c.id, name: c.name.clone(), before_media: oc.media_id, after_media: c.media_id });
    }
    for ((k, x), (_, y)) in oc.color.fields().iter().zip(c.color.fields().iter()) {
        if x != y {
            out.push(Change::ColorOrEffectModified { clip_id: c.id, name: c.name.clone(), param: (*k).into(), before: *x, after: *y });
        }
    }
    if oc.gain_db != c.gain_db {
        out.push(Change::AudioGainModified { clip_id: c.id, name: c.name.clone(), before_db: oc.gain_db, after_db: c.gain_db });
    }
    for (edge, x, y) in [("in", oc.fade_in_frames, c.fade_in_frames), ("out", oc.fade_out_frames, c.fade_out_frames)] {
        if x != y {
            out.push(Change::ClipFadeModified { clip_id: c.id, name: c.name.clone(), track: t.name.clone(), edge: edge.into(), before: x, after: y });
        }
    }
    if oc.transition_in != c.transition_in {
        out.push(Change::TransitionModified { clip_id: c.id, name: c.name.clone(), track: t.name.clone(), before: oc.transition_in, after: c.transition_in });
    }
    if oc.speed != c.speed || oc.reversed != c.reversed {
        out.push(Change::ClipSpeedModified {
            clip_id: c.id,
            name: c.name.clone(),
            track: t.name.clone(),
            before_speed: oc.speed,
            after_speed: c.speed,
            before_reversed: oc.reversed,
            after_reversed: c.reversed,
        });
    }
    let tf = |c: &Clip| -> Vec<(&'static str, f64)> {
        let (t, k) = (c.transform, c.crop);
        vec![
            ("scale_x", t.scale_x),
            ("scale_y", t.scale_y),
            ("position_x", t.position_x),
            ("position_y", t.position_y),
            ("rotation_deg", t.rotation_deg),
            ("crop_left", k.left),
            ("crop_right", k.right),
            ("crop_top", k.top),
            ("crop_bottom", k.bottom),
        ]
    };
    for ((field, x), (_, y)) in tf(oc).into_iter().zip(tf(c)) {
        if x != y {
            out.push(Change::ClipTransformModified { clip_id: c.id, name: c.name.clone(), track: t.name.clone(), field: field.into(), before: x, after: y });
        }
    }
    diff_vfx(oc, c, &t.name, out);
    diff_ofx(oc, c, &t.name, out);
    if oc.lut != c.lut {
        out.push(Change::LutModified { clip_id: c.id, name: c.name.clone(), before: oc.lut.clone(), after: c.lut.clone() });
    }
    if oc.link_id != c.link_id {
        out.push(Change::ExtraFieldModified {
            object: "clip".into(),
            object_id: c.id.to_string(),
            key: "link_id".into(),
            before: serde_json::to_value(oc.link_id).unwrap_or_default(),
            after: serde_json::to_value(c.link_id).unwrap_or_default(),
        });
    }
    diff_extra("clip", &c.id.to_string(), &oc.extra, &c.extra, out);
}

fn vfx_label(k: VfxKind) -> &'static str {
    match k {
        VfxKind::GaussianBlur => "Gaussian blur",
        VfxKind::Glow => "Glow",
        VfxKind::Vignette => "Vignette",
        VfxKind::FilmGrain => "Film grain",
        VfxKind::ChromaticAberration => "Chromatic aberration",
    }
}

fn diff_vfx(oc: &Clip, c: &Clip, track: &str, out: &mut Vec<Change>) {
    let push = |out: &mut Vec<Change>, kind: VfxKind, field: &str, before: Value, after: Value| {
        out.push(Change::VfxModified { clip_id: c.id, name: c.name.clone(), track: track.into(), effect: vfx_label(kind).into(), field: field.into(), before, after });
    };
    for fx in &c.vfx {
        match oc.vfx.iter().find(|o| o.id == fx.id) {
            None => push(out, fx.kind, "added", Value::Null, fx.amount.into()),
            Some(o) => {
                if o.amount != fx.amount {
                    push(out, fx.kind, "amount", o.amount.into(), fx.amount.into());
                }
                if o.enabled != fx.enabled {
                    push(out, fx.kind, "enabled", o.enabled.into(), fx.enabled.into());
                }
            }
        }
    }
    for o in &oc.vfx {
        if !c.vfx.iter().any(|f| f.id == o.id) {
            push(out, o.kind, "removed", o.amount.into(), Value::Null);
        }
    }
    let ids = |v: &Vec<crate::model::Vfx>| v.iter().map(|f| f.id).collect::<Vec<_>>();
    let (a, b): (Vec<_>, Vec<_>) = (ids(&oc.vfx).into_iter().filter(|i| c.vfx.iter().any(|f| f.id == *i)).collect(), ids(&c.vfx).into_iter().filter(|i| oc.vfx.iter().any(|f| f.id == *i)).collect());
    if a != b {
        push(out, c.vfx[0].kind, "order", Value::Null, Value::Null);
    }
}

/// OpenFX effects reuse the effect change kind; parameters are named fields.
fn diff_ofx(oc: &Clip, c: &Clip, track: &str, out: &mut Vec<Change>) {
    let push = |out: &mut Vec<Change>, label: &str, field: &str, before: Value, after: Value| {
        out.push(Change::VfxModified { clip_id: c.id, name: c.name.clone(), track: track.into(), effect: format!("OpenFX {label}"), field: field.into(), before, after });
    };
    for fx in &c.ofx {
        match oc.ofx.iter().find(|o| o.id == fx.id) {
            None => push(out, &fx.label, "added", Value::Null, Value::Null),
            Some(o) => {
                if o.enabled != fx.enabled {
                    push(out, &fx.label, "enabled", o.enabled.into(), fx.enabled.into());
                }
                let mut keys: Vec<&String> = o.params.keys().chain(fx.params.keys()).collect();
                keys.sort();
                keys.dedup();
                for k in keys {
                    let (b, a) = (o.params.get(k).cloned().unwrap_or(Value::Null), fx.params.get(k).cloned().unwrap_or(Value::Null));
                    if a != b {
                        push(out, &fx.label, k, b, a);
                    }
                }
            }
        }
    }
    for o in &oc.ofx {
        if !c.ofx.iter().any(|f| f.id == o.id) {
            push(out, &o.label, "removed", Value::Null, Value::Null);
        }
    }
    let common = |a: &Vec<crate::model::OfxEffect>, b: &Vec<crate::model::OfxEffect>| a.iter().map(|f| f.id).filter(|i| b.iter().any(|g| g.id == *i)).collect::<Vec<_>>();
    if common(&oc.ofx, &c.ofx) != common(&c.ofx, &oc.ofx) {
        push(out, &c.ofx[0].label, "order", Value::Null, Value::Null);
    }
}

fn transition_name(k: TransitionKind) -> &'static str {
    match k {
        TransitionKind::CrossDissolve => "Cross dissolve",
        TransitionKind::DipToBlack => "Dip to black",
    }
}

fn file_name(p: &str) -> String {
    p.rsplit(['/', '\\']).next().unwrap_or(p).to_string()
}

fn fmt_num(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.contains('.') { s.to_string() } else { format!("{s}.0") }
}

fn val(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "nothing".into(),
        Value::Bool(true) => "on".into(),
        Value::Bool(false) => "off".into(),
        other => other.to_string(),
    }
}

/// Readable sentence for a change, using real values from the project.
pub fn describe(ch: &Change, b: &Project) -> String {
    let tc = |f: i64| format!("frame {f} ({})", timecode(f, b.settings.frame_rate));
    match ch {
        Change::ClipAdded { name, track, start, .. } => format!("Clip added: {name} on {track} at {}", tc(*start)),
        Change::ClipRemoved { name, track, start, .. } => format!("Clip removed: {name} from {track} at {}", tc(*start)),
        Change::ClipTrimmed { name, before_in, before_out, after_in, after_out, .. } => {
            let mut parts = vec![];
            if before_in != after_in {
                parts.push(format!("source-in changed from frame {before_in} to frame {after_in}"));
            }
            if before_out != after_out {
                parts.push(format!("source-out changed from frame {before_out} to frame {after_out}"));
            }
            format!("{name}: {}", parts.join("; "))
        }
        Change::ClipMoved { name, track, before_start, after_start, .. } => {
            format!("{name} moved on {track} from frame {before_start} to frame {after_start}")
        }
        Change::ClipTrackChanged { name, before_track, after_track, before_start, after_start, .. } => {
            format!("{name} moved from track {before_track} at frame {before_start} to track {after_track} at frame {after_start}")
        }
        Change::ClipNameChanged { before, after, .. } => format!("Clip renamed from {before} to {after}"),
        Change::ColorOrEffectModified { name, param, before, after, .. } => {
            let mut p = param.clone();
            if let Some(f) = p.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            format!("{name}: {p} changed from {} to {}", fmt_num(*before), fmt_num(*after))
        }
        Change::AudioGainModified { name, before_db, after_db, .. } => {
            format!("{name}: gain changed from {} dB to {} dB", fmt_num(*before_db), fmt_num(*after_db))
        }
        Change::ClipMediaChanged { name, .. } => format!("{name}: source media replaced"),
        Change::TrackAdded { name, kind, .. } => format!("Track {name} added ({kind})"),
        Change::TrackRemoved { name, .. } => format!("Track {name} removed"),
        Change::TrackModified { name, field, before, after, .. } => match field.as_str() {
            "muted" => format!("Track {name} was {}", if after == &Value::Bool(true) { "muted" } else { "unmuted" }),
            "locked" => format!("Track {name} was {}", if after == &Value::Bool(true) { "locked" } else { "unlocked" }),
            _ => format!("Track renamed from {} to {}", val(before), val(after)),
        },
        Change::TrackReordered { before, after } => format!("Tracks reordered from {} to {}", before.join(", "), after.join(", ")),
        Change::ProjectSettingsModified { field, before, after } => {
            format!("Project {} changed from {} to {}", field.replace('_', " "), val(before), val(after))
        }
        Change::MediaReferenceChanged { name, field, before, after, .. } => match field.as_str() {
            "added" => format!("Media added: {name}"),
            "removed" => format!("Media removed: {name}"),
            "path" => format!("Media {name} relinked from {} to {}", val(before), val(after)),
            f => format!("Media {name}: {f} changed"),
        },
        Change::ExtraFieldModified { object, key, before, after, .. } => {
            format!("{object} field {key:?} changed from {} to {}", val(before), val(after))
        }
        Change::ClipSlipped { name, track, delta, before_in, after_in, .. } => {
            format!("{name} ({track}): slipped by {delta:+} frames (source-in {before_in} to {after_in})")
        }
        Change::ClipFadeModified { name, track, edge, before, after, .. } => match (before, after) {
            (0, a) => format!("{name} ({track}): fade-{edge} of {a} frames added"),
            (b, 0) => format!("{name} ({track}): fade-{edge} of {b} frames removed"),
            (b, a) => format!("{name} ({track}): fade-{edge} changed from {b} to {a} frames"),
        },
        Change::TransitionModified { name, track, before, after, .. } => match (before, after) {
            (None, Some(t)) => format!("{} of {} frames added before {name} ({track})", transition_name(t.kind), t.duration_frames),
            (Some(t), None) => format!("{} removed before {name} ({track})", transition_name(t.kind)),
            (Some(b), Some(a)) => format!(
                "Transition before {name} ({track}) changed from {} {}f to {} {}f",
                transition_name(b.kind),
                b.duration_frames,
                transition_name(a.kind),
                a.duration_frames
            ),
            (None, None) => format!("{name}: transition unchanged"),
        },
        Change::ClipSpeedModified { name, track, before_speed, after_speed, before_reversed, after_reversed, .. } => {
            let mut parts = vec![];
            if before_speed != after_speed {
                parts.push(format!("speed changed from {}x to {}x", fmt_num(*before_speed), fmt_num(*after_speed)));
            }
            if before_reversed != after_reversed {
                parts.push(if *after_reversed { "now plays in reverse".to_string() } else { "now plays forward".to_string() });
            }
            format!("{name} ({track}): {}", parts.join("; "))
        }
        Change::LutModified { name, before, after, .. } => match (before, after) {
            (None, Some(a)) => format!("{name}: LUT {} applied", file_name(a)),
            (Some(b), None) => format!("{name}: LUT {} removed", file_name(b)),
            (Some(b), Some(a)) => format!("{name}: LUT changed from {} to {}", file_name(b), file_name(a)),
            (None, None) => format!("{name}: LUT unchanged"),
        },
        Change::TitleAdded { label, track, start, .. } => format!("{label} added on {track} at {}", tc(*start)),
        Change::TitleRemoved { label, track, start, .. } => format!("{label} removed from {track} at {}", tc(*start)),
        Change::TitleMoved { label, before_track, after_track, before_start, after_start, .. } => {
            format!("{label} moved from {before_track} at frame {before_start} to {after_track} at frame {after_start}")
        }
        Change::TitleModified { label, field, before, after, .. } => {
            format!("{label}: {} changed from {} to {}", field.replace('_', " "), val(before), val(after))
        }
        Change::ClipTransformModified { name, track, field, before, after, .. } => {
            let pretty = field.replace('_', " ").replace("deg", "(°)");
            format!("{name} ({track}): {pretty} changed from {} to {}", fmt_num(*before), fmt_num(*after))
        }
        Change::VfxModified { name, track, effect, field, before, after, .. } => match field.as_str() {
            "added" if after.is_null() => format!("{name} ({track}): {effect} added"),
            "added" => format!("{name} ({track}): {effect} added at {}", val(after)),
            "removed" => format!("{name} ({track}): {effect} removed"),
            "enabled" => format!("{name} ({track}): {effect} {}", if after == &Value::Bool(true) { "enabled" } else { "bypassed" }),
            "order" => format!("{name} ({track}): effects reordered"),
            _ => format!("{name} ({track}): {effect} {field} changed from {} to {}", val(before), val(after)),
        },
        Change::MarkerAdded { name, frame, .. } => format!("Marker {name:?} added at {}", tc(*frame)),
        Change::MarkerRemoved { name, frame, .. } => format!("Marker {name:?} removed from {}", tc(*frame)),
        Change::MarkerModified { name, field, before, after, .. } => {
            format!("Marker {name:?}: {field} changed from {} to {}", val(before), val(after))
        }
    }
}

fn clip_of(ch: &Change) -> Option<Uuid> {
    match ch {
        Change::ClipAdded { clip_id, .. }
        | Change::ClipRemoved { clip_id, .. }
        | Change::ClipTrimmed { clip_id, .. }
        | Change::ClipMoved { clip_id, .. }
        | Change::ClipTrackChanged { clip_id, .. }
        | Change::ClipNameChanged { clip_id, .. }
        | Change::ColorOrEffectModified { clip_id, .. }
        | Change::AudioGainModified { clip_id, .. }
        | Change::ClipMediaChanged { clip_id, .. }
        | Change::ClipSlipped { clip_id, .. }
        | Change::ClipFadeModified { clip_id, .. }
        | Change::TransitionModified { clip_id, .. }
        | Change::ClipSpeedModified { clip_id, .. }
        | Change::LutModified { clip_id, .. }
        | Change::ClipTransformModified { clip_id, .. }
        | Change::VfxModified { clip_id, .. } => Some(*clip_id),
        Change::TitleAdded { title_id, .. } | Change::TitleMoved { title_id, .. } | Change::TitleModified { title_id, .. } => Some(*title_id),
        _ => None,
    }
}

fn track_name_of(p: &Project, clip_id: Uuid) -> Option<&str> {
    p.clip(clip_id).map(|(t, _)| t.name.as_str())
}

/// Typed changes with readable summaries. Summaries name the track (linked
/// picture and sound produce one entry each), and a clip added as the right
/// half of a blade cut is described as a split.
pub fn diff_entries(a: &Project, b: &Project) -> Vec<DiffEntry> {
    let changes = diff(a, b);
    let trimmed_outs: Vec<(Uuid, i64)> = changes
        .iter()
        .filter_map(|c| match c {
            Change::ClipTrimmed { clip_id, before_in, after_in, after_out, .. } if before_in == after_in => Some((*clip_id, *after_out)),
            _ => None,
        })
        .collect();
    changes
        .into_iter()
        .map(|change| {
            let mut summary = describe(&change, b);
            match &change {
                Change::ClipAdded { clip_id, name, track, start, source_in, .. } => {
                    let (_, c) = b.clip(*clip_id).expect("added clip exists in b");
                    let left = trimmed_outs.iter().find(|(lid, out)| {
                        *out == *source_in && b.clip(*lid).map(|(lt, lc)| lc.media_id == c.media_id && lc.end() == *start && lt.name == *track).unwrap_or(false)
                    });
                    if left.is_some() {
                        summary = format!("{name} split at frame {start} on {track} (new right half)");
                    }
                }
                Change::ClipTrimmed { clip_id, .. } | Change::ColorOrEffectModified { clip_id, .. } | Change::AudioGainModified { clip_id, .. } => {
                    if let (Some(t), Some((head, rest))) = (track_name_of(b, *clip_id), summary.split_once(": ")) {
                        summary = format!("{head} ({t}): {rest}");
                    }
                }
                _ => {}
            }
            DiffEntry { summary, clip_id: clip_of(&change), change }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixtures::*;
    use crate::model::{ColorParams, TrackKind};
    use crate::timeline::*;

    fn base() -> (Project, Uuid) {
        let (mut p, mid) = project();
        let v1 = p.tracks[0].id;
        let ids = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 120, source_in: Some(0), source_out: Some(200) }).unwrap();
        (p, ids[0])
    }

    fn only<'a>(ch: &'a [Change], pred: impl Fn(&Change) -> bool) -> Vec<&'a Change> {
        ch.iter().filter(|c| pred(c)).collect()
    }

    #[test]
    fn identical_projects_have_no_diff() {
        let (p, _) = base();
        assert!(diff(&p, &p.clone()).is_empty());
    }

    #[test]
    fn clip_added_and_removed() {
        let (a, v) = base();
        let (empty, _) = (Project { tracks: a.tracks.iter().map(|t| Track { clips: vec![], ..t.clone() }).collect(), ..a.clone() }, ());
        let d = diff(&empty, &a);
        assert_eq!(only(&d, |c| matches!(c, Change::ClipAdded { .. })).len(), 2);
        let d = diff(&a, &empty);
        assert!(d.iter().any(|c| matches!(c, Change::ClipRemoved { clip_id, .. } if *clip_id == v)));
        let e = diff_entries(&empty, &a);
        assert!(e[0].summary.starts_with("Clip added: intro.mp4"));
    }

    #[test]
    fn move_and_track_change() {
        let (a, v) = base();
        let mut b = a.clone();
        let v2 = b.tracks[1].id;
        move_clip(&mut b, v, v2, 240).unwrap();
        let d = diff_entries(&a, &b);
        let tc = d.iter().find(|e| matches!(e.change, Change::ClipTrackChanged { .. })).unwrap();
        assert_eq!(tc.summary, "intro.mp4 moved from track V1 at frame 120 to track V2 at frame 240");
        // Linked audio stayed on A1 but shifted: a ClipMoved.
        assert!(d.iter().any(|e| matches!(&e.change, Change::ClipMoved { before_start: 120, after_start: 240, .. })));
    }

    #[test]
    fn trim_is_not_reported_as_move() {
        let (a, v) = base();
        let mut b = a.clone();
        trim_clip(&mut b, v, Edge::Out, 186).unwrap();
        trim_clip(&mut b, v, Edge::In, 10).unwrap();
        let d = diff(&a, &b);
        assert!(only(&d, |c| matches!(c, Change::ClipMoved { .. })).is_empty());
        let e = diff_entries(&a, &b);
        assert!(e.iter().any(|e| e.summary == "intro.mp4 (V1): source-in changed from frame 0 to frame 10; source-out changed from frame 200 to frame 186"));
    }

    #[test]
    fn split_is_described_as_split() {
        let (a, v) = base();
        let mut b = a.clone();
        split_clip(&mut b, v, 200).unwrap();
        let e = diff_entries(&a, &b);
        assert!(e.iter().any(|e| e.summary == "intro.mp4 split at frame 200 on V1 (new right half)"), "{:?}", e.iter().map(|e| &e.summary).collect::<Vec<_>>());
        assert!(e.iter().any(|e| e.summary == "intro.mp4 (V1): source-out changed from frame 200 to frame 80"));
    }

    #[test]
    fn v2_clip_changes_are_described() {
        use crate::model::{Transition, TransitionKind};
        let (mut a, v) = base(); // intro 0..200 at 120
        trim_clip(&mut a, v, Edge::In, 20).unwrap();
        let v1 = a.tracks[0].id;
        let mid = a.media[0].id;
        place_clip(&mut a, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(140) }).unwrap();
        let mut b = a.clone();
        slip_clip(&mut b, v, 12).unwrap();
        update_clip(&mut b, v, ClipPatch { fade_out_frames: Some(15), speed: None, ..Default::default() }).unwrap();
        update_clip(&mut b, v, ClipPatch { transition_in: Some(Some(Transition { kind: TransitionKind::DipToBlack, duration_frames: 8 })), lut: Some(Some("C:/luts/Warm.cube".into())), ..Default::default() }).unwrap();
        let e = diff_entries(&a, &b);
        let s: Vec<&str> = e.iter().map(|e| e.summary.as_str()).collect();
        assert!(s.contains(&"intro.mp4 (V1): slipped by +12 frames (source-in 20 to 32)"), "{s:?}");
        assert!(!e.iter().any(|e| matches!(e.change, Change::ClipTrimmed { .. })), "a slip is not a trim");
        assert!(s.contains(&"intro.mp4 (V1): fade-out of 15 frames added"), "{s:?}");
        assert!(s.contains(&"Dip to black of 8 frames added before intro.mp4 (V1)"), "{s:?}");
        assert!(s.contains(&"intro.mp4: LUT Warm.cube applied"), "{s:?}");
        let mut c = a.clone();
        update_clip(&mut c, v, ClipPatch { speed: Some(2.0), reversed: Some(true), ..Default::default() }).unwrap();
        let e = diff_entries(&a, &c);
        assert!(e.iter().any(|e| e.summary == "intro.mp4 (V1): speed changed from 1.0x to 2.0x; now plays in reverse"), "{:?}", e.iter().map(|e| &e.summary).collect::<Vec<_>>());
        assert!(!e.iter().any(|e| matches!(e.change, Change::ClipMoved { .. })), "a retime is not a move");
        assert!(e.iter().all(|e| e.clip_id.is_some()), "every clip change marks its clip");
    }

    #[test]
    fn titles_markers_and_mixer() {
        let (a, _) = base();
        let mut b = a.clone();
        let v2 = b.tracks[1].id;
        let t = add_title(&mut b, NewTitle { track_id: v2, start: 0, duration: 50, text: "Harbour".into(), font_size: Some(60), color_hex: None, position: None }).unwrap();
        let m = add_marker(&mut b, 75, Some("Music in".into()), Some("red".into())).unwrap();
        let a1 = b.tracks[2].id;
        update_track(&mut b, a1, TrackPatch { pan: Some(-0.5), eq_high_db: Some(2.0), ..Default::default() }).unwrap();
        let e = diff_entries(&a, &b);
        let s: Vec<&str> = e.iter().map(|e| e.summary.as_str()).collect();
        assert!(s.contains(&"Title “Harbour” added on V2 at frame 0 (00:00:00:00)"), "{s:?}");
        assert!(s.contains(&"Marker \"Music in\" added at frame 75 (00:00:03:00)"), "{s:?}");
        assert!(e.iter().any(|e| matches!(&e.change, Change::TrackModified { field, .. } if field == "pan")));
        assert!(e.iter().any(|e| matches!(&e.change, Change::TrackModified { field, .. } if field == "eq_high_db")));
        let mut c = b.clone();
        update_title(&mut c, t, TitlePatch { text: Some("Harbour, 1912".into()), ..Default::default() }).unwrap();
        update_marker(&mut c, m, MarkerPatch { frame: Some(80), ..Default::default() }).unwrap();
        set_range(&mut c, Some(10), Some(90)).unwrap();
        let e = diff_entries(&b, &c);
        let s: Vec<&str> = e.iter().map(|e| e.summary.as_str()).collect();
        assert!(s.contains(&"Title “Harbour, 1912”: text changed from Harbour to Harbour, 1912"), "{s:?}");
        assert!(s.contains(&"Marker \"Music in\": frame changed from 75 to 80"), "{s:?}");
        assert!(s.contains(&"Project in/out range changed from nothing to 10..90"), "{s:?}");
        assert_eq!(e.iter().find(|e| matches!(e.change, Change::TitleModified { .. })).unwrap().clip_id, Some(t));
    }

    #[test]
    fn color_gain_name_changes() {
        let (a, v) = base();
        let mut b = a.clone();
        let mut col = ColorParams::default();
        col.saturation = 1.2;
        update_clip(&mut b, v, ClipPatch { name: Some("Opening".into()), gain_db: Some(-3.0), color: Some(col), ..Default::default() }).unwrap();
        let e = diff_entries(&a, &b);
        let s: Vec<&str> = e.iter().map(|e| e.summary.as_str()).collect();
        assert!(s.contains(&"Opening (V1): Saturation changed from 1.0 to 1.2"), "{s:?}");
        assert!(s.contains(&"Opening (V1): gain changed from 0.0 dB to -3.0 dB"), "{s:?}");
        assert!(s.contains(&"Clip renamed from intro.mp4 to Opening"), "{s:?}");
    }

    #[test]
    fn track_changes() {
        let (a, _) = base();
        let mut b = a.clone();
        let a1 = b.tracks[2].id;
        update_track(&mut b, a1, TrackPatch { muted: Some(true), ..Default::default() }).unwrap();
        let new = add_track(&mut b, TrackKind::Video).unwrap();
        let e = diff_entries(&a, &b);
        assert!(e.iter().any(|e| e.summary == "Track A1 was muted"));
        assert!(e.iter().any(|e| matches!(e.change, Change::TrackAdded { track_id, .. } if track_id == new)));
        let mut c = a.clone();
        let v2 = c.tracks[1].id;
        reorder_track(&mut c, v2, 0).unwrap();
        assert!(diff(&a, &c).iter().any(|ch| matches!(ch, Change::TrackReordered { .. })));
        let mut d = a.clone();
        let t = d.tracks[3].id;
        remove_track(&mut d, t).unwrap();
        assert!(diff(&a, &d).iter().any(|ch| matches!(ch, Change::TrackRemoved { .. })));
    }

    #[test]
    fn settings_media_and_unknown_fields() {
        let (a, _) = base();
        let mut b = a.clone();
        b.settings.width = 1280;
        b.settings.height = 720;
        b.media[0].path = "/new/intro.mp4".into();
        b.tracks[0].clips[0].extra.insert("future".into(), Value::from(1));
        let e = diff_entries(&a, &b);
        assert!(e.iter().any(|e| e.summary == "Project resolution changed from 1920x1080 to 1280x720"));
        assert!(e.iter().any(|e| e.summary == "Media intro.mp4 relinked from /media/intro.mp4 to /new/intro.mp4"));
        assert!(e.iter().any(|e| matches!(&e.change, Change::ExtraFieldModified { key, .. } if key == "future")));
    }
}

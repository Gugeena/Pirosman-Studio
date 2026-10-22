//! Three-way, property-level project merge.
//!
//! The project is flattened into objects keyed by stable id (settings,
//! project metadata, media, tracks, clips) whose properties are JSON values.
//! A clip's track membership is just another property (`track_id`), so a
//! move on one branch and a color change on the other merge cleanly.
//!
//! Per property: unchanged on one side → take the other; same change on both
//! → take it; different changes → **conflict** (never silently resolved).
//! Delete-vs-modify is a conflict. After property merging, the rebuilt
//! timeline is validated; clips that now overlap, or clips left on a deleted
//! track, become structural conflicts that the user resolves per clip group.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};
use crate::model::{Clip, Marker, MediaRef, Project, ProjectSettings, TitleClip, Track};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Ours,
    Theirs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    /// Stable key, used to submit a resolution.
    pub id: String,
    pub object_kind: String,
    pub object_id: String,
    pub label: String,
    pub property: String,
    pub base: Value,
    pub ours: Value,
    pub theirs: Value,
    pub explanation: String,
    pub resolution: Option<Side>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeOutcome {
    pub conflicts: Vec<Conflict>,
    /// Properties merged automatically (one line each).
    pub auto_merged: Vec<String>,
    /// Present when every conflict is resolved and the result validates.
    pub merged: Option<Project>,
}

type Key = (String, String);
type Objects = BTreeMap<Key, Map<String, Value>>;

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn flatten(p: &Project) -> Objects {
    let mut o = Objects::new();
    let mut meta = p.extra.clone();
    meta.insert("name".into(), p.name.clone().into());
    if let Some(v) = p.mark_in {
        meta.insert("mark_in".into(), v.into());
    }
    if let Some(v) = p.mark_out {
        meta.insert("mark_out".into(), v.into());
    }
    o.insert(("project".into(), "project".into()), meta);
    for m in &p.markers {
        o.insert(("marker".into(), m.id.to_string()), obj(serde_json::to_value(m).unwrap()));
    }
    o.insert(("settings".into(), "settings".into()), obj(serde_json::to_value(&p.settings).unwrap()));
    for m in &p.media {
        o.insert(("media".into(), m.id.to_string()), obj(serde_json::to_value(m).unwrap()));
    }
    for t in &p.tracks {
        let mut tm = obj(serde_json::to_value(t).unwrap());
        tm.remove("clips");
        tm.remove("titles");
        o.insert(("track".into(), t.id.to_string()), tm);
        for x in &t.titles {
            let mut xm = obj(serde_json::to_value(x).unwrap());
            xm.insert("track_id".into(), t.id.to_string().into());
            o.insert(("title".into(), x.id.to_string()), xm);
        }
        for c in &t.clips {
            let mut cm = obj(serde_json::to_value(c).unwrap());
            cm.insert("track_id".into(), t.id.to_string().into());
            // Each grade parameter merges independently.
            // Each grade / transform / crop field merges independently. Defaults are
            // flattened explicitly (they are omitted from JSON) so "unchanged"
            // compares equal on every side.
            let groups = [
                ("color", serde_json::to_value(c.color).unwrap()),
                ("transform", serde_json::to_value(c.transform).unwrap()),
                ("crop", serde_json::to_value(c.crop).unwrap()),
            ];
            for (group, v) in groups {
                cm.remove(group);
                if let Value::Object(m) = v {
                    for (k, v) in m {
                        cm.insert(format!("{group}.{k}"), v);
                    }
                }
            }
            o.insert(("clip".into(), c.id.to_string()), cm);
        }
    }
    o
}

fn label(kind: &str, props: Option<&Map<String, Value>>, id: &str) -> String {
    let name = props.and_then(|m| m.get("name").or_else(|| m.get("text"))).and_then(Value::as_str).unwrap_or(id);
    match kind {
        "project" => "Project".into(),
        "settings" => "Project settings".into(),
        k => format!("{} {name}", k[..1].to_uppercase() + &k[1..]),
    }
}

const ABSENT: &str = "<absent>";

fn present(v: Option<&Map<String, Value>>) -> Value {
    v.map(|m| Value::Object(m.clone())).unwrap_or_else(|| Value::String(ABSENT.into()))
}

struct Ctx<'a> {
    choices: &'a HashMap<String, Side>,
    conflicts: Vec<Conflict>,
    auto: Vec<String>,
}

impl Ctx<'_> {
    fn conflict(&mut self, c: Conflict) -> Side {
        let r = self.choices.get(&c.id).copied();
        let side = r.unwrap_or(Side::Ours);
        self.conflicts.push(Conflict { resolution: r, ..c });
        side
    }
}

fn merge_objects(base: &Objects, ours: &Objects, theirs: &Objects, ctx: &mut Ctx) -> Objects {
    let keys: BTreeSet<&Key> = base.keys().chain(ours.keys()).chain(theirs.keys()).collect();
    let mut out = Objects::new();
    for key in keys {
        let (kind, id) = (key.0.as_str(), key.1.as_str());
        let (b, o, t) = (base.get(key), ours.get(key), theirs.get(key));
        let lbl = label(kind, o.or(t).or(b), id);
        match (b, o, t) {
            (_, Some(o), Some(t)) => {
                let empty = Map::new();
                let b = b.unwrap_or(&empty);
                let mut m = Map::new();
                let props: BTreeSet<&String> = b.keys().chain(o.keys()).chain(t.keys()).collect();
                // Keep ours' property order for stable serialization.
                let mut ordered: Vec<&String> = o.keys().collect();
                ordered.extend(props.iter().filter(|p| !o.contains_key(**p)).copied());
                for p in ordered {
                    let (bv, ov, tv) = (b.get(p), o.get(p), t.get(p));
                    let pick = if ov == tv {
                        ov
                    } else if ov == bv {
                        if tv.is_some() {
                            ctx.auto.push(format!("{lbl}: {p} taken from theirs"));
                        }
                        tv
                    } else if tv == bv {
                        ov
                    } else {
                        let side = ctx.conflict(Conflict {
                            id: format!("{kind}:{id}:{p}"),
                            object_kind: kind.into(),
                            object_id: id.into(),
                            label: lbl.clone(),
                            property: p.clone(),
                            base: bv.cloned().unwrap_or(Value::Null),
                            ours: ov.cloned().unwrap_or(Value::Null),
                            theirs: tv.cloned().unwrap_or(Value::Null),
                            explanation: format!("Both branches changed {p} of {lbl} to different values."),
                            resolution: None,
                        });
                        if side == Side::Ours { ov } else { tv }
                    };
                    if let Some(v) = pick {
                        m.insert(p.clone(), v.clone());
                    }
                }
                out.insert(key.clone(), m);
            }
            (None, Some(o), None) => {
                out.insert(key.clone(), o.clone());
            }
            (None, None, Some(t)) => {
                ctx.auto.push(format!("{lbl} added from theirs"));
                out.insert(key.clone(), t.clone());
            }
            (Some(_), None, None) | (None, None, None) => {}
            (Some(b), None, Some(t)) => {
                if t == b {
                    // Deleted on ours, untouched on theirs.
                } else {
                    let side = ctx.conflict(Conflict {
                        id: format!("{kind}:{id}:*"),
                        object_kind: kind.into(),
                        object_id: id.into(),
                        label: lbl.clone(),
                        property: "(exists)".into(),
                        base: present(Some(b)),
                        ours: present(None),
                        theirs: present(Some(t)),
                        explanation: format!("Ours deleted {lbl}, but theirs modified it."),
                        resolution: None,
                    });
                    if side == Side::Theirs {
                        out.insert(key.clone(), t.clone());
                    }
                }
            }
            (Some(b), Some(o), None) => {
                if o == b {
                    ctx.auto.push(format!("{lbl} deleted by theirs"));
                } else {
                    let side = ctx.conflict(Conflict {
                        id: format!("{kind}:{id}:*"),
                        object_kind: kind.into(),
                        object_id: id.into(),
                        label: lbl.clone(),
                        property: "(exists)".into(),
                        base: present(Some(b)),
                        ours: present(Some(o)),
                        theirs: present(None),
                        explanation: format!("Theirs deleted {lbl}, but ours modified it."),
                        resolution: None,
                    });
                    if side == Side::Ours {
                        out.insert(key.clone(), o.clone());
                    }
                }
            }
        }
    }
    out
}

fn track_order(p: &Project) -> Vec<String> {
    p.tracks.iter().map(|t| t.id.to_string()).collect()
}

type Orphan = (String, Uuid, String);

fn rebuild(template: &Project, objs: &Objects, order: &[String], ours: &Project, theirs: &Project) -> CoreResult<(Project, Vec<Orphan>)> {
    let mut p = template.clone();
    let mut meta = objs.get(&("project".into(), "project".into())).cloned().unwrap_or_default();
    p.name = meta.remove("name").and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| template.name.clone());
    p.mark_in = meta.remove("mark_in").and_then(|v| v.as_i64());
    p.mark_out = meta.remove("mark_out").and_then(|v| v.as_i64());
    p.extra = meta;
    p.markers = objs
        .iter()
        .filter(|((k, _), _)| k == "marker")
        .map(|(_, m)| serde_json::from_value::<Marker>(Value::Object(m.clone())))
        .collect::<Result<_, _>>()?;
    p.settings = serde_json::from_value::<ProjectSettings>(Value::Object(objs.get(&("settings".into(), "settings".into())).cloned().unwrap_or_default()))?;

    // Media: ours order, then theirs order for additions.
    let media_ids: Vec<String> = ours.media.iter().chain(theirs.media.iter()).map(|m| m.id.to_string()).collect();
    let mut seen = BTreeSet::new();
    p.media = vec![];
    for id in media_ids {
        if seen.insert(id.clone()) {
            if let Some(m) = objs.get(&("media".into(), id)) {
                p.media.push(serde_json::from_value::<MediaRef>(Value::Object(m.clone()))?);
            }
        }
    }

    // Tracks in the merged order; added tracks join after their kind group.
    p.tracks = vec![];
    let mut placed = BTreeSet::new();
    for id in order {
        if let Some(t) = objs.get(&("track".into(), id.clone())) {
            p.tracks.push(serde_json::from_value::<Track>(Value::Object(t.clone()))?);
            placed.insert(id.clone());
        }
    }
    for ((kind, id), t) in objs.iter() {
        if kind == "track" && !placed.contains(id) {
            let tr: Track = serde_json::from_value(Value::Object(t.clone()))?;
            let pos = p.tracks.iter().rposition(|x| x.kind == tr.kind).map(|i| i + 1).unwrap_or(p.tracks.len());
            p.tracks.insert(pos, tr);
        }
    }

    // Clips onto their tracks.
    let mut orphans = vec![];
    for ((kind, id), c) in objs.iter() {
        if kind != "clip" {
            continue;
        }
        let mut c = c.clone();
        let tid = c.remove("track_id").and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        for group in ["color", "transform", "crop"] {
            let prefix = format!("{group}.");
            let keys: Vec<String> = c.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
            if keys.is_empty() {
                continue;
            }
            let mut m = Map::new();
            for k in keys {
                let v = c.remove(&k).unwrap();
                m.insert(k[prefix.len()..].to_string(), v);
            }
            c.insert(group.into(), Value::Object(m));
        }
        let clip: Clip = serde_json::from_value(Value::Object(c))?;
        match p.tracks.iter_mut().find(|t| t.id.to_string() == tid) {
            Some(t) => t.clips.push(clip),
            None => orphans.push(("clip".into(), Uuid::parse_str(id).unwrap_or_default(), tid)),
        }
    }
    for ((kind, id), x) in objs.iter() {
        if kind != "title" {
            continue;
        }
        let mut x = x.clone();
        let tid = x.remove("track_id").and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        let title: TitleClip = serde_json::from_value(Value::Object(x))?;
        match p.tracks.iter_mut().find(|t| t.id.to_string() == tid) {
            Some(t) => t.titles.push(title),
            None => orphans.push(("title".into(), Uuid::parse_str(id).unwrap_or_default(), tid)),
        }
    }
    p.sort_clips();
    Ok((p, orphans))
}

/// Overlapping item pairs (clips and titles) per track, as (track name, (kind, id), (kind, id)).
fn overlaps(p: &Project) -> Vec<(String, (String, Uuid), (String, Uuid))> {
    let mut out = vec![];
    for t in &p.tracks {
        let mut items: Vec<(i64, i64, &str, Uuid)> = t.clips.iter().map(|c| (c.start, c.end(), "clip", c.id)).collect();
        items.extend(t.titles.iter().map(|x| (x.start, x.end(), "title", x.id)));
        items.sort_by_key(|i| (i.0, i.3));
        for w in items.windows(2) {
            if w[1].0 < w[0].1 {
                out.push((t.name.clone(), (w[0].2.to_string(), w[0].3), (w[1].2.to_string(), w[1].3)));
            }
        }
    }
    out
}

/// Merge `theirs` into `ours` relative to `base`, applying `choices` to
/// conflicts. Unresolved conflicts tentatively keep ours.
pub fn merge(base: &Project, ours: &Project, theirs: &Project, choices: &HashMap<String, Side>) -> CoreResult<MergeOutcome> {
    if base.id != ours.id || ours.id != theirs.id {
        return Err(CoreError::Invalid("cannot merge revisions of different projects".into()));
    }
    let (fb, fo, ft) = (flatten(base), flatten(ours), flatten(theirs));
    let mut ctx = Ctx { choices, conflicts: vec![], auto: vec![] };
    let mut objs = merge_objects(&fb, &fo, &ft, &mut ctx);

    let (ob, oo, ot) = (track_order(base), track_order(ours), track_order(theirs));
    let common = |v: &Vec<String>| -> Vec<String> { v.iter().filter(|i| ob.contains(i) && oo.contains(i) && ot.contains(i)).cloned().collect() };
    let (cb, co, ct) = (common(&ob), common(&oo), common(&ot));
    let order = if co == ct || ct == cb {
        oo.clone()
    } else if co == cb {
        ctx.auto.push("Track order taken from theirs".into());
        ot.clone()
    } else {
        let side = ctx.conflict(Conflict {
            id: "tracks:order".into(),
            object_kind: "tracks".into(),
            object_id: "order".into(),
            label: "Track order".into(),
            property: "order".into(),
            base: cb.clone().into(),
            ours: co.clone().into(),
            theirs: ct.clone().into(),
            explanation: "Both branches reordered tracks differently.".into(),
            resolution: None,
        });
        if side == Side::Ours { oo.clone() } else { ot.clone() }
    };

    // Structural pass: resolve orphaned clips and overlaps via clip-level choices.
    let (fo_clips, ft_clips) = (fo.clone(), ft.clone());
    let mut project = None;
    for _ in 0..8 {
        let (p, orphans) = rebuild(ours, &objs, &order, ours, theirs)?;
        let mut changed = false;
        for (okind, clip, tid) in orphans {
            let key: Key = (okind.clone(), clip.to_string());
            let lbl = label(&okind, objs.get(&key), &clip.to_string());
            let side = ctx.conflict(Conflict {
                id: format!("orphan:{clip}"),
                object_kind: okind.clone(),
                object_id: clip.to_string(),
                label: lbl.clone(),
                property: "track_id".into(),
                base: Value::Null,
                ours: present(fo_clips.get(&key)),
                theirs: present(ft_clips.get(&key)),
                explanation: format!("{lbl} sits on a track that the other branch deleted."),
                resolution: None,
            });
            // Ours = drop the clip; theirs = restore the track it needs.
            if side == Side::Theirs {
                let tk: Key = ("track".into(), tid.clone());
                if let Some(t) = ft.get(&tk).or(fo.get(&tk)).or(fb.get(&tk)) {
                    objs.insert(tk, t.clone());
                } else {
                    objs.remove(&key);
                }
            } else {
                objs.remove(&key);
            }
            changed = true;
        }
        if changed {
            continue;
        }
        let ov = overlaps(&p);
        if ov.is_empty() {
            project = Some(p);
            break;
        }
        for (track, (kind_a, a), (kind_b, b)) in ov {
            let (ka, kb): (Key, Key) = ((kind_a.clone(), a.to_string()), (kind_b.clone(), b.to_string()));
            let (la, lb) = (label(&kind_a, objs.get(&ka), &a.to_string()), label(&kind_b, objs.get(&kb), &b.to_string()));
            let side = ctx.conflict(Conflict {
                id: format!("overlap:{a}:{b}"),
                object_kind: "timeline".into(),
                object_id: format!("{a}:{b}"),
                label: format!("{la} / {lb}"),
                property: "placement".into(),
                base: Value::Null,
                ours: Value::Array(vec![present(fo_clips.get(&ka)), present(fo_clips.get(&kb))]),
                theirs: Value::Array(vec![present(ft_clips.get(&ka)), present(ft_clips.get(&kb))]),
                explanation: format!("After merging, {la} and {lb} overlap on track {track}."),
                resolution: None,
            });
            let src = if side == Side::Ours { &fo_clips } else { &ft_clips };
            for k in [ka, kb] {
                match src.get(&k) {
                    Some(v) => {
                        objs.insert(k, v.clone());
                    }
                    None => {
                        objs.remove(&k);
                    }
                }
            }
        }
        // Both clips now come from one valid side, so they no longer collide
        // with each other; any remaining collision with a third clip surfaces
        // as a new conflict on the next pass.
    }

    // Deduplicate conflicts produced across passes (keep first).
    let mut seen = BTreeSet::new();
    ctx.conflicts.retain(|c| seen.insert(c.id.clone()));
    let all_resolved = ctx.conflicts.iter().all(|c| c.resolution.is_some());
    let merged = match project {
        Some(p) if all_resolved => {
            p.validate().map_err(|e| CoreError::Conflict(format!("merged project is invalid: {e}")))?;
            Some(p)
        }
        _ => None,
    };
    Ok(MergeOutcome { conflicts: ctx.conflicts, auto_merged: ctx.auto, merged })
}

/// Persisted merge in progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeState {
    pub base: String,
    pub ours: String,
    pub theirs: String,
    pub theirs_branch: String,
    pub choices: HashMap<String, Side>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixtures::*;
    use crate::model::{ColorParams, TrackKind};
    use crate::timeline::*;

    fn base() -> (Project, Uuid, Uuid) {
        let (mut p, mid) = project();
        let v1 = p.tracks[0].id;
        let a = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(50) }).unwrap();
        let b = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 100, source_in: Some(50), source_out: Some(100) }).unwrap();
        (p, a[0], b[0])
    }

    fn none() -> HashMap<String, Side> {
        HashMap::new()
    }

    #[test]
    fn independent_clip_changes_merge() {
        let (b, c1, c2) = base();
        let mut ours = b.clone();
        let mut theirs = b.clone();
        trim_clip(&mut ours, c1, Edge::Out, 40).unwrap();
        let mut col = ColorParams::default();
        col.saturation = 1.5;
        update_clip(&mut theirs, c2, ClipPatch { color: Some(col), ..Default::default() }).unwrap();
        let out = merge(&b, &ours, &theirs, &none()).unwrap();
        assert!(out.conflicts.is_empty(), "{:?}", out.conflicts);
        let m = out.merged.unwrap();
        assert_eq!(m.clip(c1).unwrap().1.source_out, 40);
        assert_eq!(m.clip(c2).unwrap().1.color.saturation, 1.5);
    }

    #[test]
    fn different_properties_of_same_clip_merge() {
        let (b, c1, _) = base();
        let mut ours = b.clone();
        let mut theirs = b.clone();
        update_clip(&mut ours, c1, ClipPatch { name: Some("Hero".into()), ..Default::default() }).unwrap();
        update_clip(&mut theirs, c1, ClipPatch { gain_db: Some(-4.0), ..Default::default() }).unwrap();
        let out = merge(&b, &ours, &theirs, &none()).unwrap();
        let m = out.merged.unwrap();
        let c = m.clip(c1).unwrap().1;
        assert_eq!((c.name.as_str(), c.gain_db), ("Hero", -4.0));
    }

    #[test]
    fn same_property_conflicts_and_resolves() {
        let (b, c1, _) = base();
        let mut ours = b.clone();
        let mut theirs = b.clone();
        let mut x = ColorParams::default();
        x.saturation = 1.2;
        update_clip(&mut ours, c1, ClipPatch { color: Some(x), ..Default::default() }).unwrap();
        x.saturation = 0.8;
        update_clip(&mut theirs, c1, ClipPatch { color: Some(x), ..Default::default() }).unwrap();
        let out = merge(&b, &ours, &theirs, &none()).unwrap();
        assert_eq!(out.conflicts.len(), 1);
        let c = &out.conflicts[0];
        assert_eq!(c.property, "color.saturation");
        assert_eq!((c.base.clone(), c.ours.clone(), c.theirs.clone()), (Value::from(1.0), Value::from(1.2), Value::from(0.8)));
        assert!(out.merged.is_none(), "unresolved conflicts block the merge");
        let choices = HashMap::from([(c.id.clone(), Side::Theirs)]);
        let m = merge(&b, &ours, &theirs, &choices).unwrap().merged.unwrap();
        assert_eq!(m.clip(c1).unwrap().1.color.saturation, 0.8);
    }

    #[test]
    fn different_grade_parameters_merge() {
        let (b, c1, _) = base();
        let mut ours = b.clone();
        let mut theirs = b.clone();
        let mut x = ColorParams::default();
        x.lift = 0.05;
        update_clip(&mut ours, c1, ClipPatch { color: Some(x), ..Default::default() }).unwrap();
        let mut y = ColorParams::default();
        y.saturation = 1.4;
        update_clip(&mut theirs, c1, ClipPatch { color: Some(y), ..Default::default() }).unwrap();
        let m = merge(&b, &ours, &theirs, &none()).unwrap().merged.unwrap();
        let c = m.clip(c1).unwrap().1.color;
        assert_eq!((c.lift, c.saturation), (0.05, 1.4));
    }

    #[test]
    fn v2_properties_merge_independently() {
        use crate::model::{Transition, TransitionKind};
        let (b, c1, c2) = base(); // c1 0..50, c2 100..150 on V1
        let mut ours = b.clone();
        let mut theirs = b.clone();
        update_clip(&mut ours, c1, ClipPatch { fade_in_frames: Some(10), ..Default::default() }).unwrap();
        update_clip(&mut theirs, c1, ClipPatch { lut: Some(Some("/l/a.cube".into())), ..Default::default() }).unwrap();
        update_clip(&mut theirs, c2, ClipPatch { speed: Some(2.0), ..Default::default() }).unwrap();
        let v2 = b.tracks[1].id;
        add_title(&mut ours, NewTitle { track_id: v2, start: 0, duration: 40, text: "Ours".into(), font_size: None, color_hex: None, position: None }).unwrap();
        add_marker(&mut theirs, 30, Some("Theirs".into()), None).unwrap();
        let a1 = b.tracks[2].id;
        update_track(&mut ours, a1, TrackPatch { pan: Some(0.3), ..Default::default() }).unwrap();
        update_track(&mut theirs, a1, TrackPatch { eq_mid_db: Some(-3.0), ..Default::default() }).unwrap();
        set_range(&mut theirs, Some(0), Some(80)).unwrap();
        let out = merge(&b, &ours, &theirs, &none()).unwrap();
        assert!(out.conflicts.is_empty(), "{:?}", out.conflicts.iter().map(|c| &c.id).collect::<Vec<_>>());
        let m = out.merged.unwrap();
        let c = m.clip(c1).unwrap().1;
        assert_eq!((c.fade_in_frames, c.lut.as_deref()), (10, Some("/l/a.cube")));
        assert_eq!(m.clip(c2).unwrap().1.speed, 2.0);
        assert_eq!(m.tracks[1].titles[0].text, "Ours");
        assert_eq!(m.markers[0].name, "Theirs");
        let t = m.track(a1).unwrap();
        assert_eq!((t.pan, t.eq_mid_db), (0.3, -3.0));
        assert_eq!((m.mark_in, m.mark_out), (Some(0), Some(80)));
        // Same property, different values → conflict (transition kind).
        let (b2, _, _) = base();
        let v1 = b2.tracks[0].id;
        let mid = b2.media[0].id;
        let mut base2 = b2.clone();
        let cx = place_clip(&mut base2, PlaceClip { media_id: mid, track_id: v1, start: 50, source_in: Some(100), source_out: Some(140) }).unwrap()[0];
        let mut o2 = base2.clone();
        let mut t2 = base2.clone();
        update_clip(&mut o2, cx, ClipPatch { transition_in: Some(Some(Transition { kind: TransitionKind::DipToBlack, duration_frames: 6 })), ..Default::default() }).unwrap();
        update_clip(&mut t2, cx, ClipPatch { transition_in: Some(Some(Transition { kind: TransitionKind::CrossDissolve, duration_frames: 10 })), ..Default::default() }).unwrap();
        let out = merge(&base2, &o2, &t2, &none()).unwrap();
        assert!(out.conflicts.iter().any(|c| c.property == "transition_in"), "{:?}", out.conflicts.iter().map(|c| &c.id).collect::<Vec<_>>());
    }

    #[test]
    fn title_overlap_is_a_timeline_conflict() {
        let (b, _, _) = base();
        let mut ours = b.clone();
        let mut theirs = b.clone();
        let v2 = b.tracks[1].id;
        add_title(&mut ours, NewTitle { track_id: v2, start: 0, duration: 40, text: "A".into(), font_size: None, color_hex: None, position: None }).unwrap();
        add_title(&mut theirs, NewTitle { track_id: v2, start: 20, duration: 40, text: "B".into(), font_size: None, color_hex: None, position: None }).unwrap();
        let out = merge(&b, &ours, &theirs, &none()).unwrap();
        let c = out.conflicts.iter().find(|c| c.id.starts_with("overlap:")).expect("overlap conflict");
        let choices = HashMap::from([(c.id.clone(), Side::Theirs)]);
        let m = merge(&b, &ours, &theirs, &choices).unwrap().merged.unwrap();
        assert_eq!(m.tracks[1].titles.len(), 1);
        assert_eq!(m.tracks[1].titles[0].text, "B");
    }

    #[test]
    fn transform_fields_and_vfx_merge() {
        use crate::model::{Transform, Vfx, VfxKind};
        let (b, c1, _) = base();
        let mut ours = b.clone();
        let mut theirs = b.clone();
        update_clip(&mut ours, c1, ClipPatch { transform: Some(Transform { scale_x: 1.5, ..Default::default() }), ..Default::default() }).unwrap();
        update_clip(&mut theirs, c1, ClipPatch { transform: Some(Transform { rotation_deg: 10.0, ..Default::default() }), vfx: Some(vec![Vfx { id: Uuid::new_v4(), kind: VfxKind::FilmGrain, amount: 0.3, enabled: true }]), ..Default::default() }).unwrap();
        let out = merge(&b, &ours, &theirs, &none()).unwrap();
        assert!(out.conflicts.is_empty(), "{:?}", out.conflicts.iter().map(|c| &c.id).collect::<Vec<_>>());
        let m = out.merged.unwrap();
        let c = m.clip(c1).unwrap().1;
        assert_eq!((c.transform.scale_x, c.transform.rotation_deg, c.vfx.len()), (1.5, 10.0, 1));
        let d = crate::diff::diff_entries(&b, &m);
        let s: Vec<&str> = d.iter().map(|e| e.summary.as_str()).collect();
        assert!(s.contains(&"intro.mp4 (V1): scale x changed from 1.0 to 1.5"), "{s:?}");
        assert!(s.contains(&"intro.mp4 (V1): rotation (°) changed from 0.0 to 10.0"), "{s:?}");
        assert!(s.contains(&"intro.mp4 (V1): Film grain added at 0.3"), "{s:?}");
    }

    #[test]
    fn delete_vs_modify_conflicts() {
        let (b, c1, _) = base();
        let mut ours = b.clone();
        let mut theirs = b.clone();
        delete_clips(&mut ours, &[c1], false).unwrap();
        update_clip(&mut theirs, c1, ClipPatch { gain_db: Some(-2.0), ..Default::default() }).unwrap();
        let out = merge(&b, &ours, &theirs, &none()).unwrap();
        assert!(out.conflicts.iter().any(|c| c.id == format!("clip:{c1}:*")));
        // Unmodified linked audio deletion merges cleanly.
        assert!(out.auto_merged.iter().any(|s| s.contains("deleted") ) || out.conflicts.len() == 1);
    }

    #[test]
    fn overlapping_moves_become_timeline_conflict() {
        let (b, c1, c2) = base();
        let mut ours = b.clone();
        let mut theirs = b.clone();
        let v1 = b.tracks[0].id;
        // Ours moves c1 later into 60..110? keep valid on its own: 50..100.
        move_clip(&mut ours, c1, v1, 50).unwrap();
        // Theirs extends c2 to start earlier (in-trim) into 70.
        trim_clip(&mut theirs, c2, Edge::In, 20).unwrap();
        let out = merge(&b, &ours, &theirs, &none()).unwrap();
        assert!(out.conflicts.iter().any(|c| c.id.starts_with("overlap:")), "{:?}", out.conflicts.iter().map(|c| &c.id).collect::<Vec<_>>());
        let choices: HashMap<String, Side> = out.conflicts.iter().map(|c| (c.id.clone(), Side::Ours)).collect();
        let m = merge(&b, &ours, &theirs, &choices).unwrap();
        m.merged.expect("resolved merge validates").validate().unwrap();
    }

    #[test]
    fn track_added_both_sides_and_order() {
        let (b, _, _) = base();
        let mut ours = b.clone();
        let mut theirs = b.clone();
        let x = add_track(&mut ours, TrackKind::Video).unwrap();
        let y = add_track(&mut theirs, TrackKind::Audio).unwrap();
        let m = merge(&b, &ours, &theirs, &none()).unwrap().merged.unwrap();
        assert!(m.track(x).is_some() && m.track(y).is_some());
    }

    #[test]
    fn unknown_fields_survive_merge() {
        let (b, c1, _) = base();
        let ours = b.clone();
        let mut theirs = b.clone();
        theirs.tracks[0].clips.iter_mut().find(|c| c.id == c1).unwrap().extra.insert("note".into(), Value::from("keep"));
        let m = merge(&b, &ours, &theirs, &none()).unwrap().merged.unwrap();
        assert_eq!(m.clip(c1).unwrap().1.extra.get("note"), Some(&Value::from("keep")));
    }
}

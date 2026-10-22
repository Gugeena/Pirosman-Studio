//! OpenTimelineIO (`.otio` JSON) export, written by hand so no Python or C++
//! OTIO runtime is needed.
//!
//! Mapping:
//! - Project → `Timeline.1`; tracks → `Track.1` children of a `Stack.1`
//!   (video tracks bottom-to-top, then audio), as OTIO expects.
//! - Clip → `Clip.2` whose `source_range` starts at the source in-point and
//!   lasts the clip's *timeline* duration; speed/reverse become a
//!   `LinearTimeWarp.1` effect (`time_scalar`, negative when reversed).
//! - Gaps → `Gap.1`; cross dissolves → `Transition.1` (`SMPTE_Dissolve`,
//!   in/out offsets = the handles); dip to black → `Transition.1` with
//!   `Custom_Transition` and `metadata.pirosman.kind`.
//! - Titles → `Clip.2` with a `GeneratorReference.1` (`pirosman.title`).
//! - Markers → `Marker.2` on the stack.
//! - Pirosman-only data (ids, fades, grade, gain, LUT, mixer) rides in
//!   `metadata.pirosman` so nothing is silently lost.

use serde_json::{json, Value};

use crate::model::{dissolve_halves, Clip, MediaKind, Project, TitleClip, Track, TrackKind, TransitionKind};
use crate::time::Rational;

fn rt(frames: i64, rate: Rational) -> Value {
    json!({ "OTIO_SCHEMA": "RationalTime.1", "rate": rate.as_f64(), "value": frames as f64 })
}

fn tr(start: i64, dur: i64, rate: Rational) -> Value {
    json!({ "OTIO_SCHEMA": "TimeRange.1", "start_time": rt(start, rate), "duration": rt(dur, rate) })
}

/// `file://` URL with the characters OTIO consumers choke on percent-encoded.
pub fn file_url(path: &str) -> String {
    let mut p = path.replace('\\', "/");
    if !p.starts_with('/') {
        p = format!("/{p}"); // Windows drive: file:///D:/...
    }
    let mut out = String::from("file://");
    for ch in p.chars() {
        match ch {
            ' ' => out.push_str("%20"),
            '%' => out.push_str("%25"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3F"),
            c => out.push(c),
        }
    }
    out
}

fn gap(len: i64, rate: Rational) -> Value {
    json!({ "OTIO_SCHEMA": "Gap.1", "name": "", "source_range": tr(0, len, rate), "effects": [], "markers": [], "metadata": {} })
}

fn clip_json(p: &Project, c: &Clip, kind: TrackKind) -> Value {
    let rate = p.settings.frame_rate;
    let m = p.media(c.media_id).expect("validated");
    let available = m.duration_frames.map(|d| tr(0, d, rate)).unwrap_or(Value::Null);
    let mut effects = vec![];
    if c.speed != 1.0 || c.reversed {
        let scalar = if c.reversed { -c.speed } else { c.speed };
        effects.push(json!({ "OTIO_SCHEMA": "LinearTimeWarp.1", "name": "", "effect_name": "LinearTimeWarp", "time_scalar": scalar, "metadata": {} }));
    }
    let mut meta = json!({
        "clip_id": c.id,
        "media_id": c.media_id,
        "source_out": c.source_out,
        "link_id": c.link_id,
    });
    if c.fade_in_frames > 0 || c.fade_out_frames > 0 {
        meta["fade_in_frames"] = json!(c.fade_in_frames);
        meta["fade_out_frames"] = json!(c.fade_out_frames);
    }
    if kind == TrackKind::Video && !c.color.is_neutral() {
        meta["color"] = serde_json::to_value(c.color).unwrap();
    }
    if kind == TrackKind::Audio && c.gain_db != 0.0 {
        meta["gain_db"] = json!(c.gain_db);
    }
    if let Some(l) = &c.lut {
        meta["lut"] = json!(l);
    }
    json!({
        "OTIO_SCHEMA": "Clip.2",
        "name": c.name,
        "source_range": tr(c.source_in, c.duration(), rate),
        "media_references": {
            "DEFAULT_MEDIA": {
                "OTIO_SCHEMA": "ExternalReference.1",
                "name": m.name,
                "target_url": file_url(&m.path),
                "available_range": available,
                "available_image_bounds": null,
                "metadata": { "pirosman": { "sha256": m.sha256, "kind": match m.kind { MediaKind::Video => "video", MediaKind::Audio => "audio", MediaKind::Image => "image" } } }
            }
        },
        "active_media_reference_key": "DEFAULT_MEDIA",
        "effects": effects,
        "markers": [],
        "enabled": true,
        "metadata": { "pirosman": meta }
    })
}

fn title_json(x: &TitleClip, rate: Rational) -> Value {
    json!({
        "OTIO_SCHEMA": "Clip.2",
        "name": x.label(),
        "source_range": tr(0, x.duration, rate),
        "media_references": {
            "DEFAULT_MEDIA": {
                "OTIO_SCHEMA": "GeneratorReference.1",
                "name": "Title",
                "generator_kind": "pirosman.title",
                "parameters": { "text": x.text, "font_size": x.font_size, "color": x.color_hex, "position": x.position },
                "available_range": null,
                "available_image_bounds": null,
                "metadata": {}
            }
        },
        "active_media_reference_key": "DEFAULT_MEDIA",
        "effects": [],
        "markers": [],
        "enabled": true,
        "metadata": { "pirosman": { "title_id": x.id, "fade_in_frames": x.fade_in_frames, "fade_out_frames": x.fade_out_frames } }
    })
}

fn track_json(p: &Project, t: &Track) -> Value {
    let rate = p.settings.frame_rate;
    enum Item<'a> {
        C(&'a Clip),
        T(&'a TitleClip),
    }
    let mut items: Vec<(i64, i64, Item)> = t.clips.iter().map(|c| (c.start, c.end(), Item::C(c))).collect();
    items.extend(t.titles.iter().map(|x| (x.start, x.end(), Item::T(x))));
    items.sort_by_key(|i| i.0);
    let mut children = vec![];
    let mut cursor = 0;
    for (s, e, it) in items {
        if s > cursor {
            children.push(gap(s - cursor, rate));
        }
        match it {
            Item::C(c) => {
                if let Some(trn) = c.transition_in {
                    let (lead, tail) = match trn.kind {
                        TransitionKind::CrossDissolve => dissolve_halves(trn.duration_frames),
                        TransitionKind::DipToBlack => (trn.duration_frames - trn.duration_frames / 2, trn.duration_frames / 2),
                    };
                    // OTIO: in_offset reaches back into the outgoing clip, out_offset into the incoming one.
                    let (ttype, kind) = match trn.kind {
                        TransitionKind::CrossDissolve => ("SMPTE_Dissolve", "cross_dissolve"),
                        TransitionKind::DipToBlack => ("Custom_Transition", "dip_to_black"),
                    };
                    children.push(json!({
                        "OTIO_SCHEMA": "Transition.1",
                        "name": "",
                        "transition_type": ttype,
                        "in_offset": rt(tail, rate),
                        "out_offset": rt(lead, rate),
                        "metadata": { "pirosman": { "kind": kind, "duration_frames": trn.duration_frames } }
                    }));
                }
                children.push(clip_json(p, c, t.kind));
            }
            Item::T(x) => children.push(title_json(x, rate)),
        }
        cursor = e;
    }
    let mut meta = json!({ "track_id": t.id, "muted": t.muted, "locked": t.locked });
    if t.kind == TrackKind::Audio {
        meta["mixer"] = json!({ "solo": t.solo, "volume_db": t.volume_db, "pan": t.pan, "eq_low_db": t.eq_low_db, "eq_mid_db": t.eq_mid_db, "eq_high_db": t.eq_high_db });
    }
    json!({
        "OTIO_SCHEMA": "Track.1",
        "name": t.name,
        "kind": if t.kind == TrackKind::Video { "Video" } else { "Audio" },
        "source_range": null,
        "children": children,
        "effects": [],
        "markers": [],
        "enabled": !t.muted,
        "metadata": { "pirosman": meta }
    })
}

fn marker_color(c: &str) -> &'static str {
    match c {
        "red" => "RED",
        "blue" => "BLUE",
        "green" => "GREEN",
        "purple" => "PURPLE",
        _ => "YELLOW",
    }
}

/// Build the OTIO document for a project.
pub fn export_otio(p: &Project) -> Value {
    let rate = p.settings.frame_rate;
    let mut tracks: Vec<Value> = p.tracks.iter().filter(|t| t.kind == TrackKind::Video).map(|t| track_json(p, t)).collect();
    tracks.extend(p.tracks.iter().filter(|t| t.kind == TrackKind::Audio).map(|t| track_json(p, t)));
    let markers: Vec<Value> = p
        .markers
        .iter()
        .map(|m| {
            json!({
                "OTIO_SCHEMA": "Marker.2",
                "name": m.name,
                "color": marker_color(&m.color),
                "marked_range": tr(m.frame, 0, rate),
                "comment": m.note,
                "metadata": { "pirosman": { "marker_id": m.id } }
            })
        })
        .collect();
    json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": p.name,
        "global_start_time": rt(0, rate),
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "name": "tracks",
            "source_range": null,
            "children": tracks,
            "effects": [],
            "markers": markers,
            "enabled": true,
            "metadata": {}
        },
        "metadata": { "pirosman": {
            "project_id": p.id,
            "frame_rate": p.settings.frame_rate.ffmpeg(),
            "resolution": format!("{}x{}", p.settings.width, p.settings.height),
            "mark_in": p.mark_in,
            "mark_out": p.mark_out
        } }
    })
}

pub fn export_otio_string(p: &Project) -> String {
    serde_json::to_string_pretty(&export_otio(p)).expect("OTIO JSON serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixtures::*;
    use crate::model::{Transition, TransitionKind};
    use crate::timeline::*;

    #[test]
    fn timeline_structure_gaps_and_transitions() {
        let (mut p, mid) = project();
        p.media[0].path = "D:\\My Films\\shot #1.mp4".into();
        let v1 = p.tracks[0].id;
        place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 10, source_in: Some(0), source_out: Some(40) }).unwrap();
        let b = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 50, source_in: Some(100), source_out: Some(150) }).unwrap()[0];
        update_clip(&mut p, b, ClipPatch { transition_in: Some(Some(Transition { kind: TransitionKind::CrossDissolve, duration_frames: 10 })), ..Default::default() }).unwrap();
        let v2 = p.tracks[1].id;
        add_title(&mut p, NewTitle { track_id: v2, start: 0, duration: 25, text: "Harbour".into(), font_size: None, color_hex: None, position: None }).unwrap();
        add_marker(&mut p, 30, Some("Beat".into()), Some("red".into())).unwrap();
        let o = export_otio(&p);
        assert_eq!(o["OTIO_SCHEMA"], "Timeline.1");
        let tracks = o["tracks"]["children"].as_array().unwrap();
        assert_eq!(tracks.len(), 4);
        assert_eq!(tracks[0]["kind"], "Video");
        assert_eq!(tracks[2]["kind"], "Audio");
        let v1c = tracks[0]["children"].as_array().unwrap();
        let schemas: Vec<&str> = v1c.iter().map(|c| c["OTIO_SCHEMA"].as_str().unwrap()).collect();
        assert_eq!(schemas, vec!["Gap.1", "Clip.2", "Transition.1", "Clip.2"]);
        assert_eq!(v1c[0]["source_range"]["duration"]["value"], 10.0);
        assert_eq!(v1c[1]["source_range"]["duration"]["value"], 40.0);
        assert_eq!(v1c[2]["transition_type"], "SMPTE_Dissolve");
        assert_eq!((v1c[2]["in_offset"]["value"].as_f64(), v1c[2]["out_offset"]["value"].as_f64()), (Some(5.0), Some(5.0)));
        assert_eq!(v1c[1]["media_references"]["DEFAULT_MEDIA"]["target_url"], "file:///D:/My%20Films/shot%20%231.mp4");
        assert_eq!(tracks[1]["children"][0]["media_references"]["DEFAULT_MEDIA"]["generator_kind"], "pirosman.title");
        assert_eq!(o["tracks"]["markers"][0]["color"], "RED");
        assert_eq!(o["tracks"]["markers"][0]["marked_range"]["start_time"]["value"], 30.0);
        assert_eq!(v1c[1]["source_range"]["start_time"]["rate"], 25.0);
    }

    #[test]
    fn retime_becomes_linear_time_warp() {
        let (mut p, mid) = project();
        let v1 = p.tracks[0].id;
        let c = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(100) }).unwrap()[0];
        update_clip(&mut p, c, ClipPatch { speed: Some(2.0), reversed: Some(true), ..Default::default() }).unwrap();
        let o = export_otio(&p);
        let clip = &o["tracks"]["children"][0]["children"][0];
        assert_eq!(clip["effects"][0]["OTIO_SCHEMA"], "LinearTimeWarp.1");
        assert_eq!(clip["effects"][0]["time_scalar"], -2.0);
        assert_eq!(clip["source_range"]["duration"]["value"], 50.0, "timeline duration");
        // Valid JSON text, parseable back.
        let s = export_otio_string(&p);
        let back: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back, o);
    }

    #[test]
    fn ntsc_rate_is_exact_float() {
        let mut p = project().0;
        p.settings.frame_rate = Rational::new(24000, 1001);
        let o = export_otio(&p);
        assert!((o["global_start_time"]["rate"].as_f64().unwrap() - 23.976_023_976).abs() < 1e-6);
        assert_eq!(file_url("/home/a b/c.mov"), "file:///home/a%20b/c.mov");
    }
}

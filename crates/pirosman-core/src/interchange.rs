//! Timeline import from other editors.
//!
//! - **FCP 7 XML (`xmeml`)**: the documented interchange DaVinci Resolve
//!   exports (`resolve.EXPORT_FCP_7_XML`). Sequences, video/audio tracks,
//!   clip items, file references, links, cross-dissolve transitions.
//! - **OpenTimelineIO (`.otio`)**: what Resolve exports with
//!   `resolve.EXPORT_OTIO`, and what Pirosman itself writes (`otio.rs`).
//! - **`.drp` / `.drt` archives**: Resolve's own project/timeline packages
//!   are ZIP files with an undocumented internal layout. Any embedded `xmeml`
//!   or OTIO document is imported; otherwise only referenced media paths are
//!   recovered (best effort; reported as such).
//!
//! Imports produce a *new* Pirosman project; media is referenced, not
//! copied, and can be probed/hashed afterwards with [`enrich_media`].

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};
use crate::media::{import_media, Tools};
use crate::model::{Clip, Marker, MediaKind, MediaRef, Project, ProjectSettings, Track, TrackKind, Transition, TransitionKind};
use crate::time::Rational;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub project: Project,
    pub format: String,
    pub warnings: Vec<String>,
}

const MEDIA_EXT: [&str; 22] = [
    "mov", "mp4", "m4v", "mxf", "mkv", "avi", "mts", "m2ts", "braw", "r3d", "ari", "dng", "wav", "aif", "aiff", "mp3", "m4a", "png", "jpg", "jpeg", "tif", "exr",
];

fn kind_from_ext(path: &str) -> MediaKind {
    let e = Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match e.as_str() {
        "wav" | "aif" | "aiff" | "mp3" | "m4a" | "flac" => MediaKind::Audio,
        "png" | "jpg" | "jpeg" | "tif" | "tiff" | "exr" | "dng" => MediaKind::Image,
        _ => MediaKind::Video,
    }
}

/// `file://localhost/D:/a%20b.mov` → `D:/a b.mov`.
pub fn url_to_path(url: &str) -> String {
    let s = url.trim();
    let s = s.strip_prefix("file://localhost").or_else(|| s.strip_prefix("file://")).unwrap_or(s);
    let bytes = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let mut p = String::from_utf8_lossy(&out).into_owned();
    // "/D:/x" → "D:/x" on Windows-style paths.
    if p.len() > 3 && p.as_bytes()[0] == b'/' && p.as_bytes()[2] == b':' {
        p.remove(0);
    }
    p
}

fn new_media(name: &str, path: &str, frames: Option<i64>, video: bool, audio: bool) -> MediaRef {
    let kind = if !video && audio { MediaKind::Audio } else if kind_from_ext(path) == MediaKind::Image { MediaKind::Image } else { MediaKind::Video };
    MediaRef {
        id: Uuid::new_v4(),
        name: name.to_string(),
        path: path.to_string(),
        sha256: None,
        size: 0,
        modified: None,
        kind,
        duration_frames: if kind == MediaKind::Image { None } else { frames },
        has_video: video,
        has_audio: audio,
        width: None,
        height: None,
        frame_rate: None,
        video_codec: None,
        audio_codec: None,
        sample_rate: None,
        channels: None,
        container: None,
        extra: Map::new(),
    }
}

/// Builds a project, refusing (with a warning) clips that would overlap.
struct Builder {
    p: Project,
    warnings: Vec<String>,
}

impl Builder {
    fn new(name: &str, settings: ProjectSettings) -> Self {
        let mut p = Project::new(name, settings);
        p.tracks.clear();
        Self { p, warnings: vec![] }
    }
    fn track(&mut self, kind: TrackKind) -> usize {
        let n = self.p.tracks.iter().filter(|t| t.kind == kind).count() + 1;
        let name = format!("{}{n}", if kind == TrackKind::Video { "V" } else { "A" });
        self.p.tracks.push(Track::new(kind, &name));
        self.p.tracks.len() - 1
    }
    fn push(&mut self, ti: usize, c: Clip) {
        let t = &self.p.tracks[ti];
        if t.clips.iter().any(|o| o.start < c.end() && c.start < o.end()) {
            self.warnings.push(format!("skipped overlapping clip {:?} on {}", c.name, t.name));
            return;
        }
        if c.source_out <= c.source_in || c.start < 0 {
            self.warnings.push(format!("skipped clip {:?} with an empty or negative range", c.name));
            return;
        }
        self.p.tracks[ti].clips.push(c);
    }
    fn finish(mut self, format: &str) -> CoreResult<ImportResult> {
        for kind in [TrackKind::Video, TrackKind::Audio] {
            if !self.p.tracks.iter().any(|t| t.kind == kind) {
                self.track(kind);
            }
        }
        // Media lengths must cover every clip that uses them.
        let mut need: HashMap<Uuid, i64> = HashMap::new();
        for c in self.p.tracks.iter().flat_map(|t| &t.clips) {
            let e = need.entry(c.media_id).or_insert(0);
            *e = (*e).max(c.source_out);
        }
        for m in &mut self.p.media {
            if m.kind != MediaKind::Image {
                let n = need.get(&m.id).copied().unwrap_or(1);
                m.duration_frames = Some(m.duration_frames.unwrap_or(n).max(n));
            }
        }
        // Clips must match their track kind's streams.
        for t in &self.p.tracks {
            for c in &t.clips {
                if let Some(m) = self.p.media.iter_mut().find(|m| m.id == c.media_id) {
                    match t.kind {
                        TrackKind::Video => m.has_video = true,
                        TrackKind::Audio => m.has_audio = true,
                    }
                }
            }
        }
        self.p.prune_dangling_transitions();
        self.p.sort_clips();
        // Transitions whose handles the source does not provide become dips.
        if let Err(e) = self.p.validate() {
            for t in &mut self.p.tracks {
                for c in &mut t.clips {
                    if let Some(tr) = &mut c.transition_in {
                        tr.kind = TransitionKind::DipToBlack;
                    }
                }
            }
            self.warnings.push(format!("cross dissolves converted to dips where handles were missing ({e})"));
        }
        self.p.validate()?;
        Ok(ImportResult { project: self.p, format: format.into(), warnings: self.warnings })
    }
}

fn child<'a>(n: roxmltree::Node<'a, 'a>, tag: &str) -> Option<roxmltree::Node<'a, 'a>> {
    n.children().find(|c| c.has_tag_name(tag))
}
fn text_of(n: roxmltree::Node, tag: &str) -> Option<String> {
    child(n, tag).and_then(|c| c.text()).map(|s| s.trim().to_string())
}
fn int_of(n: roxmltree::Node, tag: &str) -> Option<i64> {
    text_of(n, tag).and_then(|s| s.parse::<f64>().ok()).map(|v| v.round() as i64)
}
fn rate_of(n: roxmltree::Node) -> Option<Rational> {
    let r = child(n, "rate")?;
    let tb = int_of(r, "timebase")? as u32;
    let ntsc = text_of(r, "ntsc").map(|s| s.eq_ignore_ascii_case("true")).unwrap_or(false);
    Some(if ntsc { Rational::new(tb * 1000, 1001) } else { Rational::new(tb, 1) })
}

/// Import an FCP 7 XML (`xmeml`) document's first sequence.
pub fn import_xmeml(xml: &str) -> CoreResult<ImportResult> {
    // FCP 7 XML carries <!DOCTYPE xmeml>; roxmltree caps entity expansion.
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(xml, opts).map_err(|e| CoreError::Format(format!("not valid XML: {e}")))?;
    let root = doc.root_element();
    if !root.has_tag_name("xmeml") {
        return Err(CoreError::Format("not an FCP 7 XML (xmeml) document".into()));
    }
    let seq = root.descendants().find(|n| n.has_tag_name("sequence")).ok_or_else(|| CoreError::Format("the XML has no sequence".into()))?;
    let rate = rate_of(seq).unwrap_or(Rational::new(25, 1));
    let media_el = child(seq, "media").ok_or_else(|| CoreError::Format("the sequence has no media".into()))?;
    let (mut w, mut h) = (1920u32, 1080u32);
    if let Some(sc) = child(media_el, "video").and_then(|v| child(v, "format")).and_then(|f| child(f, "samplecharacteristics")) {
        w = int_of(sc, "width").unwrap_or(1920) as u32 / 2 * 2;
        h = int_of(sc, "height").unwrap_or(1080) as u32 / 2 * 2;
    }
    let name = text_of(seq, "name").unwrap_or_else(|| "Imported sequence".into());
    let mut b = Builder::new(&name, ProjectSettings { width: w.max(16), height: h.max(16), frame_rate: rate, ..Default::default() });

    // File definitions may appear once and be referenced by id later.
    let mut files: HashMap<String, Uuid> = HashMap::new();
    for f in seq.descendants().filter(|n| n.has_tag_name("file")) {
        let Some(id) = f.attribute("id") else { continue };
        if files.contains_key(id) {
            continue;
        }
        let Some(url) = text_of(f, "pathurl") else { continue };
        let path = url_to_path(&url);
        let fm = child(f, "media");
        let video = fm.map(|m| child(m, "video").is_some()).unwrap_or(kind_from_ext(&path) != MediaKind::Audio);
        let audio = fm.map(|m| child(m, "audio").is_some()).unwrap_or(kind_from_ext(&path) == MediaKind::Audio);
        let fname = text_of(f, "name").unwrap_or_else(|| Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        let m = new_media(&fname, &path, int_of(f, "duration"), video, audio);
        files.insert(id.to_string(), m.id);
        b.p.media.push(m);
    }

    let mut link_groups: HashMap<String, Uuid> = HashMap::new();
    let mut item_link: Vec<(Uuid, Vec<String>, String)> = vec![];
    for (kind, tag) in [(TrackKind::Video, "video"), (TrackKind::Audio, "audio")] {
        let Some(group) = child(media_el, tag) else { continue };
        for tr in group.children().filter(|n| n.has_tag_name("track")) {
            let ti = b.track(kind);
            let mut pending: Option<(i64, i64)> = None;
            for it in tr.children().filter(|n| n.is_element()) {
                if it.has_tag_name("transitionitem") {
                    if let (Some(s), Some(e)) = (int_of(it, "start"), int_of(it, "end")) {
                        pending = Some((s, e));
                    }
                    continue;
                }
                if !it.has_tag_name("clipitem") {
                    continue;
                }
                if text_of(it, "enabled").map(|s| s.eq_ignore_ascii_case("false")).unwrap_or(false) {
                    continue;
                }
                let Some(file_id) = child(it, "file").and_then(|f| f.attribute("id")) else {
                    b.warnings.push("skipped a clip item without a file reference (generator or nested sequence)".into());
                    continue;
                };
                let Some(&mid) = files.get(file_id) else {
                    b.warnings.push(format!("skipped a clip referencing unknown file {file_id}"));
                    continue;
                };
                let (mut start, mut end) = (int_of(it, "start").unwrap_or(0), int_of(it, "end").unwrap_or(0));
                let (sin, sout) = (int_of(it, "in").unwrap_or(0), int_of(it, "out").unwrap_or(0));
                let mut trans = None;
                if start < 0 {
                    if let Some((ts, te)) = pending {
                        start = (ts + te) / 2;
                        trans = Some(Transition { kind: TransitionKind::CrossDissolve, duration_frames: (te - ts).max(2) });
                    }
                }
                if end < 0 {
                    end = start + (sout - sin);
                }
                pending = None;
                let mname = text_of(it, "name").unwrap_or_else(|| "clip".into());
                let mut c = Clip::new(mid, &mname, start, sin, sin + (end - start));
                c.transition_in = trans;
                let links: Vec<String> = it.descendants().filter(|n| n.has_tag_name("linkclipref")).filter_map(|n| n.text().map(|s| s.trim().to_string())).collect();
                if let Some(own) = it.attribute("id") {
                    item_link.push((c.id, links, own.to_string()));
                }
                b.push(ti, c);
            }
        }
    }
    // Clip items that reference each other share a link id.
    for (cid, refs, own) in &item_link {
        if refs.len() < 2 {
            continue;
        }
        let key = {
            let mut r = refs.clone();
            r.push(own.clone());
            r.sort();
            r.dedup();
            r.join("|")
        };
        let lid = *link_groups.entry(key).or_insert_with(Uuid::new_v4);
        for t in &mut b.p.tracks {
            if let Some(c) = t.clips.iter_mut().find(|c| c.id == *cid) {
                c.link_id = Some(lid);
            }
        }
    }
    b.finish("FCP 7 XML")
}

fn rt_frames(v: &Value, rate: Rational) -> i64 {
    let value = v["value"].as_f64().unwrap_or(0.0);
    let r = v["rate"].as_f64().unwrap_or(rate.as_f64());
    (value / r * rate.as_f64()).round() as i64
}

/// Import an OpenTimelineIO JSON document.
pub fn import_otio(text: &str) -> CoreResult<ImportResult> {
    let v: Value = serde_json::from_str(text).map_err(|e| CoreError::Format(format!("not valid OTIO JSON: {e}")))?;
    if !v["OTIO_SCHEMA"].as_str().unwrap_or("").starts_with("Timeline.") {
        return Err(CoreError::Format("not an OTIO Timeline".into()));
    }
    let fr = v["global_start_time"]["rate"]
        .as_f64()
        .or_else(|| v["tracks"]["children"][0]["children"].as_array().and_then(|c| c.iter().find_map(|x| x["source_range"]["duration"]["rate"].as_f64())))
        .unwrap_or(25.0);
    let rate = if (fr - fr.round()).abs() < 1e-6 { Rational::new(fr.round() as u32, 1) } else { Rational::new((fr * 1001.0).round() as u32, 1001).reduced() };
    let meta = &v["metadata"]["pirosman"];
    let (mut w, mut h) = (1920, 1080);
    if let Some((a, b)) = meta["resolution"].as_str().and_then(|r| r.split_once('x')) {
        w = a.parse().unwrap_or(1920);
        h = b.parse().unwrap_or(1080);
    }
    let name = v["name"].as_str().unwrap_or("Imported OTIO").to_string();
    let mut b = Builder::new(&name, ProjectSettings { width: w, height: h, frame_rate: rate, ..Default::default() });
    let mut media_by_url: HashMap<String, Uuid> = HashMap::new();
    for t in v["tracks"]["children"].as_array().cloned().unwrap_or_default() {
        let kind = if t["kind"].as_str() == Some("Audio") { TrackKind::Audio } else { TrackKind::Video };
        let ti = b.track(kind);
        let mut cursor = 0i64;
        let mut pending: Option<(i64, i64)> = None;
        for item in t["children"].as_array().cloned().unwrap_or_default() {
            let schema = item["OTIO_SCHEMA"].as_str().unwrap_or("");
            let dur = rt_frames(&item["source_range"]["duration"], rate);
            if schema.starts_with("Gap.") {
                cursor += dur;
                continue;
            }
            if schema.starts_with("Transition.") {
                pending = Some((rt_frames(&item["in_offset"], rate), rt_frames(&item["out_offset"], rate)));
                continue;
            }
            if !schema.starts_with("Clip.") {
                cursor += dur;
                continue;
            }
            let mref = if item["media_references"].is_object() {
                let key = item["active_media_reference_key"].as_str().unwrap_or("DEFAULT_MEDIA");
                item["media_references"][key].clone()
            } else {
                item["media_reference"].clone()
            };
            let Some(url) = mref["target_url"].as_str() else {
                b.warnings.push(format!("skipped {:?}: generator or missing media reference", item["name"].as_str().unwrap_or("clip")));
                cursor += dur;
                pending = None;
                continue;
            };
            let path = url_to_path(url);
            let mid = *media_by_url.entry(path.clone()).or_insert_with(|| {
                let avail = (!mref["available_range"].is_null()).then(|| rt_frames(&mref["available_range"]["duration"], rate));
                let nm = mref["name"].as_str().map(String::from).unwrap_or_else(|| Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                let m = new_media(&nm, &path, avail, kind == TrackKind::Video, kind == TrackKind::Audio);
                let id = m.id;
                b.p.media.push(m);
                id
            });
            let sin = rt_frames(&item["source_range"]["start_time"], rate);
            let mut speed = 1.0;
            let mut reversed = false;
            for fx in item["effects"].as_array().cloned().unwrap_or_default() {
                if fx["OTIO_SCHEMA"].as_str().unwrap_or("").starts_with("LinearTimeWarp") {
                    let s = fx["time_scalar"].as_f64().unwrap_or(1.0);
                    reversed = s < 0.0;
                    speed = s.abs().clamp(0.25, 4.0);
                }
            }
            let src_len = ((dur as f64) * speed).round() as i64;
            let mut c = Clip::new(mid, item["name"].as_str().unwrap_or("clip"), cursor, sin, sin + src_len.max(1));
            c.speed = speed;
            c.reversed = reversed;
            if let Some((tail, lead)) = pending.take() {
                let kind = if t["children"].as_array().map(|_| true).unwrap_or(false) && lead + tail > 0 {
                    TransitionKind::CrossDissolve
                } else {
                    TransitionKind::DipToBlack
                };
                c.transition_in = Some(Transition { kind, duration_frames: (lead + tail).max(2) });
            }
            if let Some(m) = item["metadata"]["pirosman"].as_object() {
                c.fade_in_frames = m.get("fade_in_frames").and_then(Value::as_i64).unwrap_or(0);
                c.fade_out_frames = m.get("fade_out_frames").and_then(Value::as_i64).unwrap_or(0);
                if let Some(l) = m.get("link_id").and_then(Value::as_str).and_then(|s| Uuid::parse_str(s).ok()) {
                    c.link_id = Some(l);
                }
            }
            cursor += dur;
            b.push(ti, c);
        }
    }
    for m in v["tracks"]["markers"].as_array().cloned().unwrap_or_default() {
        let color = m["color"].as_str().unwrap_or("YELLOW").to_lowercase();
        b.p.markers.push(Marker {
            id: Uuid::new_v4(),
            frame: rt_frames(&m["marked_range"]["start_time"], rate),
            name: m["name"].as_str().filter(|s| !s.is_empty()).unwrap_or("Marker").to_string(),
            color: if crate::model::MARKER_COLORS.contains(&color.as_str()) { color } else { "yellow".into() },
            note: m["comment"].as_str().unwrap_or("").to_string(),
            extra: Map::new(),
        });
    }
    b.finish("OpenTimelineIO")
}

/// Import a Resolve `.drp`/`.drt` archive (ZIP). Embedded xmeml or OTIO is
/// imported fully; otherwise referenced media paths are recovered.
pub fn import_resolve_archive(path: &Path) -> CoreResult<ImportResult> {
    let f = std::fs::File::open(path)?;
    let mut za = zip::ZipArchive::new(f).map_err(|e| CoreError::Format(format!("{} is not a ZIP-based Resolve archive: {e}", path.display())))?;
    let mut texts = vec![];
    for i in 0..za.len() {
        let mut e = za.by_index(i).map_err(|e| CoreError::Format(e.to_string()))?;
        if e.is_dir() || e.size() > 256 << 20 {
            continue;
        }
        let name = e.name().to_string();
        let mut s = String::new();
        if e.read_to_string(&mut s).is_ok() {
            texts.push((name, s));
        }
    }
    for (name, s) in &texts {
        if s.contains("<xmeml") {
            let mut r = import_xmeml(s)?;
            r.format = format!("Resolve archive ({name}, FCP 7 XML)");
            return Ok(r);
        }
        if s.contains("\"OTIO_SCHEMA\"") {
            let mut r = import_otio(s)?;
            r.format = format!("Resolve archive ({name}, OTIO)");
            return Ok(r);
        }
    }
    // Proprietary layout: recover media references only.
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Resolve project".into());
    let mut b = Builder::new(&stem, ProjectSettings::default());
    let mut seen = std::collections::HashSet::new();
    for (_, s) in &texts {
        for tok in s.split(['<', '>', '"', '\'', '\n', '\r', '\t']) {
            let t = tok.trim();
            let looks_path = (t.len() > 4) && (t.contains('/') || t.contains('\\')) && t.rsplit('.').next().map(|e| MEDIA_EXT.contains(&e.to_lowercase().as_str())).unwrap_or(false);
            if looks_path && seen.insert(t.to_string()) {
                let p = url_to_path(t);
                let nm = Path::new(&p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let k = kind_from_ext(&p);
                b.p.media.push(new_media(&nm, &p, Some(1), k != MediaKind::Audio, k == MediaKind::Audio));
            }
        }
    }
    b.warnings.push(format!(
        "{} uses Resolve's undocumented internal layout: recovered {} media reference(s) but no timeline. Export the timeline from Resolve as FCP 7 XML or OTIO for a full import.",
        path.display(),
        b.p.media.len()
    ));
    b.finish("Resolve archive (media only)")
}

/// Import by file extension.
pub fn import_timeline_file(path: &Path) -> CoreResult<ImportResult> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "drp" | "drt" => import_resolve_archive(path),
        "otio" => import_otio(&std::fs::read_to_string(path)?),
        "xml" => import_xmeml(&std::fs::read_to_string(path)?),
        _ => Err(CoreError::Invalid("choose an FCP 7 .xml, .otio, .drp or .drt file".into())),
    }
}

/// Probe and hash every imported media file that exists on disk, keeping
/// media ids. Files that are missing stay as offline references.
pub fn enrich_media(tools: &Tools, p: &mut Project) -> Vec<String> {
    let mut notes = vec![];
    let rate = p.settings.frame_rate;
    for m in &mut p.media {
        let path = Path::new(&m.path);
        if !path.is_file() {
            notes.push(format!("{} is offline at {}", m.name, m.path));
            continue;
        }
        match import_media(tools, path, rate) {
            Ok(fresh) => {
                let keep = m.duration_frames;
                let id = m.id;
                *m = MediaRef { id, ..fresh };
                if let (Some(k), Some(d)) = (keep, m.duration_frames) {
                    if d < k {
                        notes.push(format!("{} is shorter on disk ({d} frames) than the timeline uses ({k})", m.name));
                        m.duration_frames = Some(k);
                    }
                }
            }
            Err(e) => notes.push(format!("{}: {e}", m.name)),
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    pub const XMEML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE xmeml>
<xmeml version="5">
  <sequence id="sequence-1">
    <name>Harbour Cut</name>
    <rate><timebase>24</timebase><ntsc>TRUE</ntsc></rate>
    <media>
      <video>
        <format><samplecharacteristics><width>1920</width><height>1080</height></samplecharacteristics></format>
        <track>
          <clipitem id="clipitem-1">
            <name>intro.mov</name><enabled>TRUE</enabled>
            <start>0</start><end>48</end><in>10</in><out>58</out>
            <file id="file-1"><name>intro.mov</name><pathurl>file://localhost/D:/Footage/My%20Shoot/intro.mov</pathurl><duration>240</duration>
              <media><video/><audio/></media></file>
            <link><linkclipref>clipitem-1</linkclipref></link><link><linkclipref>clipitem-3</linkclipref></link>
          </clipitem>
          <transitionitem><start>40</start><end>56</end><alignment>center</alignment><effect><name>Cross Dissolve</name></effect></transitionitem>
          <clipitem id="clipitem-2">
            <name>ridge.mov</name><start>-1</start><end>100</end><in>30</in><out>82</out>
            <file id="file-2"><name>ridge.mov</name><pathurl>file:///D:/Footage/ridge.mov</pathurl><duration>300</duration><media><video/></media></file>
          </clipitem>
        </track>
      </video>
      <audio>
        <track>
          <clipitem id="clipitem-3">
            <name>intro.mov</name><start>0</start><end>48</end><in>10</in><out>58</out>
            <file id="file-1"/>
            <link><linkclipref>clipitem-1</linkclipref></link><link><linkclipref>clipitem-3</linkclipref></link>
          </clipitem>
        </track>
      </audio>
    </media>
  </sequence>
</xmeml>"#;

    #[test]
    fn imports_fcp7_xml() {
        let r = import_xmeml(XMEML).unwrap();
        let p = &r.project;
        assert_eq!(p.name, "Harbour Cut");
        assert_eq!(p.settings.frame_rate, Rational::new(24000, 1001));
        assert_eq!(p.media.len(), 2);
        assert_eq!(p.media[0].path, "D:/Footage/My Shoot/intro.mov");
        let v = &p.tracks.iter().find(|t| t.kind == TrackKind::Video).unwrap().clips;
        assert_eq!((v[0].start, v[0].source_in, v[0].source_out), (0, 10, 58));
        assert_eq!(v[1].start, 48, "clip entering a centred 16f dissolve starts mid-transition");
        assert_eq!(v[1].transition_in.unwrap().duration_frames, 16);
        let a = &p.tracks.iter().find(|t| t.kind == TrackKind::Audio).unwrap().clips;
        assert_eq!(a.len(), 1);
        assert!(v[0].link_id.is_some() && v[0].link_id == a[0].link_id, "linked A/V via linkclipref");
        assert!(import_xmeml("<fcpxml/>").is_err());
        assert!(import_xmeml("not xml").is_err());
    }

    #[test]
    fn otio_round_trip_through_our_exporter() {
        let r = import_xmeml(XMEML).unwrap();
        let mut p = r.project;
        p.markers.push(Marker { id: Uuid::new_v4(), frame: 12, name: "Beat".into(), color: "red".into(), note: "drums".into(), extra: Map::new() });
        let text = crate::otio::export_otio_string(&p);
        let back = import_otio(&text).unwrap().project;
        let spans = |p: &Project| -> Vec<(i64, i64, i64)> { p.tracks.iter().flat_map(|t| t.clips.iter().map(|c| (c.start, c.source_in, c.end()))).collect() };
        assert_eq!(spans(&back), spans(&p));
        assert_eq!(back.settings.frame_rate, p.settings.frame_rate);
        assert_eq!(back.markers[0].name, "Beat");
        assert_eq!(back.markers[0].color, "red");
        assert!(back.tracks.iter().flat_map(|t| &t.clips).any(|c| c.transition_in.is_some()));
    }

    #[test]
    fn resolve_archive_with_embedded_xml_and_media_only_fallback() {
        let d = tempfile::tempdir().unwrap();
        let drp = d.path().join("Harbour.drp");
        {
            let mut zw = zip::ZipWriter::new(std::fs::File::create(&drp).unwrap());
            zw.start_file("SeqContainer/timeline.xml", zip::write::SimpleFileOptions::default()).unwrap();
            zw.write_all(XMEML.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        let r = import_timeline_file(&drp).unwrap();
        assert!(r.format.contains("FCP 7 XML"));
        assert_eq!(r.project.tracks.iter().map(|t| t.clips.len()).sum::<usize>(), 3);
        let drt = d.path().join("Opaque.drt");
        {
            let mut zw = zip::ZipWriter::new(std::fs::File::create(&drt).unwrap());
            zw.start_file("project.xml", zip::write::SimpleFileOptions::default()).unwrap();
            zw.write_all(b"<Project><Media><Path>D:\\Footage\\a.mov</Path><Path>D:\\Footage\\b.wav</Path></Media></Project>").unwrap();
            zw.finish().unwrap();
        }
        let r = import_timeline_file(&drt).unwrap();
        assert_eq!(r.project.media.len(), 2);
        assert!(r.warnings[0].contains("undocumented"));
        std::fs::write(d.path().join("x.mov"), b"x").unwrap();
        assert!(import_timeline_file(&d.path().join("x.mov")).is_err());
    }

    #[test]
    fn url_decoding() {
        assert_eq!(url_to_path("file://localhost/C:/a%20b/c.mov"), "C:/a b/c.mov");
        assert_eq!(url_to_path("file:///home/x/y.mov"), "/home/x/y.mov");
    }
}

//! Timeline editing operations.
//!
//! Every operation is transactional: it mutates a clone, drops transitions
//! whose cut no longer exists, validates the whole project, and only then
//! replaces the caller's project. A failed edit leaves the project untouched.
//! Clips that share a `link_id` (picture + sync sound) are moved, trimmed,
//! split, slipped, slid, rolled, retimed and deleted together.

use serde::{Deserialize, Serialize};
use serde_json::Map;
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};
use crate::model::{Clip, ColorParams, Crop, Marker, MediaKind, OfxEffect, Project, TitleClip, TitlePosition, Track, TrackKind, Transform, Transition, Vfx, MARKER_COLORS};

fn transact<T>(p: &mut Project, f: impl FnOnce(&mut Project) -> CoreResult<T>) -> CoreResult<T> {
    let mut q = p.clone();
    let out = f(&mut q)?;
    q.prune_dangling_transitions();
    q.sort_clips();
    q.validate()?;
    *p = q;
    Ok(out)
}

fn ensure_unlocked(p: &Project, track_idx: usize) -> CoreResult<()> {
    let t = &p.tracks[track_idx];
    if t.locked {
        Err(CoreError::Locked(t.name.clone()))
    } else {
        Ok(())
    }
}

/// All clip ids in the link group of `clip_id` (including itself).
pub fn link_group(p: &Project, clip_id: Uuid) -> Vec<Uuid> {
    let Some((_, c)) = p.clip(clip_id) else { return vec![] };
    match c.link_id {
        None => vec![clip_id],
        Some(l) => p.tracks.iter().flat_map(|t| &t.clips).filter(|c| c.link_id == Some(l)).map(|c| c.id).collect(),
    }
}

fn locate(p: &Project, clip_id: Uuid) -> CoreResult<(usize, usize)> {
    p.find_clip(clip_id).ok_or_else(|| CoreError::NotFound(format!("clip {clip_id}")))
}

fn track_index(p: &Project, track_id: Uuid) -> CoreResult<usize> {
    p.tracks.iter().position(|t| t.id == track_id).ok_or_else(|| CoreError::NotFound(format!("track {track_id}")))
}

fn next_track_name(p: &Project, kind: TrackKind) -> String {
    let prefix = if kind == TrackKind::Video { "V" } else { "A" };
    let mut n = 1;
    loop {
        let name = format!("{prefix}{n}");
        if !p.tracks.iter().any(|t| t.name == name) {
            return name;
        }
        n += 1;
    }
}

fn range_free(t: &Track, start: i64, end: i64, ignore: &[Uuid]) -> bool {
    t.clips.iter().filter(|c| !ignore.contains(&c.id)).all(|c| c.end() <= start || c.start >= end)
        && t.titles.iter().filter(|c| !ignore.contains(&c.id)).all(|c| c.end() <= start || c.start >= end)
}

/// Source range covering the timeline sub-range `[t0, t1)` of a clip
/// (offsets from the clip's start), honouring speed and reverse.
pub fn sub_range(c: &Clip, t0: i64, t1: i64) -> (i64, i64) {
    let dur = c.duration();
    let sf = |t: i64| if t >= dur { c.source_len() } else { c.source_frames_for(t).min(c.source_len()) };
    if c.reversed {
        (c.source_out - sf(t1), c.source_out - sf(t0))
    } else {
        (c.source_in + sf(t0), c.source_in + sf(t1))
    }
}

/// Default length used when placing a still image.
pub fn still_default_frames(p: &Project) -> i64 {
    5 * p.settings.frame_rate.num as i64 / p.settings.frame_rate.den as i64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceClip {
    pub media_id: Uuid,
    pub track_id: Uuid,
    pub start: i64,
    pub source_in: Option<i64>,
    pub source_out: Option<i64>,
}

fn placed_len(q: &Project, req: &PlaceClip) -> CoreResult<i64> {
    let m = q.media(req.media_id).ok_or_else(|| CoreError::NotFound(format!("media {}", req.media_id)))?;
    let full = m.duration_frames.unwrap_or_else(|| still_default_frames(q));
    Ok(req.source_out.unwrap_or(full) - req.source_in.unwrap_or(0))
}

fn place_inner(q: &mut Project, req: &PlaceClip) -> CoreResult<Vec<Uuid>> {
    let m = q.media(req.media_id).ok_or_else(|| CoreError::NotFound(format!("media {}", req.media_id)))?.clone();
    let ti = track_index(q, req.track_id)?;
    ensure_unlocked(q, ti)?;
    let full = m.duration_frames.unwrap_or_else(|| still_default_frames(q));
    let si = req.source_in.unwrap_or(0);
    let so = req.source_out.unwrap_or(full);
    let kind = q.tracks[ti].kind;
    let link = (kind == TrackKind::Video && m.has_audio && m.kind == MediaKind::Video).then(Uuid::new_v4);
    let mut clip = Clip::new(m.id, &m.name, req.start, si, so);
    clip.link_id = link;
    let (start, end) = (clip.start, clip.end());
    if !range_free(&q.tracks[ti], start, end, &[]) {
        return Err(CoreError::Conflict(format!(
            "track {} already has something in that range; use Overwrite (F10) or Insert (F9)",
            q.tracks[ti].name
        )));
    }
    let mut ids = vec![clip.id];
    q.tracks[ti].clips.push(clip.clone());
    if let Some(link) = link {
        let audio_ti = match q.tracks.iter().position(|t| t.kind == TrackKind::Audio && !t.locked && range_free(t, start, end, &[])) {
            Some(i) => i,
            None => {
                let name = next_track_name(q, TrackKind::Audio);
                q.tracks.push(Track::new(TrackKind::Audio, &name));
                q.tracks.len() - 1
            }
        };
        let a = Clip { id: Uuid::new_v4(), link_id: Some(link), ..clip };
        ids.push(a.id);
        q.tracks[audio_ti].clips.push(a);
    }
    Ok(ids)
}

/// Place media into free space on a track. Video media with audio on a video
/// track also gets a linked sync-audio clip on the first audio track with
/// room (a new audio track is created if none has room). Returns new clip ids.
pub fn place_clip(p: &mut Project, req: PlaceClip) -> CoreResult<Vec<Uuid>> {
    transact(p, |q| place_inner(q, &req))
}

/// Remove everything on track `ti` inside `[a, b)`: clips and titles inside
/// are deleted, those crossing an edge are trimmed, one spanning the whole
/// range is split around it.
fn clear_range(q: &mut Project, ti: usize, a: i64, b: i64) {
    let t = &mut q.tracks[ti];
    let mut keep: Vec<Clip> = vec![];
    for c in t.clips.drain(..) {
        let (s, e) = (c.start, c.end());
        if e <= a || s >= b {
            keep.push(c);
            continue;
        }
        if s < a {
            let (si, so) = sub_range(&c, 0, a - s);
            keep.push(Clip { source_in: si, source_out: so, fade_out_frames: 0, ..c.clone() });
        }
        if e > b {
            let (si, so) = sub_range(&c, b - s, e - s);
            let right = Clip {
                id: if s < a { Uuid::new_v4() } else { c.id },
                start: b,
                source_in: si,
                source_out: so,
                fade_in_frames: 0,
                transition_in: None,
                ..c.clone()
            };
            keep.push(right);
        }
    }
    t.clips = keep;
    let mut titles = vec![];
    for x in t.titles.drain(..) {
        let (s, e) = (x.start, x.end());
        if e <= a || s >= b {
            titles.push(x);
            continue;
        }
        if s < a {
            titles.push(TitleClip { duration: a - s, fade_out_frames: 0, ..x.clone() });
        }
        if e > b {
            titles.push(TitleClip { id: if s < a { Uuid::new_v4() } else { x.id }, start: b, duration: e - b, fade_in_frames: 0, ..x.clone() });
        }
    }
    t.titles = titles;
    // Fades that no longer fit the trimmed length are clamped.
    for c in &mut t.clips {
        let d = c.duration();
        c.fade_in_frames = c.fade_in_frames.min(d);
        c.fade_out_frames = c.fade_out_frames.min(d - c.fade_in_frames);
    }
}

/// Overwrite: the placed range replaces whatever is on the target track (and
/// on the audio track receiving its sync sound). Clips crossing the edges are
/// trimmed, linked partners of affected clips are cleared over the same range.
pub fn overwrite_edit(p: &mut Project, req: PlaceClip) -> CoreResult<Vec<Uuid>> {
    transact(p, |q| {
        let ti = track_index(q, req.track_id)?;
        ensure_unlocked(q, ti)?;
        let len = placed_len(q, &req)?;
        let (a, b) = (req.start, req.start + len);
        let mut tracks = vec![ti];
        let m = q.media(req.media_id).cloned().unwrap();
        if q.tracks[ti].kind == TrackKind::Video && m.has_audio && m.kind == MediaKind::Video {
            if let Some(ai) = q.tracks.iter().position(|t| t.kind == TrackKind::Audio && !t.locked) {
                tracks.push(ai);
            }
        }
        // Partners of any linked clip touched on those tracks are cleared too.
        let mut i = 0;
        while i < tracks.len() {
            let tix = tracks[i];
            let links: Vec<Uuid> = q.tracks[tix].clips.iter().filter(|c| c.start < b && c.end() > a).filter_map(|c| c.link_id).collect();
            for (oi, t) in q.tracks.iter().enumerate() {
                if !tracks.contains(&oi) && !t.locked && t.clips.iter().any(|c| c.link_id.map(|l| links.contains(&l)).unwrap_or(false)) {
                    tracks.push(oi);
                }
            }
            i += 1;
        }
        for t in tracks {
            clear_range(q, t, a, b);
        }
        relink_split_halves(q, b);
        place_inner(q, &req)
    })
}

/// Insert: everything on unlocked tracks from the insert frame onward moves
/// right by the new clip's length (clips spanning that frame are split first),
/// then the clip is placed in the opened gap.
pub fn insert_edit(p: &mut Project, req: PlaceClip) -> CoreResult<Vec<Uuid>> {
    transact(p, |q| {
        let ti = track_index(q, req.track_id)?;
        ensure_unlocked(q, ti)?;
        let len = placed_len(q, &req)?;
        let at = req.start;
        for t in q.tracks.iter_mut().filter(|t| !t.locked) {
            let mut out: Vec<Clip> = vec![];
            for c in t.clips.drain(..) {
                if c.start < at && c.end() > at {
                    let off = at - c.start;
                    let (l0, l1) = sub_range(&c, 0, off);
                    let (r0, r1) = sub_range(&c, off, c.duration());
                    out.push(Clip { source_in: l0, source_out: l1, fade_out_frames: 0, ..c.clone() });
                    out.push(Clip { id: Uuid::new_v4(), start: at + len, source_in: r0, source_out: r1, fade_in_frames: 0, transition_in: None, ..c.clone() });
                } else if c.start >= at {
                    out.push(Clip { start: c.start + len, ..c });
                } else {
                    out.push(c);
                }
            }
            t.clips = out;
            for x in &mut t.titles {
                if x.start >= at {
                    x.start += len;
                } else if x.end() > at {
                    x.duration += len; // a title across the insert point stretches over it
                }
            }
        }
        // Halves of split linked clips get fresh shared link ids.
        relink_split_halves(q, at + len);
        for c in q.tracks.iter_mut().flat_map(|t| t.clips.iter_mut()) {
            let d = c.duration();
            c.fade_in_frames = c.fade_in_frames.min(d);
            c.fade_out_frames = c.fade_out_frames.min(d - c.fade_in_frames);
        }
        place_inner(q, &req)
    })
}

/// After a split at `at`, right halves that still carry their left half's link
/// id get a new link id shared among themselves.
fn relink_split_halves(q: &mut Project, at: i64) {
    let mut remap: std::collections::HashMap<Uuid, Uuid> = std::collections::HashMap::new();
    let lefts: std::collections::HashSet<Uuid> = q.tracks.iter().flat_map(|t| &t.clips).filter(|c| c.end() < at || c.start < at).filter_map(|c| c.link_id).collect();
    for c in q.tracks.iter_mut().flat_map(|t| t.clips.iter_mut()) {
        if c.start == at && c.transition_in.is_none() {
            if let Some(l) = c.link_id {
                if lefts.contains(&l) {
                    let n = *remap.entry(l).or_insert_with(Uuid::new_v4);
                    c.link_id = Some(n);
                }
            }
        }
    }
}

/// Move a clip to `start` on `track_id` (same kind). Linked clips shift by the
/// same delta on their own tracks. Titles move with `move_title`.
pub fn move_clip(p: &mut Project, clip_id: Uuid, track_id: Uuid, start: i64) -> CoreResult<()> {
    if p.find_title(clip_id).is_some() {
        return move_title(p, clip_id, track_id, start);
    }
    transact(p, |q| {
        let (ti, ci) = locate(q, clip_id)?;
        let delta = start - q.tracks[ti].clips[ci].start;
        let dest = track_index(q, track_id)?;
        if q.tracks[dest].kind != q.tracks[ti].kind {
            return Err(CoreError::Invalid("a clip can only move to a track of the same kind".into()));
        }
        ensure_unlocked(q, ti)?;
        ensure_unlocked(q, dest)?;
        for id in link_group(q, clip_id) {
            let (lti, lci) = locate(q, id)?;
            ensure_unlocked(q, lti)?;
            q.tracks[lti].clips[lci].start += delta;
        }
        if dest != ti {
            let (ti, ci) = locate(q, clip_id)?;
            let c = q.tracks[ti].clips.remove(ci);
            q.tracks[dest].clips.push(c);
        }
        Ok(())
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    In,
    Out,
}

/// Trim a clip edge to a new source frame. Trimming the in-point keeps the
/// clip's end fixed on the timeline. Linked clips receive the same source delta.
/// On a reversed clip the timeline head shows the source out-point, so `In`
/// and `Out` still name the timeline edges: the head edge moves `source_out`.
pub fn trim_clip(p: &mut Project, clip_id: Uuid, edge: Edge, new_source_frame: i64) -> CoreResult<()> {
    if let Some((ti, xi)) = p.find_title(clip_id) {
        // Titles have no source: `new_source_frame` is the new timeline edge.
        return transact(p, |q| {
            ensure_unlocked(q, ti)?;
            let x = &mut q.tracks[ti].titles[xi];
            match edge {
                Edge::In => {
                    let end = x.end();
                    x.start = new_source_frame;
                    x.duration = end - new_source_frame;
                }
                Edge::Out => x.duration = new_source_frame - x.start,
            }
            Ok(())
        });
    }
    transact(p, |q| {
        let (ti, ci) = locate(q, clip_id)?;
        let c = &q.tracks[ti].clips[ci];
        let delta = match (edge, c.reversed) {
            (Edge::In, false) => new_source_frame - c.source_in,
            (Edge::In, true) => new_source_frame - c.source_out,
            (Edge::Out, false) => new_source_frame - c.source_out,
            (Edge::Out, true) => new_source_frame - c.source_in,
        };
        for id in link_group(q, clip_id) {
            let (lti, lci) = locate(q, id)?;
            ensure_unlocked(q, lti)?;
            let c = &mut q.tracks[lti].clips[lci];
            let end = c.end();
            match (edge, c.reversed) {
                (Edge::In, false) => c.source_in += delta,
                (Edge::In, true) => c.source_out += delta,
                (Edge::Out, false) => c.source_out += delta,
                (Edge::Out, true) => c.source_in += delta,
            }
            if edge == Edge::In {
                c.start = end - c.duration();
            }
            if c.source_out <= c.source_in {
                return Err(CoreError::Invalid(format!("trim would leave {:?} empty", c.name)));
            }
            let d = c.duration();
            c.fade_in_frames = c.fade_in_frames.min(d);
            c.fade_out_frames = c.fade_out_frames.min(d - c.fade_in_frames);
        }
        Ok(())
    })
}

/// Slip: shift which source frames a clip shows by `delta` source frames
/// without moving it on the timeline or changing its length.
pub fn slip_clip(p: &mut Project, clip_id: Uuid, delta: i64) -> CoreResult<()> {
    transact(p, |q| {
        if delta == 0 {
            return Ok(());
        }
        for id in link_group(q, clip_id) {
            let (ti, ci) = locate(q, id)?;
            ensure_unlocked(q, ti)?;
            let still = q.media(q.tracks[ti].clips[ci].media_id).map(|m| m.kind == MediaKind::Image).unwrap_or(false);
            if still {
                return Err(CoreError::Invalid("a still image has nothing to slip".into()));
            }
            let c = &mut q.tracks[ti].clips[ci];
            c.source_in += delta;
            c.source_out += delta;
            if c.source_in < 0 {
                return Err(CoreError::Invalid(format!("slipping {:?} by {delta} runs before the media's first frame", c.name)));
            }
        }
        Ok(())
    })
}

/// Slide: move a clip by `delta` timeline frames between its neighbours; the
/// clip touching its head gets a longer/shorter tail and the clip touching its
/// tail starts later/earlier, so nothing else on the track moves.
pub fn slide_clip(p: &mut Project, clip_id: Uuid, delta: i64) -> CoreResult<()> {
    transact(p, |q| {
        if delta == 0 {
            return Ok(());
        }
        for id in link_group(q, clip_id) {
            let (ti, ci) = locate(q, id)?;
            ensure_unlocked(q, ti)?;
            let (s, e) = (q.tracks[ti].clips[ci].start, q.tracks[ti].clips[ci].end());
            let t = &mut q.tracks[ti];
            if let Some(prev) = t.clips.iter_mut().find(|o| o.id != id && o.end() == s) {
                if prev.speed != 1.0 || prev.reversed {
                    return Err(CoreError::Invalid("slide needs 1x, forward neighbours".into()));
                }
                prev.source_out += delta;
            }
            if let Some(next) = t.clips.iter_mut().find(|o| o.id != id && o.start == e) {
                if next.speed != 1.0 || next.reversed {
                    return Err(CoreError::Invalid("slide needs 1x, forward neighbours".into()));
                }
                next.start += delta;
                next.source_in += delta;
            }
            t.clips.iter_mut().find(|o| o.id == id).unwrap().start += delta;
        }
        Ok(())
    })
}

/// Roll: move the cut between `left_id` and the clip touching its tail by
/// `delta` frames; the total length of the pair is unchanged.
pub fn roll_edit(p: &mut Project, left_id: Uuid, delta: i64) -> CoreResult<()> {
    transact(p, |q| {
        let mut rolled = 0;
        for id in link_group(q, left_id) {
            let (ti, ci) = locate(q, id)?;
            ensure_unlocked(q, ti)?;
            let e = q.tracks[ti].clips[ci].end();
            let t = &mut q.tracks[ti];
            let Some(ni) = t.clips.iter().position(|o| o.id != id && o.start == e) else { continue };
            let li = t.clips.iter().position(|o| o.id == id).unwrap();
            for i in [li, ni] {
                if t.clips[i].speed != 1.0 || t.clips[i].reversed {
                    return Err(CoreError::Invalid("roll needs both clips at 1x and forward".into()));
                }
            }
            t.clips[li].source_out += delta;
            t.clips[ni].start += delta;
            t.clips[ni].source_in += delta;
            rolled += 1;
        }
        if rolled == 0 {
            return Err(CoreError::Invalid("roll needs a clip that starts exactly where this one ends".into()));
        }
        Ok(())
    })
}

/// Blade: split every clip in the link group that spans timeline frame `at`.
/// Returns the ids of the new right-hand clips.
pub fn split_clip(p: &mut Project, clip_id: Uuid, at: i64) -> CoreResult<Vec<Uuid>> {
    if let Some((ti, xi)) = p.find_title(clip_id) {
        return transact(p, |q| {
            ensure_unlocked(q, ti)?;
            let x = q.tracks[ti].titles[xi].clone();
            if at <= x.start || at >= x.end() {
                return Err(CoreError::Invalid(format!("frame {at} is not inside {}", x.label())));
            }
            let right = TitleClip { id: Uuid::new_v4(), start: at, duration: x.end() - at, fade_in_frames: 0, ..x.clone() };
            q.tracks[ti].titles[xi].duration = at - x.start;
            q.tracks[ti].titles[xi].fade_out_frames = 0;
            let id = right.id;
            q.tracks[ti].titles.push(right);
            Ok(vec![id])
        });
    }
    transact(p, |q| {
        let group = link_group(q, clip_id);
        let new_link = q.clip(clip_id).and_then(|(_, c)| c.link_id).map(|_| Uuid::new_v4());
        let mut created = vec![];
        for id in group {
            let (ti, ci) = locate(q, id)?;
            ensure_unlocked(q, ti)?;
            let c = q.tracks[ti].clips[ci].clone();
            if at <= c.start || at >= c.end() {
                if id == clip_id {
                    return Err(CoreError::Invalid(format!("frame {at} is not inside clip {:?}", c.name)));
                }
                continue;
            }
            let off = at - c.start;
            let (l0, l1) = sub_range(&c, 0, off);
            let (r0, r1) = sub_range(&c, off, c.duration());
            let right = Clip {
                id: Uuid::new_v4(),
                start: at,
                source_in: r0,
                source_out: r1,
                link_id: new_link,
                fade_in_frames: 0,
                transition_in: None,
                ..c.clone()
            };
            let left = &mut q.tracks[ti].clips[ci];
            left.source_in = l0;
            left.source_out = l1;
            left.fade_out_frames = 0;
            let ld = left.duration();
            left.fade_in_frames = left.fade_in_frames.min(ld);
            let mut right = right;
            let rd = right.duration();
            right.fade_out_frames = right.fade_out_frames.min(rd);
            // Rounding at non-1x speeds: keep the halves contiguous.
            right.start = q.tracks[ti].clips[ci].end();
            created.push(right.id);
            q.tracks[ti].clips.push(right);
        }
        Ok(created)
    })
}

/// Delete clips or titles (and linked partners). With `ripple`, later items
/// on unlocked tracks shift left to close the deleted span.
pub fn delete_clips(p: &mut Project, ids: &[Uuid], ripple: bool) -> CoreResult<()> {
    transact(p, |q| {
        let mut all = vec![];
        for id in ids {
            if q.find_title(*id).is_some() {
                if !all.contains(id) {
                    all.push(*id);
                }
                continue;
            }
            for l in link_group(q, *id) {
                if !all.contains(&l) {
                    all.push(l);
                }
            }
        }
        if all.is_empty() {
            return Err(CoreError::NotFound("nothing to delete".into()));
        }
        let mut span = (i64::MAX, i64::MIN);
        for id in &all {
            if let Some((ti, xi)) = q.find_title(*id) {
                ensure_unlocked(q, ti)?;
                let x = &q.tracks[ti].titles[xi];
                span = (span.0.min(x.start), span.1.max(x.end()));
            } else {
                let (ti, ci) = locate(q, *id)?;
                ensure_unlocked(q, ti)?;
                let c = &q.tracks[ti].clips[ci];
                span = (span.0.min(c.start), span.1.max(c.end()));
            }
        }
        for t in &mut q.tracks {
            t.clips.retain(|c| !all.contains(&c.id));
            t.titles.retain(|c| !all.contains(&c.id));
        }
        if ripple {
            let gap = span.1 - span.0;
            for t in q.tracks.iter().filter(|t| !t.locked) {
                if !range_free(t, span.0, span.1, &[]) {
                    return Err(CoreError::Conflict(format!("ripple delete would collide with clips on track {}", t.name)));
                }
            }
            for t in q.tracks.iter_mut().filter(|t| !t.locked) {
                for c in t.clips.iter_mut().filter(|c| c.start >= span.1) {
                    c.start -= gap;
                }
                for c in t.titles.iter_mut().filter(|c| c.start >= span.1) {
                    c.start -= gap;
                }
            }
        }
        Ok(())
    })
}

/// Field updates for a clip. `transition_in` and `lut` use a nested Option:
/// `Some(None)` clears the value. Speed, reverse and transitions apply to the
/// whole link group so picture and sound stay locked.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClipPatch {
    pub name: Option<String>,
    pub gain_db: Option<f64>,
    pub color: Option<ColorParams>,
    pub fade_in_frames: Option<i64>,
    pub fade_out_frames: Option<i64>,
    pub speed: Option<f64>,
    pub reversed: Option<bool>,
    #[serde(default, with = "double_option")]
    pub transition_in: Option<Option<Transition>>,
    #[serde(default, with = "double_option")]
    pub lut: Option<Option<String>>,
    pub transform: Option<Transform>,
    pub crop: Option<Crop>,
    pub vfx: Option<Vec<Vfx>>,
    pub ofx: Option<Vec<OfxEffect>>,
}

mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<T: Serialize, S: Serializer>(v: &Option<Option<T>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            None => s.serialize_none(),
            Some(inner) => inner.serialize(s),
        }
    }
    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
        Ok(Some(Option::<T>::deserialize(d)?))
    }
}

pub fn update_clip(p: &mut Project, clip_id: Uuid, patch: ClipPatch) -> CoreResult<()> {
    transact(p, |q| {
        let (ti, ci) = locate(q, clip_id)?;
        ensure_unlocked(q, ti)?;
        let c = &mut q.tracks[ti].clips[ci];
        if let Some(n) = patch.name {
            let n = n.trim().to_string();
            if n.is_empty() || n.len() > 200 {
                return Err(CoreError::Invalid("clip name must be 1–200 characters".into()));
            }
            c.name = n;
        }
        if let Some(g) = patch.gain_db {
            c.gain_db = (g * 10.0).round() / 10.0;
        }
        if let Some(col) = patch.color {
            c.color = col;
        }
        if let Some(f) = patch.fade_in_frames {
            c.fade_in_frames = f;
        }
        if let Some(f) = patch.fade_out_frames {
            c.fade_out_frames = f;
        }
        if let Some(l) = patch.lut {
            c.lut = l.filter(|s| !s.trim().is_empty());
        }
        if let Some(t) = patch.transform {
            c.transform = t;
        }
        if let Some(cr) = patch.crop {
            c.crop = cr;
        }
        if let Some(v) = patch.vfx {
            c.vfx = v;
        }
        if let Some(v) = patch.ofx {
            c.ofx = v;
        }
        if let Some(Some(_)) = patch.transition_in {
            if q.prev_adjacent(clip_id).is_none() {
                return Err(CoreError::Invalid("a transition needs a clip ending exactly where this one starts".into()));
            }
        }
        let group = link_group(q, clip_id);
        for id in group {
            let (lti, lci) = locate(q, id)?;
            ensure_unlocked(q, lti)?;
            let c = &mut q.tracks[lti].clips[lci];
            if let Some(s) = patch.speed {
                c.speed = (s * 100.0).round() / 100.0;
            }
            if let Some(r) = patch.reversed {
                c.reversed = r;
            }
            if let Some(t) = patch.transition_in {
                c.transition_in = t;
            }
            let d = c.duration();
            if patch.speed.is_some() {
                c.fade_in_frames = c.fade_in_frames.min(d);
                c.fade_out_frames = c.fade_out_frames.min(d - c.fade_in_frames);
            }
        }
        Ok(())
    })
}

/// Insert a new, automatically named track after the last track of its kind.
/// Returns its index.
fn insert_track(q: &mut Project, kind: TrackKind) -> usize {
    let name = next_track_name(q, kind);
    let pos = q.tracks.iter().rposition(|t| t.kind == kind).map(|i| i + 1).unwrap_or(q.tracks.len());
    q.tracks.insert(pos, Track::new(kind, &name));
    pos
}

pub fn add_track(p: &mut Project, kind: TrackKind) -> CoreResult<Uuid> {
    transact(p, |q| {
        let pos = insert_track(q, kind);
        Ok(q.tracks[pos].id)
    })
}

pub fn remove_track(p: &mut Project, track_id: Uuid) -> CoreResult<()> {
    transact(p, |q| {
        let ti = track_index(q, track_id)?;
        let t = &q.tracks[ti];
        let n = t.clips.len() + t.titles.len();
        if n > 0 {
            return Err(CoreError::Invalid(format!("track {} still has {n} item(s); delete them first", t.name)));
        }
        if q.tracks.iter().filter(|o| o.kind == t.kind).count() == 1 {
            return Err(CoreError::Invalid(format!("track {} is the last {:?} track", t.name, t.kind).to_lowercase()));
        }
        q.tracks.remove(ti);
        Ok(())
    })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackPatch {
    pub name: Option<String>,
    pub muted: Option<bool>,
    pub locked: Option<bool>,
    pub solo: Option<bool>,
    pub volume_db: Option<f64>,
    pub pan: Option<f64>,
    pub eq_low_db: Option<f64>,
    pub eq_mid_db: Option<f64>,
    pub eq_high_db: Option<f64>,
}

pub fn update_track(p: &mut Project, track_id: Uuid, patch: TrackPatch) -> CoreResult<()> {
    transact(p, |q| {
        let t = q.track_mut(track_id).ok_or_else(|| CoreError::NotFound(format!("track {track_id}")))?;
        if let Some(n) = patch.name {
            let n = n.trim().to_string();
            if n.is_empty() || n.len() > 40 {
                return Err(CoreError::Invalid("track name must be 1–40 characters".into()));
            }
            t.name = n;
        }
        let r1 = |v: f64| (v * 10.0).round() / 10.0;
        let r2 = |v: f64| (v * 100.0).round() / 100.0;
        if let Some(m) = patch.muted {
            t.muted = m;
        }
        if let Some(l) = patch.locked {
            t.locked = l;
        }
        if let Some(s) = patch.solo {
            t.solo = s;
        }
        if let Some(v) = patch.volume_db {
            t.volume_db = r1(v);
        }
        if let Some(v) = patch.pan {
            t.pan = r2(v);
        }
        if let Some(v) = patch.eq_low_db {
            t.eq_low_db = r1(v);
        }
        if let Some(v) = patch.eq_mid_db {
            t.eq_mid_db = r1(v);
        }
        if let Some(v) = patch.eq_high_db {
            t.eq_high_db = r1(v);
        }
        Ok(())
    })
}

/// Move a track to a new index in the track list.
pub fn reorder_track(p: &mut Project, track_id: Uuid, new_index: usize) -> CoreResult<()> {
    transact(p, |q| {
        let ti = track_index(q, track_id)?;
        let t = q.tracks.remove(ti);
        let idx = new_index.min(q.tracks.len());
        q.tracks.insert(idx, t);
        Ok(())
    })
}

// ------------------------------------------------------------------ titles

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewTitle {
    pub track_id: Uuid,
    pub start: i64,
    pub duration: i64,
    pub text: String,
    pub font_size: Option<u32>,
    pub color_hex: Option<String>,
    pub position: Option<TitlePosition>,
}

pub fn add_title(p: &mut Project, req: NewTitle) -> CoreResult<Uuid> {
    transact(p, |q| {
        let mut ti = track_index(q, req.track_id)?;
        ensure_unlocked(q, ti)?;
        let h = q.settings.height;
        let x = TitleClip {
            id: Uuid::new_v4(),
            start: req.start,
            duration: req.duration,
            text: req.text,
            font_size: req.font_size.unwrap_or((h / 12).max(8)),
            color_hex: req.color_hex.unwrap_or_else(|| "#f2efe9".into()),
            position: req.position.unwrap_or(TitlePosition::LowerThird),
            fade_in_frames: 0,
            fade_out_frames: 0,
            extra: Map::new(),
        };
        if !range_free(&q.tracks[ti], x.start, x.end(), &[]) {
            if q.tracks[ti].kind != TrackKind::Video {
                return Err(CoreError::Conflict(format!("track {} is not free there; put the title on an empty track", q.tracks[ti].name)));
            }
            // Busy video track: use the lowest free, unlocked video track above
            // it, or add a new video track on top (one undo step with the title).
            let above = (ti + 1..q.tracks.len()).find(|&i| {
                let t = &q.tracks[i];
                t.kind == TrackKind::Video && !t.locked && range_free(t, x.start, x.end(), &[])
            });
            ti = match above {
                Some(i) => i,
                None => insert_track(q, TrackKind::Video),
            };
        }
        let id = x.id;
        q.tracks[ti].titles.push(x);
        Ok(id)
    })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TitlePatch {
    pub text: Option<String>,
    pub font_size: Option<u32>,
    pub color_hex: Option<String>,
    pub position: Option<TitlePosition>,
    pub duration: Option<i64>,
    pub fade_in_frames: Option<i64>,
    pub fade_out_frames: Option<i64>,
}

pub fn update_title(p: &mut Project, id: Uuid, patch: TitlePatch) -> CoreResult<()> {
    transact(p, |q| {
        let (ti, xi) = q.find_title(id).ok_or_else(|| CoreError::NotFound(format!("title {id}")))?;
        ensure_unlocked(q, ti)?;
        let x = &mut q.tracks[ti].titles[xi];
        if let Some(v) = patch.text {
            x.text = v;
        }
        if let Some(v) = patch.font_size {
            x.font_size = v;
        }
        if let Some(v) = patch.color_hex {
            x.color_hex = v.to_lowercase();
        }
        if let Some(v) = patch.position {
            x.position = v;
        }
        if let Some(v) = patch.duration {
            x.duration = v;
        }
        if let Some(v) = patch.fade_in_frames {
            x.fade_in_frames = v;
        }
        if let Some(v) = patch.fade_out_frames {
            x.fade_out_frames = v;
        }
        Ok(())
    })
}

pub fn move_title(p: &mut Project, id: Uuid, track_id: Uuid, start: i64) -> CoreResult<()> {
    transact(p, |q| {
        let (ti, xi) = q.find_title(id).ok_or_else(|| CoreError::NotFound(format!("title {id}")))?;
        let dest = track_index(q, track_id)?;
        ensure_unlocked(q, ti)?;
        ensure_unlocked(q, dest)?;
        let mut x = q.tracks[ti].titles.remove(xi);
        x.start = start;
        q.tracks[dest].titles.push(x);
        Ok(())
    })
}

// ------------------------------------------------------------------ markers & range

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarkerPatch {
    pub frame: Option<i64>,
    pub name: Option<String>,
    pub color: Option<String>,
    pub note: Option<String>,
}

pub fn add_marker(p: &mut Project, frame: i64, name: Option<String>, color: Option<String>) -> CoreResult<Uuid> {
    transact(p, |q| {
        let n = q.markers.len() + 1;
        let m = Marker {
            id: Uuid::new_v4(),
            frame,
            name: name.unwrap_or_else(|| format!("Marker {n}")),
            color: color.unwrap_or_else(|| MARKER_COLORS[0].into()),
            note: String::new(),
            extra: Map::new(),
        };
        let id = m.id;
        q.markers.push(m);
        Ok(id)
    })
}

pub fn update_marker(p: &mut Project, id: Uuid, patch: MarkerPatch) -> CoreResult<()> {
    transact(p, |q| {
        let m = q.markers.iter_mut().find(|m| m.id == id).ok_or_else(|| CoreError::NotFound(format!("marker {id}")))?;
        if let Some(v) = patch.frame {
            m.frame = v;
        }
        if let Some(v) = patch.name {
            m.name = v.trim().to_string();
        }
        if let Some(v) = patch.color {
            m.color = v;
        }
        if let Some(v) = patch.note {
            m.note = v;
        }
        Ok(())
    })
}

pub fn delete_marker(p: &mut Project, id: Uuid) -> CoreResult<()> {
    transact(p, |q| {
        let before = q.markers.len();
        q.markers.retain(|m| m.id != id);
        if q.markers.len() == before {
            return Err(CoreError::NotFound(format!("marker {id}")));
        }
        Ok(())
    })
}

/// Set or clear the timeline in/out points used for range rendering.
pub fn set_range(p: &mut Project, mark_in: Option<i64>, mark_out: Option<i64>) -> CoreResult<()> {
    transact(p, |q| {
        q.mark_in = mark_in;
        q.mark_out = mark_out;
        Ok(())
    })
}

// ------------------------------------------------------------------ copy / paste / duplicate

/// Clips and titles copied from the timeline, with the tracks they came from.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Clipboard {
    /// Earliest start among the copied items; paste offsets are relative to it.
    pub origin: i64,
    /// (track id, track kind, clip)
    pub clips: Vec<(Uuid, TrackKind, Clip)>,
    pub titles: Vec<(Uuid, TitleClip)>,
}

impl Clipboard {
    pub fn is_empty(&self) -> bool {
        self.clips.is_empty() && self.titles.is_empty()
    }
    fn span(&self) -> (i64, i64) {
        let starts = self.clips.iter().map(|(_, _, c)| c.start).chain(self.titles.iter().map(|(_, t)| t.start));
        let ends = self.clips.iter().map(|(_, _, c)| c.end()).chain(self.titles.iter().map(|(_, t)| t.end()));
        (starts.min().unwrap_or(0), ends.max().unwrap_or(0))
    }
}

/// Copy clips (with their linked partners) and titles.
pub fn copy_items(p: &Project, ids: &[Uuid]) -> CoreResult<Clipboard> {
    let mut cb = Clipboard::default();
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        if let Some((ti, xi)) = p.find_title(*id) {
            if seen.insert(*id) {
                cb.titles.push((p.tracks[ti].id, p.tracks[ti].titles[xi].clone()));
            }
            continue;
        }
        for l in link_group(p, *id) {
            if seen.insert(l) {
                let (ti, ci) = locate(p, l)?;
                cb.clips.push((p.tracks[ti].id, p.tracks[ti].kind, p.tracks[ti].clips[ci].clone()));
            }
        }
    }
    if cb.is_empty() {
        return Err(CoreError::NotFound("nothing selected to copy".into()));
    }
    cb.origin = cb.span().0;
    Ok(cb)
}

/// Paste a clipboard with its first item at `at`, overwriting whatever is in
/// the way. Every pasted clip and title gets a fresh id; linked pairs get a
/// fresh shared link id. Items return to their original track when it still
/// exists and is unlocked, otherwise to the first unlocked track of the same kind.
pub fn paste_items(p: &mut Project, cb: &Clipboard, at: i64) -> CoreResult<Vec<Uuid>> {
    if cb.is_empty() {
        return Err(CoreError::Invalid("the clipboard is empty".into()));
    }
    transact(p, |q| {
        let pick = |q: &Project, tid: Uuid, kind: TrackKind| -> CoreResult<usize> {
            match q.tracks.iter().position(|t| t.id == tid && !t.locked) {
                Some(i) => Ok(i),
                None => q
                    .tracks
                    .iter()
                    .position(|t| t.kind == kind && !t.locked)
                    .ok_or_else(|| CoreError::Invalid(format!("no unlocked {kind:?} track to paste onto").to_lowercase())),
            }
        };
        let mut links: std::collections::HashMap<Uuid, Uuid> = std::collections::HashMap::new();
        let mut created = vec![];
        let ids: std::collections::HashSet<Uuid> = cb.clips.iter().map(|(_, _, c)| c.id).collect();
        for (tid, kind, c) in &cb.clips {
            let ti = pick(q, *tid, *kind)?;
            let start = at + (c.start - cb.origin);
            let mut n = c.clone();
            n.id = Uuid::new_v4();
            n.start = start;
            n.link_id = c.link_id.map(|l| *links.entry(l).or_insert_with(Uuid::new_v4));
            // A transition only survives when its outgoing clip was pasted too.
            if n.transition_in.is_some() && !cb.clips.iter().any(|(t2, _, o)| t2 == tid && o.end() == c.start && ids.contains(&o.id)) {
                n.transition_in = None;
            }
            clear_range(q, ti, start, start + n.duration());
            created.push(n.id);
            q.tracks[ti].clips.push(n);
        }
        for (tid, x) in &cb.titles {
            let ti = pick(q, *tid, TrackKind::Video)?;
            let mut n = x.clone();
            n.id = Uuid::new_v4();
            n.start = at + (x.start - cb.origin);
            clear_range(q, ti, n.start, n.end());
            created.push(n.id);
            q.tracks[ti].titles.push(n);
        }
        Ok(created)
    })
}

/// Duplicate items immediately after the end of the selection.
pub fn duplicate_items(p: &mut Project, ids: &[Uuid]) -> CoreResult<Vec<Uuid>> {
    let cb = copy_items(p, ids)?;
    let end = cb.span().1;
    paste_items(p, &cb, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixtures::*;
    use crate::model::TransitionKind;

    fn placed() -> (Project, Uuid, Uuid) {
        let (mut p, mid) = project();
        let v1 = p.tracks[0].id;
        let ids = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(100) }).unwrap();
        (p, ids[0], ids[1])
    }

    fn two_clips() -> (Project, Uuid, Uuid) {
        let (mut p, mid) = project();
        let v1 = p.tracks[0].id;
        let a = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(20), source_out: Some(100) }).unwrap()[0];
        let b = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 80, source_in: Some(120), source_out: Some(200) }).unwrap()[0];
        (p, a, b)
    }

    #[test]
    fn copy_paste_duplicate_mint_fresh_ids() {
        let (mut p, v, a) = placed(); // 0..100 V1 + linked A1
        let cb = copy_items(&p, &[v]).unwrap();
        assert_eq!(cb.clips.len(), 2, "linked audio copied along");
        let new = paste_items(&mut p, &cb, 150).unwrap();
        assert_eq!(new.len(), 2);
        let (nv, na) = (p.clip(new[0]).unwrap().1.clone(), p.clip(new[1]).unwrap().1.clone());
        assert!(new.iter().all(|id| *id != v && *id != a));
        assert_eq!((nv.start, na.start), (150, 150));
        assert_eq!(nv.link_id, na.link_id);
        assert_ne!(nv.link_id, p.clip(v).unwrap().1.link_id, "fresh link id");
        // Paste over existing material overwrites it.
        paste_items(&mut p, &cb, 50).unwrap();
        assert_eq!(p.tracks[0].clips.iter().map(|c| (c.start, c.end())).collect::<Vec<_>>(), vec![(0, 50), (50, 150), (150, 250)]);
        // Duplicate lands right after the selection.
        let (mut q, v2, _) = placed();
        let d = duplicate_items(&mut q, &[v2]).unwrap();
        assert_eq!(q.clip(d[0]).unwrap().1.start, 100);
        // Transform/crop/vfx patch round trip.
        let mut t = Transform::default();
        t.scale_x = 1.5;
        t.rotation_deg = 12.0;
        update_clip(&mut q, v2, ClipPatch { transform: Some(t), crop: Some(Crop { left: 0.1, ..Default::default() }), vfx: Some(vec![Vfx { id: Uuid::new_v4(), kind: crate::model::VfxKind::Vignette, amount: 0.5, enabled: true }]), ..Default::default() }).unwrap();
        assert_eq!(q.clip(v2).unwrap().1.transform.scale_x, 1.5);
        assert!(update_clip(&mut q, v2, ClipPatch { crop: Some(Crop { left: 0.9, ..Default::default() }), ..Default::default() }).is_err());
        let a1 = link_group(&q, v2).into_iter().find(|i| *i != v2).unwrap();
        assert!(update_clip(&mut q, a1, ClipPatch { transform: Some(t), ..Default::default() }).is_err(), "audio clips have no transform");
    }

    #[test]
    fn place_creates_linked_audio() {
        let (p, v, a) = placed();
        let (vt, vc) = p.clip(v).unwrap();
        let (at, ac) = p.clip(a).unwrap();
        assert_eq!(vt.kind, TrackKind::Video);
        assert_eq!(at.kind, TrackKind::Audio);
        assert_eq!(vc.link_id, ac.link_id);
        assert!(vc.link_id.is_some());
        assert_eq!((ac.start, ac.source_in, ac.source_out), (0, 0, 100));
    }

    #[test]
    fn move_keeps_ids_and_link_sync() {
        let (mut p, v, a) = placed();
        let v2 = p.tracks[1].id;
        move_clip(&mut p, v, v2, 120).unwrap();
        let (t, c) = p.clip(v).unwrap();
        assert_eq!((t.id, c.start, c.id), (v2, 120, v));
        assert_eq!(p.clip(a).unwrap().1.start, 120, "linked audio follows");
        let a1 = p.tracks[2].id;
        let before = p.clone();
        assert!(move_clip(&mut p, v, a1, 0).is_err());
        assert_eq!(p, before);
        assert!(move_clip(&mut p, v, v2, -5).is_err(), "negative start");
    }

    #[test]
    fn trim_in_keeps_out_fixed_on_timeline() {
        let (mut p, v, a) = placed();
        trim_clip(&mut p, v, Edge::In, 10).unwrap();
        let c = p.clip(v).unwrap().1;
        assert_eq!((c.start, c.source_in, c.source_out, c.end()), (10, 10, 100, 100));
        trim_clip(&mut p, v, Edge::Out, 86).unwrap();
        assert_eq!(p.clip(v).unwrap().1.source_out, 86);
        assert_eq!(p.clip(a).unwrap().1.source_out, 86);
        assert!(trim_clip(&mut p, v, Edge::Out, 300).is_err(), "beyond media");
        assert!(trim_clip(&mut p, v, Edge::In, 90).is_err(), "past out");
    }

    #[test]
    fn split_produces_contiguous_halves() {
        let (mut p, v, a) = placed();
        let new = split_clip(&mut p, v, 40).unwrap();
        assert_eq!(new.len(), 2);
        let left = p.clip(v).unwrap().1.clone();
        let right = p.clip(new[0]).unwrap().1.clone();
        assert_eq!((left.source_in, left.source_out, left.end()), (0, 40, 40));
        assert_eq!((right.start, right.source_in, right.source_out), (40, 40, 100));
        assert_ne!(right.link_id, left.link_id);
        assert_eq!(p.clip(a).unwrap().1.source_out, 40);
        assert!(split_clip(&mut p, v, 0).is_err(), "split at boundary");
    }

    #[test]
    fn delete_and_ripple() {
        let (mut p, mid) = project();
        let v1 = p.tracks[0].id;
        let first = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(50) }).unwrap();
        let second = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 50, source_in: Some(50), source_out: Some(100) }).unwrap();
        delete_clips(&mut p, &[first[0]], true).unwrap();
        assert!(p.clip(first[1]).is_none(), "linked audio deleted too");
        assert_eq!(p.clip(second[0]).unwrap().1.start, 0);
        assert_eq!(p.clip(second[1]).unwrap().1.start, 0);
    }

    #[test]
    fn locked_track_rejects_edits() {
        let (mut p, v, _) = placed();
        let v1 = p.tracks[0].id;
        update_track(&mut p, v1, TrackPatch { locked: Some(true), ..Default::default() }).unwrap();
        assert!(matches!(move_clip(&mut p, v, v1, 10), Err(CoreError::Locked(_))));
        assert!(split_clip(&mut p, v, 10).is_err());
        assert!(slip_clip(&mut p, v, 5).is_err());
    }

    #[test]
    fn tracks_add_remove_reorder() {
        let (mut p, _) = project();
        let id = add_track(&mut p, TrackKind::Video).unwrap();
        assert_eq!(p.track(id).unwrap().name, "V3");
        assert_eq!(p.tracks.iter().position(|t| t.id == id), Some(2));
        reorder_track(&mut p, id, 0).unwrap();
        assert_eq!(p.tracks[0].id, id);
        remove_track(&mut p, id).unwrap();
        assert!(p.track(id).is_none());
    }

    #[test]
    fn overlap_rejected_by_place() {
        let (mut p, mid) = project();
        let v1 = p.tracks[0].id;
        place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: None, source_out: Some(100) }).unwrap();
        let r = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 50, source_in: None, source_out: Some(100) });
        assert!(matches!(r, Err(CoreError::Conflict(_))));
    }

    #[test]
    fn color_validation_through_update() {
        let (mut p, v, _) = placed();
        let mut c = ColorParams::default();
        c.saturation = 1.2;
        update_clip(&mut p, v, ClipPatch { color: Some(c), ..Default::default() }).unwrap();
        c.gamma = 0.0;
        assert!(update_clip(&mut p, v, ClipPatch { color: Some(c), ..Default::default() }).is_err());
        assert_eq!(p.clip(v).unwrap().1.color.saturation, 1.2);
    }

    // ---------- v2 operations

    #[test]
    fn slip_moves_source_not_timeline() {
        let (mut p, v, a) = placed();
        trim_clip(&mut p, v, Edge::In, 10).unwrap(); // 10..100 at 10
        slip_clip(&mut p, v, 25).unwrap();
        let c = p.clip(v).unwrap().1;
        assert_eq!((c.start, c.duration(), c.source_in, c.source_out), (10, 90, 35, 125));
        assert_eq!(p.clip(a).unwrap().1.source_in, 35, "linked audio slips too");
        assert!(slip_clip(&mut p, v, -40).is_err(), "before media start");
        assert!(slip_clip(&mut p, v, 200).is_err(), "past media end");
    }

    #[test]
    fn slide_trims_neighbours() {
        let (mut p, mid) = project();
        let v1 = p.tracks[0].id;
        let a = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(50) }).unwrap()[0];
        let b = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 50, source_in: Some(100), source_out: Some(130) }).unwrap()[0];
        let c = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 80, source_in: Some(150), source_out: Some(200) }).unwrap()[0];
        slide_clip(&mut p, b, 10).unwrap();
        let (ca, cb, cc) = (p.clip(a).unwrap().1.clone(), p.clip(b).unwrap().1.clone(), p.clip(c).unwrap().1.clone());
        assert_eq!((ca.end(), cb.start, cb.end(), cc.start), (60, 60, 90, 90));
        assert_eq!((cb.source_in, cb.source_out), (100, 130), "slid clip keeps its source");
        assert_eq!(cc.source_in, 160);
        assert_eq!(cc.end(), 130, "total length unchanged");
    }

    #[test]
    fn roll_moves_the_cut() {
        let (mut p, a, b) = two_clips();
        roll_edit(&mut p, a, -10).unwrap();
        let (ca, cb) = (p.clip(a).unwrap().1.clone(), p.clip(b).unwrap().1.clone());
        assert_eq!((ca.end(), cb.start, cb.source_in, cb.end()), (70, 70, 110, 160));
        assert!(roll_edit(&mut p, b, 5).is_err(), "nothing after b");
    }

    #[test]
    fn insert_splits_and_ripples() {
        let (mut p, mid) = project();
        let v1 = p.tracks[0].id;
        let x = place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(100) }).unwrap();
        let ids = insert_edit(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 40, source_in: Some(200), source_out: Some(230) }).unwrap();
        let v = &p.tracks[0].clips;
        assert_eq!(v.len(), 3);
        assert_eq!((v[0].start, v[0].end()), (0, 40));
        assert_eq!((v[1].id, v[1].start, v[1].end()), (ids[0], 40, 70));
        assert_eq!((v[2].start, v[2].source_in, v[2].end()), (70, 40, 130));
        // Sync audio was split and rippled identically.
        let a = &p.tracks[2].clips;
        assert_eq!(a.iter().map(|c| (c.start, c.end())).collect::<Vec<_>>(), vec![(0, 40), (40, 70), (70, 130)]);
        assert_eq!(p.clip(x[0]).unwrap().1.link_id, p.clip(x[1]).unwrap().1.link_id);
        assert_eq!(a[2].link_id, v[2].link_id, "right halves re-linked to each other");
    }

    #[test]
    fn overwrite_replaces_the_range() {
        let (mut p, mid) = project();
        let v1 = p.tracks[0].id;
        place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(100) }).unwrap();
        overwrite_edit(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 30, source_in: Some(200), source_out: Some(220) }).unwrap();
        let v = &p.tracks[0].clips;
        assert_eq!(v.iter().map(|c| (c.start, c.end(), c.source_in)).collect::<Vec<_>>(), vec![(0, 30, 0), (30, 50, 200), (50, 100, 50)]);
        let a = &p.tracks[2].clips;
        assert_eq!(a.iter().map(|c| (c.start, c.end())).collect::<Vec<_>>(), vec![(0, 30), (30, 50), (50, 100)]);
        assert_eq!(p.duration(), 100, "overwrite never changes length");
    }

    #[test]
    fn speed_and_reverse() {
        let (mut p, v, a) = placed();
        update_clip(&mut p, v, ClipPatch { speed: Some(2.0), ..Default::default() }).unwrap();
        assert_eq!(p.clip(v).unwrap().1.duration(), 50);
        assert_eq!(p.clip(a).unwrap().1.duration(), 50, "audio retimed with picture");
        update_clip(&mut p, v, ClipPatch { reversed: Some(true), ..Default::default() }).unwrap();
        // Split a reversed 2x clip: the left half shows the END of the source.
        let r = split_clip(&mut p, v, 10).unwrap();
        let (l, rr) = (p.clip(v).unwrap().1.clone(), p.clip(r[0]).unwrap().1.clone());
        assert_eq!((l.source_in, l.source_out, l.duration()), (80, 100, 10));
        assert_eq!((rr.source_in, rr.source_out, rr.start, rr.duration()), (0, 80, 10, 40));
        let (mut q, v2, _) = placed();
        let mid = q.media[0].id;
        let v1 = q.tracks[0].id;
        place_clip(&mut q, PlaceClip { media_id: mid, track_id: v1, start: 150, source_in: Some(0), source_out: Some(10) }).unwrap();
        assert!(update_clip(&mut q, v2, ClipPatch { speed: Some(0.5), ..Default::default() }).is_err(), "200 frames would hit the next clip");
    }

    #[test]
    fn fades_and_transitions() {
        let (mut p, a, b) = two_clips();
        update_clip(&mut p, a, ClipPatch { fade_in_frames: Some(10), fade_out_frames: Some(5), ..Default::default() }).unwrap();
        assert!(update_clip(&mut p, a, ClipPatch { fade_in_frames: Some(80), ..Default::default() }).is_err());
        let t = Transition { kind: TransitionKind::CrossDissolve, duration_frames: 12 };
        update_clip(&mut p, b, ClipPatch { transition_in: Some(Some(t)), ..Default::default() }).unwrap();
        let ab = link_group(&p, b);
        assert!(ab.iter().all(|id| p.clip(*id).unwrap().1.transition_in == Some(t)), "sync audio gets the crossfade");
        // Moving the incoming clip away drops the now-dangling transition.
        let v1 = p.tracks[0].id;
        move_clip(&mut p, b, v1, 120).unwrap();
        assert_eq!(p.clip(b).unwrap().1.transition_in, None);
        assert!(update_clip(&mut p, a, ClipPatch { transition_in: Some(Some(t)), ..Default::default() }).is_err(), "nothing ends where a starts");
    }

    #[test]
    fn titles_and_markers() {
        let (mut p, _) = project();
        let v2 = p.tracks[1].id;
        let t = add_title(&mut p, NewTitle { track_id: v2, start: 10, duration: 50, text: "Harbour".into(), font_size: None, color_hex: None, position: None }).unwrap();
        update_title(&mut p, t, TitlePatch { text: Some("Harbour, 1912".into()), color_hex: Some("#F4C430".into()), ..Default::default() }).unwrap();
        assert_eq!(p.tracks[1].titles[0].color_hex, "#f4c430");
        let r = split_clip(&mut p, t, 30).unwrap();
        assert_eq!(p.tracks[1].titles.len(), 2);
        delete_clips(&mut p, &r, false).unwrap();
        assert_eq!(p.tracks[1].titles[0].end(), 30);
        let a1 = p.tracks[2].id;
        assert!(add_title(&mut p, NewTitle { track_id: a1, start: 0, duration: 10, text: "x".into(), font_size: None, color_hex: None, position: None }).is_err());
        let m = add_marker(&mut p, 24, None, None).unwrap();
        update_marker(&mut p, m, MarkerPatch { name: Some("Music in".into()), color: Some("red".into()), ..Default::default() }).unwrap();
        assert!(update_marker(&mut p, m, MarkerPatch { color: Some("teal".into()), ..Default::default() }).is_err());
        set_range(&mut p, Some(10), Some(40)).unwrap();
        assert!(set_range(&mut p, Some(40), Some(10)).is_err());
        delete_marker(&mut p, m).unwrap();
        assert!(p.markers.is_empty());
    }

    fn title_at(track_id: Uuid, start: i64) -> NewTitle {
        NewTitle { track_id, start, duration: 50, text: "Title".into(), font_size: None, color_hex: None, position: None }
    }

    #[test]
    fn new_minimal_starts_with_v1_and_a1() {
        let p = Project::new_minimal("Test", crate::model::ProjectSettings::default());
        let tracks: Vec<_> = p.tracks.iter().map(|t| (t.name.as_str(), t.kind)).collect();
        assert_eq!(tracks, vec![("V1", TrackKind::Video), ("A1", TrackKind::Audio)]);
        p.validate().unwrap();
    }

    #[test]
    fn title_over_busy_video_uses_or_adds_a_track_above() {
        let (mut p, mid) = project();
        p.tracks.retain(|t| t.name == "V1" || t.name == "A1"); // as a new app project
        let (v1, a1) = (p.tracks[0].id, p.tracks[1].id);
        place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(100) }).unwrap();
        let names = |p: &Project| p.tracks.iter().map(|t| t.name.clone()).collect::<Vec<_>>();

        // Busy V1 and no track above: V2 is created above V1 for the title.
        let t1 = add_title(&mut p, title_at(v1, 10)).unwrap();
        assert_eq!(names(&p), ["V1", "V2", "A1"]);
        assert!(p.tracks[0].titles.is_empty());
        assert_eq!(p.tracks[1].titles[0].id, t1);

        // V1 busy, V2 busy over the same range: V3 on top.
        let t2 = add_title(&mut p, title_at(v1, 20)).unwrap();
        assert_eq!(names(&p), ["V1", "V2", "V3", "A1"]);
        assert_eq!(p.tracks[2].titles[0].id, t2);

        // V1 busy but V2 free there: V2 is reused, no new track.
        let t3 = add_title(&mut p, title_at(v1, 60)).unwrap();
        assert_eq!(p.tracks.len(), 4);
        assert!(p.tracks[1].titles.iter().any(|x| x.id == t3));

        // Free target: the title stays on it, as before.
        let t4 = add_title(&mut p, title_at(v1, 200)).unwrap();
        assert_eq!(p.tracks.len(), 4);
        assert_eq!(p.tracks[0].titles[0].id, t4);

        // Audio tracks and locked tracks still refuse titles, without adding tracks.
        assert!(add_title(&mut p, title_at(a1, 10)).is_err());
        update_track(&mut p, v1, TrackPatch { locked: Some(true), ..Default::default() }).unwrap();
        assert!(add_title(&mut p, title_at(v1, 10)).is_err());
        assert_eq!(p.tracks.len(), 4);
    }

    #[test]
    fn mixer_patch_validates() {
        let (mut p, _) = project();
        let a1 = p.tracks[2].id;
        update_track(&mut p, a1, TrackPatch { pan: Some(-0.333), eq_low_db: Some(4.0), solo: Some(true), volume_db: Some(-3.0), ..Default::default() }).unwrap();
        let t = p.track(a1).unwrap();
        assert_eq!((t.pan, t.eq_low_db, t.solo, t.volume_db), (-0.33, 4.0, true, -3.0));
        assert!(update_track(&mut p, a1, TrackPatch { eq_high_db: Some(40.0), ..Default::default() }).is_err());
    }
}

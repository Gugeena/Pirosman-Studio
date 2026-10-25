//! Tauri commands: the only way the UI changes the project.
//! Every command returns the full `View` (or a typed payload) so the
//! frontend never keeps a competing copy of project state.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use pirosman_core::bundles::{self, BundleExportReport, BundleImportReport};
use pirosman_core::diagnostics::{self, OfxReport, ToolReport};
use pirosman_core::diff::{diff_entries, DiffEntry};
use pirosman_core::media::{self, browser_playable, media_status, MediaStatus};
use pirosman_core::merge::{self, MergeOutcome, MergeState, Side};
use pirosman_core::model::{MediaKind, Project, ProjectSettings, TrackKind};
use pirosman_core::project::{self, atomic_write_no_backup as atomic_write, history_dir};
use pirosman_core::render::{self, RenderPlan, RenderSettings};
use pirosman_core::revisions::{short, BranchInfo, RevisionStore, RevisionSummary};
use pirosman_core::time::Rational;
use pirosman_core::timeline::{self, ClipPatch, Edge, MarkerPatch, NewTitle, PlaceClip, TitlePatch, TrackPatch};
use pirosman_core::{CoreError, CoreResult};

use crate::state::{file_name, AppState, Session};

type Res<T> = Result<T, String>;

fn e(err: CoreError) -> String {
    err.to_string()
}

#[derive(Serialize)]
pub struct MediaInfo {
    pub id: Uuid,
    pub status: MediaStatus,
    /// Playable file for the WebView (original or proxy).
    pub preview: Option<String>,
    /// Cache pictures as `pcache:` references (see `AppState::cache_ref`).
    pub thumb: Option<String>,
    pub wave: Option<String>,
    /// Timeline filmstrip: `strip_frames` evenly spaced frames side by side.
    pub strip: Option<String>,
    pub strip_frames: u32,
    pub needs_proxy: bool,
}

/// Seconds a media lasts (its duration is stored in project frames).
fn media_seconds(m: &pirosman_core::model::MediaRef, rate: Rational) -> Option<f64> {
    m.duration_frames.filter(|f| *f > 0).map(|f| f as f64 * rate.den as f64 / rate.num as f64)
}

#[derive(Serialize)]
pub struct VcsBrief {
    pub branch: String,
    pub head: Option<String>,
    pub head_message: Option<String>,
    /// Working copy vs branch head (drives the grease-pencil change marks).
    pub changes: Vec<DiffEntry>,
    pub merging: Option<String>,
}

#[derive(Serialize)]
pub struct View {
    pub project: Project,
    pub path: Option<String>,
    pub unsaved: bool,
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
    pub media: Vec<MediaInfo>,
    pub vcs: Option<VcsBrief>,
    pub vcs_error: Option<String>,
}

fn merge_state_path(store: &RevisionStore) -> PathBuf {
    store.root().join("MERGE_STATE.json")
}

fn read_merge_state(store: &RevisionStore) -> Option<MergeState> {
    std::fs::read_to_string(merge_state_path(store)).ok().and_then(|t| serde_json::from_str(&t).ok())
}

fn build_view(app: &AppHandle, st: &AppState, s: &mut Session) -> View {
    let scope = app.asset_protocol_scope();
    let rate = s.project.settings.frame_rate;
    let media = s
        .project
        .media
        .iter()
        .map(|m| {
            let status = media_status(m);
            let sha = m.sha256.clone().unwrap_or_default();
            let exists = |p: PathBuf| p.exists().then(|| p.to_string_lossy().to_string());
            let needs_proxy = m.kind != MediaKind::Image && !browser_playable(m);
            let preview = if status == MediaStatus::Missing {
                None
            } else if needs_proxy || m.kind == MediaKind::Audio && !browser_playable(m) {
                let px = exists(st.proxy_path(&sha));
                if let Some(p) = &px {
                    let _ = scope.allow_file(p);
                }
                px
            } else {
                let _ = scope.allow_file(&m.path);
                Some(m.path.clone())
            };
            let strip_frames = if m.has_video && m.kind == MediaKind::Video { media_seconds(m, rate).map(media::filmstrip_frames).unwrap_or(0) } else { 0 };
            MediaInfo {
                id: m.id,
                status,
                preview,
                thumb: st.cache_ref(&st.thumb_path(&sha, m.kind == MediaKind::Image)),
                wave: st.cache_ref(&st.wave_path(&sha)),
                strip: if strip_frames > 0 { st.cache_ref(&st.strip_path(&sha, strip_frames)) } else { None },
                strip_frames,
                needs_proxy,
            }
        })
        .collect();
    let (vcs, vcs_error) = match s.path.is_some() {
        false => (None, None),
        true => match vcs_brief(s) {
            Ok(v) => (Some(v), None),
            Err(err) => (None, Some(err.to_string())),
        },
    };
    View {
        project: s.project.clone(),
        path: s.path.as_ref().map(|p| p.to_string_lossy().to_string()),
        unsaved: s.saved.as_ref() != Some(&s.project),
        undo_label: s.history.undo_label().map(String::from),
        redo_label: s.history.redo_label().map(String::from),
        media,
        vcs,
        vcs_error,
    }
}

fn vcs_brief(s: &mut Session) -> CoreResult<VcsBrief> {
    let store = s.store()?;
    let branch = store.current_branch()?;
    let head = store.head_revision()?;
    let head_message = match &head {
        Some(h) => Some(store.get(h)?.body.message),
        None => None,
    };
    let changes = match s.head_snapshot(&store)? {
        Some(snap) => diff_entries(&snap, &s.project),
        None => vec![],
    };
    let merging = read_merge_state(&store).map(|m| m.theirs_branch);
    Ok(VcsBrief { branch, head, head_message, changes, merging })
}

fn view(app: &AppHandle, st: &AppState) -> View {
    let mut s = st.session.lock().unwrap();
    build_view(app, st, &mut s)
}

/// Run an undoable edit and return the new view.
fn edit<T>(app: &AppHandle, st: &AppState, label: &str, f: impl FnOnce(&mut Project) -> CoreResult<T>) -> Res<View> {
    let mut s = st.session.lock().unwrap();
    s.edit(label, f).map_err(e)?;
    Ok(build_view(app, st, &mut s))
}

/// Generate thumbnails, waveforms, and proxies in the background.
pub fn spawn_media_jobs(app: AppHandle, items: Vec<pirosman_core::model::MediaRef>) {
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let Ok(tools) = st.tools() else { return };
        for m in items {
            let Some(sha) = m.sha256.clone() else { continue };
            let src = PathBuf::from(&m.path);
            if !src.is_file() {
                continue;
            }
            if m.has_video {
                let t = st.thumb_path(&sha, m.kind == MediaKind::Image);
                if !t.exists() {
                    let at = m.duration_frames.map(|_| 0.5).unwrap_or(0.0);
                    let _ = media::make_thumbnail(&tools, &src, at, &t);
                    let _ = app.emit("media-updated", m.id);
                }
            }
            if m.has_video && m.kind == MediaKind::Video {
                let rate = st.session.lock().unwrap().project.settings.frame_rate;
                if let Some(secs) = media_seconds(&m, rate) {
                    let frames = media::filmstrip_frames(secs);
                    let strip = st.strip_path(&sha, frames);
                    if !strip.exists() {
                        let _ = media::make_filmstrip(&tools, &src, secs, frames, &strip);
                        let _ = app.emit("media-updated", m.id);
                    }
                }
            }
            if m.has_audio {
                let w = st.wave_path(&sha);
                if !w.exists() {
                    let _ = media::make_waveform(&tools, &src, &w);
                    let _ = app.emit("media-updated", m.id);
                }
            }
            if m.kind != MediaKind::Image && !browser_playable(&m) {
                let px = st.proxy_path(&sha);
                if !px.exists() {
                    let _ = app.emit("media-job", serde_json::json!({ "id": m.id, "job": "proxy", "state": "running" }));
                    let r = media::make_proxy(&tools, &src, &px);
                    let _ = app.emit(
                        "media-job",
                        serde_json::json!({ "id": m.id, "job": "proxy", "state": if r.is_ok() { "done" } else { "failed" }, "error": r.err().map(|x| x.to_string()) }),
                    );
                    let _ = app.emit("media-updated", m.id);
                }
            }
        }
    });
}

// ---------------------------------------------------------------- project

#[tauri::command]
pub fn get_view(app: AppHandle, st: State<AppState>) -> View {
    view(&app, &st)
}

#[derive(Deserialize)]
pub struct NewProject {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub sample_rate: u32,
}

#[tauri::command]
pub fn new_project(app: AppHandle, st: State<AppState>, req: NewProject) -> Res<View> {
    let settings = ProjectSettings {
        width: req.width,
        height: req.height,
        frame_rate: Rational::new(req.fps_num, req.fps_den).reduced(),
        sample_rate: req.sample_rate,
        ..Default::default()
    };
    let name = req.name.trim();
    let p = Project::new_minimal(if name.is_empty() { "Untitled" } else { name }, settings);
    p.validate().map_err(e)?;
    let mut s = st.session.lock().unwrap();
    *s = Session::new(p);
    Ok(build_view(&app, &st, &mut s))
}

#[tauri::command]
pub fn open_project(app: AppHandle, st: State<AppState>, path: String) -> Res<View> {
    let p = project::load_project(Path::new(&path)).map_err(e)?;
    remember_recent(&st, &path);
    let media = p.media.clone();
    let mut s = st.session.lock().unwrap();
    *s = Session::new(p.clone());
    s.path = Some(PathBuf::from(&path));
    s.saved = Some(p);
    let v = build_view(&app, &st, &mut s);
    drop(s);
    spawn_media_jobs(app, media);
    Ok(v)
}

#[tauri::command]
pub fn save_project(app: AppHandle, st: State<AppState>, path: Option<String>) -> Res<View> {
    let mut s = st.session.lock().unwrap();
    let target = match path {
        Some(p) => {
            let mut pb = PathBuf::from(p);
            if pb.extension().map(|x| x != project::PROJECT_EXTENSION).unwrap_or(true) {
                pb.set_extension(project::PROJECT_EXTENSION);
            }
            pb
        }
        None => s.path.clone().ok_or("choose where to save the project")?,
    };
    project::save_project(&s.project, &target).map_err(e)?;
    if s.path.as_ref() != Some(&target) {
        s.head_cache = None;
    }
    remember_recent(&st, &target.to_string_lossy());
    s.path = Some(target);
    s.saved = Some(s.project.clone());
    Ok(build_view(&app, &st, &mut s))
}

#[tauri::command]
pub fn rename_project(app: AppHandle, st: State<AppState>, name: String) -> Res<View> {
    edit(&app, &st, "Rename project", |p| {
        let n = name.trim();
        if n.is_empty() || n.len() > 120 {
            return Err(CoreError::Invalid("project name must be 1–120 characters".into()));
        }
        p.name = n.into();
        Ok(())
    })
}

#[tauri::command]
pub fn undo(app: AppHandle, st: State<AppState>) -> Res<View> {
    let mut s = st.session.lock().unwrap();
    let s = &mut *s;
    s.history.undo(&mut s.project).ok_or("nothing to undo")?;
    Ok(build_view(&app, &st, s))
}

#[tauri::command]
pub fn redo(app: AppHandle, st: State<AppState>) -> Res<View> {
    let mut s = st.session.lock().unwrap();
    let s = &mut *s;
    s.history.redo(&mut s.project).ok_or("nothing to redo")?;
    Ok(build_view(&app, &st, s))
}

// ---------------------------------------------------------------- media

#[derive(Serialize)]
pub struct ImportResult {
    pub view: View,
    pub imported: Vec<String>,
    pub errors: Vec<String>,
}

#[tauri::command]
pub async fn import_media(app: AppHandle, paths: Vec<String>) -> Res<ImportResult> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let st = app2.state::<AppState>();
        let tools = st.tools().map_err(e)?;
        let rate = st.session.lock().unwrap().project.settings.frame_rate;
        let mut ok = vec![];
        let mut errors = vec![];
        for p in &paths {
            let path = Path::new(p);
            let already = st.session.lock().unwrap().project.media.iter().any(|m| Path::new(&m.path) == path);
            if already {
                errors.push(format!("{} is already in the media pool", file_name(path)));
                continue;
            }
            match media::import_media(&tools, path, rate) {
                Ok(m) => ok.push(m),
                Err(err) => errors.push(format!("{}: {err}", file_name(path))),
            }
        }
        let names = ok.iter().map(|m| m.name.clone()).collect();
        let mut s = st.session.lock().unwrap();
        let items = ok.clone();
        s.edit("Import media", |p| {
            p.media.extend(ok);
            p.validate()
        })
        .map_err(e)?;
        let view = build_view(&app2, &st, &mut s);
        drop(s);
        spawn_media_jobs(app2.clone(), items);
        Ok(ImportResult { view, imported: names, errors })
    })
    .await
    .map_err(|x| x.to_string())?
}

#[tauri::command]
pub async fn relink_media(app: AppHandle, media_id: Uuid, path: String, allow_different: bool) -> Res<View> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let st = app2.state::<AppState>();
        let tools = st.tools().map_err(e)?;
        let mut s = st.session.lock().unwrap();
        s.edit("Relink media", |p| media::relink(p, media_id, Path::new(&path), &tools, allow_different)).map_err(e)?;
        let m = s.project.media(media_id).cloned();
        let v = build_view(&app2, &st, &mut s);
        drop(s);
        if let Some(m) = m {
            spawn_media_jobs(app2.clone(), vec![m]);
        }
        Ok(v)
    })
    .await
    .map_err(|x| x.to_string())?
}

#[tauri::command]
pub fn remove_media(app: AppHandle, st: State<AppState>, media_id: Uuid) -> Res<View> {
    edit(&app, &st, "Remove media", |p| {
        let used = p.tracks.iter().flat_map(|t| &t.clips).filter(|c| c.media_id == media_id).count();
        if used > 0 {
            return Err(CoreError::Invalid(format!("this media is used by {used} clip(s) on the timeline")));
        }
        p.media.retain(|m| m.id != media_id);
        Ok(())
    })
}

// ---------------------------------------------------------------- timeline

#[tauri::command]
pub fn place_clip(app: AppHandle, st: State<AppState>, req: PlaceClip) -> Res<View> {
    edit(&app, &st, "Place clip", |p| timeline::place_clip(p, req).map(|_| ()))
}

#[tauri::command]
pub fn move_clip(app: AppHandle, st: State<AppState>, clip_id: Uuid, track_id: Uuid, start: i64) -> Res<View> {
    edit(&app, &st, "Move clip", |p| timeline::move_clip(p, clip_id, track_id, start))
}

#[tauri::command]
pub fn trim_clip(app: AppHandle, st: State<AppState>, clip_id: Uuid, edge: Edge, frame: i64) -> Res<View> {
    edit(&app, &st, "Trim clip", |p| timeline::trim_clip(p, clip_id, edge, frame))
}

#[tauri::command]
pub fn split_clip(app: AppHandle, st: State<AppState>, clip_id: Uuid, at: i64) -> Res<View> {
    edit(&app, &st, "Blade", |p| timeline::split_clip(p, clip_id, at).map(|_| ()))
}

/// Blade every unlocked track at the playhead.
#[tauri::command]
pub fn split_at(app: AppHandle, st: State<AppState>, at: i64) -> Res<View> {
    edit(&app, &st, "Blade all tracks", |p| {
        let ids: Vec<Uuid> = p
            .tracks
            .iter()
            .filter(|t| !t.locked)
            .flat_map(|t| t.clips.iter().filter(|c| c.start < at && c.end() > at).map(|c| c.id))
            .collect();
        if ids.is_empty() {
            return Err(CoreError::Invalid("no clip under the playhead".into()));
        }
        let mut done = std::collections::HashSet::new();
        for id in ids {
            if done.contains(&id) {
                continue;
            }
            for l in timeline::link_group(p, id) {
                done.insert(l);
            }
            timeline::split_clip(p, id, at)?;
        }
        Ok(())
    })
}

#[tauri::command]
pub fn delete_clips(app: AppHandle, st: State<AppState>, clip_ids: Vec<Uuid>, ripple: bool) -> Res<View> {
    edit(&app, &st, if ripple { "Ripple delete" } else { "Delete" }, |p| timeline::delete_clips(p, &clip_ids, ripple))
}

#[tauri::command]
pub fn update_clip(app: AppHandle, st: State<AppState>, clip_id: Uuid, patch: ClipPatch) -> Res<View> {
    let label = if patch.color.is_some() {
        "Grade clip"
    } else if patch.lut.is_some() {
        "Set LUT"
    } else if patch.transition_in.is_some() {
        "Transition"
    } else if patch.speed.is_some() || patch.reversed.is_some() {
        "Retime clip"
    } else if patch.fade_in_frames.is_some() || patch.fade_out_frames.is_some() {
        "Fade"
    } else if patch.gain_db.is_some() {
        "Clip gain"
    } else {
        "Rename clip"
    };
    edit(&app, &st, label, |p| timeline::update_clip(p, clip_id, patch))
}

#[tauri::command]
pub fn insert_edit(app: AppHandle, st: State<AppState>, req: PlaceClip) -> Res<View> {
    edit(&app, &st, "Insert", |p| timeline::insert_edit(p, req).map(|_| ()))
}

#[tauri::command]
pub fn overwrite_edit(app: AppHandle, st: State<AppState>, req: PlaceClip) -> Res<View> {
    edit(&app, &st, "Overwrite", |p| timeline::overwrite_edit(p, req).map(|_| ()))
}

#[tauri::command]
pub fn slip_clip(app: AppHandle, st: State<AppState>, clip_id: Uuid, delta: i64) -> Res<View> {
    edit(&app, &st, "Slip", |p| timeline::slip_clip(p, clip_id, delta))
}

#[tauri::command]
pub fn slide_clip(app: AppHandle, st: State<AppState>, clip_id: Uuid, delta: i64) -> Res<View> {
    edit(&app, &st, "Slide", |p| timeline::slide_clip(p, clip_id, delta))
}

#[tauri::command]
pub fn roll_edit(app: AppHandle, st: State<AppState>, clip_id: Uuid, delta: i64) -> Res<View> {
    edit(&app, &st, "Roll", |p| timeline::roll_edit(p, clip_id, delta))
}

#[tauri::command]
pub fn add_title(app: AppHandle, st: State<AppState>, req: NewTitle) -> Res<View> {
    edit(&app, &st, "Add title", |p| timeline::add_title(p, req).map(|_| ()))
}

#[tauri::command]
pub fn update_title(app: AppHandle, st: State<AppState>, title_id: Uuid, patch: TitlePatch) -> Res<View> {
    edit(&app, &st, "Edit title", |p| timeline::update_title(p, title_id, patch))
}

#[tauri::command]
pub fn add_marker(app: AppHandle, st: State<AppState>, frame: i64, name: Option<String>, color: Option<String>) -> Res<View> {
    edit(&app, &st, "Add marker", |p| timeline::add_marker(p, frame, name, color).map(|_| ()))
}

#[tauri::command]
pub fn update_marker(app: AppHandle, st: State<AppState>, marker_id: Uuid, patch: MarkerPatch) -> Res<View> {
    edit(&app, &st, "Edit marker", |p| timeline::update_marker(p, marker_id, patch))
}

#[tauri::command]
pub fn delete_marker(app: AppHandle, st: State<AppState>, marker_id: Uuid) -> Res<View> {
    edit(&app, &st, "Delete marker", |p| timeline::delete_marker(p, marker_id))
}

#[tauri::command]
pub fn set_range(app: AppHandle, st: State<AppState>, mark_in: Option<i64>, mark_out: Option<i64>) -> Res<View> {
    edit(&app, &st, "In/out range", |p| timeline::set_range(p, mark_in, mark_out))
}

#[tauri::command]
pub fn export_otio(st: State<AppState>, path: String) -> Res<String> {
    let s = st.session.lock().unwrap();
    let mut out = PathBuf::from(path);
    if out.extension().map(|x| x != "otio").unwrap_or(true) {
        out.set_extension("otio");
    }
    atomic_write(&out, pirosman_core::otio::export_otio_string(&s.project).as_bytes()).map_err(e)?;
    Ok(out.to_string_lossy().into())
}

/// Luma waveform + RGB parade of the graded frame of the topmost visible clip at `frame`.
#[tauri::command]
pub async fn video_scopes(app: AppHandle, frame: i64) -> Res<Option<pirosman_core::scopes::Scopes>> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let (path, secs, clip) = {
            let s = st.session.lock().unwrap();
            let p = &s.project;
            let hit = p
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Video && !t.muted)
                .rev()
                .find_map(|t| t.clips.iter().find(|c| c.start <= frame && frame < c.end()))
                .cloned();
            let Some(c) = hit else { return Ok(None) };
            let m = p.media(c.media_id).cloned().ok_or("media missing")?;
            let off = frame - c.start;
            let (si, so) = timeline::sub_range(&c, off, off + 1);
            let src = if c.reversed { so - 1 } else { si };
            let secs = pirosman_core::time::frames_to_micros(src, p.settings.frame_rate) as f64 / 1e6;
            (PathBuf::from(m.path), secs, c)
        };
        let tools = st.tools().map_err(e)?;
        pirosman_core::scopes::scopes_for(&tools, &path, secs, Some(&clip)).map(Some).map_err(e)
    })
    .await
    .map_err(|x| x.to_string())?
}

#[tauri::command]
pub fn add_track(app: AppHandle, st: State<AppState>, kind: TrackKind) -> Res<View> {
    edit(&app, &st, "Add track", |p| timeline::add_track(p, kind).map(|_| ()))
}

#[tauri::command]
pub fn remove_track(app: AppHandle, st: State<AppState>, track_id: Uuid) -> Res<View> {
    edit(&app, &st, "Remove track", |p| timeline::remove_track(p, track_id))
}

#[tauri::command]
pub fn update_track(app: AppHandle, st: State<AppState>, track_id: Uuid, patch: TrackPatch) -> Res<View> {
    let label = if patch.pan.is_some() || patch.volume_db.is_some() || patch.eq_low_db.is_some() || patch.eq_mid_db.is_some() || patch.eq_high_db.is_some() || patch.solo.is_some() {
        "Mixer"
    } else {
        "Track settings"
    };
    edit(&app, &st, label, |p| timeline::update_track(p, track_id, patch))
}

#[tauri::command]
pub fn reorder_track(app: AppHandle, st: State<AppState>, track_id: Uuid, new_index: usize) -> Res<View> {
    edit(&app, &st, "Reorder tracks", |p| timeline::reorder_track(p, track_id, new_index))
}

// ---------------------------------------------------------------- render

#[tauri::command]
pub fn render_plan(st: State<AppState>, settings: RenderSettings) -> Res<RenderPlan> {
    let s = st.session.lock().unwrap();
    render::build_plan(&s.project, &settings, true).map_err(e)
}

#[tauri::command]
pub fn default_render_settings(st: State<AppState>, output: String) -> RenderSettings {
    let s = st.session.lock().unwrap();
    RenderSettings::from_project(&s.project, &output)
}

/// Start an export (`preview = false`) or a low-resolution exact preview render.
#[tauri::command]
pub fn start_render(app: AppHandle, st: State<AppState>, settings: Option<RenderSettings>, preview: bool) -> Res<RenderPlan> {
    let tools = st.tools().map_err(e)?;
    let mut guard = st.render_cancel.lock().unwrap();
    if guard.is_some() {
        return Err("a render is already running".into());
    }
    let project = st.session.lock().unwrap().project.clone();
    let settings = if preview {
        let out = st.preview_render_path();
        std::fs::create_dir_all(out.parent().unwrap()).map_err(|x| x.to_string())?;
        let mut s = RenderSettings::from_project(&project, &out.to_string_lossy());
        let w = 640u32;
        let h = ((project.settings.height as u64 * w as u64 / project.settings.width as u64) as u32 + 1) & !1;
        s.width = w;
        s.height = h.max(16);
        s.quality = 26;
        s.overwrite = true;
        s
    } else {
        settings.ok_or("render settings are required")?
    };
    let plan = render::build_plan(&project, &settings, true).map_err(e)?;
    if !preview && Path::new(&plan.output).exists() && !settings.overwrite {
        return Err(format!("{} already exists; enable overwrite or choose another name", plan.output));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *guard = Some(cancel.clone());
    drop(guard);
    let job = if preview { "preview" } else { "export" };
    let overwrite = settings.overwrite;
    let duration_us = plan.duration_us;
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let started = std::time::Instant::now();
        // OpenFX clips are rendered through their plugins first; the plan then reads those files.
        let r = st
            .ofx_prerender(&tools, &project, &settings, &cancel, |done, total, label| {
                let p = render::RenderProgress { out_time_us: 0, duration_us, fraction: done as f64 / total.max(1) as f64, speed: None, fps: None, stage: Some(label) };
                let _ = app.emit("render-progress", serde_json::json!({ "job": job, "progress": p }));
            })
            .and_then(|pre| render::build_plan_with(&project, &settings, true, &pre))
            .and_then(|plan2| {
                render::execute(&tools, &plan2, overwrite, cancel.clone(), |p| {
                    let _ = app.emit("render-progress", serde_json::json!({ "job": job, "progress": p }));
                })
            });
        *st.render_cancel.lock().unwrap() = None;
        match r {
            Ok(res) => {
                if preview {
                    let _ = app.asset_protocol_scope().allow_file(&res.output);
                }
                let _ = app.emit("render-finished", serde_json::json!({ "job": job, "result": res, "seconds": started.elapsed().as_secs_f64() }));
            }
            Err(err) => {
                let _ = app.emit("render-failed", serde_json::json!({ "job": job, "error": err.to_string() }));
            }
        }
    });
    Ok(plan)
}

#[tauri::command]
pub fn cancel_render(st: State<AppState>) -> bool {
    if let Some(c) = st.render_cancel.lock().unwrap().as_ref() {
        c.store(true, Ordering::SeqCst);
        true
    } else {
        false
    }
}

// ---------------------------------------------------------------- revisions

fn author_or_default(author: &str) -> String {
    let a = author.trim();
    if a.is_empty() {
        std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "editor".into())
    } else {
        a.to_string()
    }
}

fn save_working(s: &mut Session) -> CoreResult<()> {
    let path = s.path.clone().ok_or_else(|| CoreError::Invalid("save the project first".into()))?;
    project::save_project(&s.project, &path)?;
    s.saved = Some(s.project.clone());
    Ok(())
}

#[derive(Serialize)]
pub struct VcsState {
    pub branches: Vec<BranchInfo>,
    pub log: Vec<RevisionSummary>,
    pub current: String,
    pub dirty: bool,
    pub merge: Option<MergeView>,
}

#[derive(Serialize)]
pub struct MergeView {
    pub theirs_branch: String,
    pub base: String,
    pub ours: String,
    pub theirs: String,
    pub outcome: MergeOutcome,
    pub incoming: Vec<DiffEntry>,
}

fn merge_view(store: &RevisionStore, ms: &MergeState) -> CoreResult<MergeView> {
    let base = store.get(&ms.base)?.body.snapshot;
    let ours = store.get(&ms.ours)?.body.snapshot;
    let theirs = store.get(&ms.theirs)?.body.snapshot;
    let outcome = merge::merge(&base, &ours, &theirs, &ms.choices)?;
    Ok(MergeView {
        theirs_branch: ms.theirs_branch.clone(),
        base: ms.base.clone(),
        ours: ms.ours.clone(),
        theirs: ms.theirs.clone(),
        incoming: diff_entries(&base, &theirs),
        outcome,
    })
}

#[tauri::command]
pub fn vcs_state(st: State<AppState>) -> Res<VcsState> {
    let s = st.session.lock().unwrap();
    let store = s.store().map_err(e)?;
    let merge = match read_merge_state(&store) {
        Some(ms) => Some(merge_view(&store, &ms).map_err(e)?),
        None => None,
    };
    Ok(VcsState {
        branches: store.branches().map_err(e)?,
        log: store.log().map_err(e)?,
        current: store.current_branch().map_err(e)?,
        dirty: store.is_dirty(&s.project).map_err(e)?,
        merge,
    })
}

#[tauri::command]
pub fn vcs_commit(app: AppHandle, st: State<AppState>, message: String, author: String) -> Res<View> {
    let mut s = st.session.lock().unwrap();
    let store = s.store().map_err(e)?;
    if read_merge_state(&store).is_some() {
        return Err("finish or abort the merge in progress first".into());
    }
    save_working(&mut s).map_err(e)?;
    store.commit(&s.project, &message, &author_or_default(&author), None).map_err(e)?;
    Ok(build_view(&app, &st, &mut s))
}

#[tauri::command]
pub fn vcs_create_branch(app: AppHandle, st: State<AppState>, name: String, from: Option<String>, switch: bool) -> Res<View> {
    let mut s = st.session.lock().unwrap();
    let store = s.store().map_err(e)?;
    let from = match from {
        Some(f) => f,
        None => store.head_revision().map_err(e)?.ok_or("commit a revision before branching")?,
    };
    store.create_branch(name.trim(), &from).map_err(e)?;
    if switch {
        switch_inner(&mut s, &store, name.trim(), "keep").map_err(e)?;
    }
    Ok(build_view(&app, &st, &mut s))
}

/// `mode`: "require_clean" (error if uncommitted changes), "stash" (preserve
/// them in history/stash then switch), or "keep" (carry the working copy
/// over unchanged — used right after creating a branch from HEAD).
fn switch_inner(s: &mut Session, store: &RevisionStore, name: &str, mode: &str) -> CoreResult<()> {
    if read_merge_state(store).is_some() {
        return Err(CoreError::Invalid("finish or abort the merge in progress first".into()));
    }
    let dirty = store.is_dirty(&s.project)? && store.head_revision()?.is_some();
    if mode == "keep" {
        store.switch_branch(name)?;
        s.head_cache = None;
        return Ok(());
    }
    if dirty {
        if mode != "stash" {
            return Err(CoreError::Dirty("you have uncommitted changes; commit them or stash them before switching".into()));
        }
        store.stash(&s.project, &format!("before switching to {name}"))?;
    }
    let snap = store.switch_branch(name)?;
    s.head_cache = None;
    if let Some(p) = snap {
        s.replace(&format!("Switch to {name}"), p);
        save_working(s)?;
    }
    Ok(())
}

#[tauri::command]
pub fn vcs_switch_branch(app: AppHandle, st: State<AppState>, name: String, mode: String) -> Res<View> {
    let mut s = st.session.lock().unwrap();
    let store = s.store().map_err(e)?;
    switch_inner(&mut s, &store, &name, &mode).map_err(e)?;
    Ok(build_view(&app, &st, &mut s))
}

#[tauri::command]
pub fn vcs_delete_branch(app: AppHandle, st: State<AppState>, name: String, force: bool) -> Res<View> {
    let mut s = st.session.lock().unwrap();
    let store = s.store().map_err(e)?;
    store.delete_branch(&name, force).map_err(e)?;
    Ok(build_view(&app, &st, &mut s))
}

/// Copy a historical revision into the working project. History is untouched;
/// uncommitted work is stashed first.
#[tauri::command]
pub fn vcs_restore(app: AppHandle, st: State<AppState>, revision: String) -> Res<View> {
    let mut s = st.session.lock().unwrap();
    let store = s.store().map_err(e)?;
    let rev = store.get(&revision).map_err(e)?;
    if store.is_dirty(&s.project).map_err(e)? {
        store.stash(&s.project, &format!("before restoring {}", short(&revision))).map_err(e)?;
    }
    s.replace(&format!("Restore {}", short(&revision)), rev.body.snapshot);
    save_working(&mut s).map_err(e)?;
    Ok(build_view(&app, &st, &mut s))
}

/// A revision id, a branch name, or "working".
fn resolve_ref(s: &Session, store: &RevisionStore, r: &str) -> CoreResult<Project> {
    if r == "working" {
        return Ok(s.project.clone());
    }
    if r.len() == 64 && store.exists(r) {
        return Ok(store.get(r)?.body.snapshot);
    }
    let id = store.branch_head(r)?.ok_or_else(|| CoreError::NotFound(format!("{r} has no revisions")))?;
    Ok(store.get(&id)?.body.snapshot)
}

#[tauri::command]
pub fn vcs_diff(st: State<AppState>, from: String, to: String) -> Res<Vec<DiffEntry>> {
    let s = st.session.lock().unwrap();
    let store = s.store().map_err(e)?;
    let a = resolve_ref(&s, &store, &from).map_err(e)?;
    let b = resolve_ref(&s, &store, &to).map_err(e)?;
    Ok(diff_entries(&a, &b))
}

#[derive(Serialize)]
pub struct MergeStart {
    pub result: String,
    pub view: View,
}

#[tauri::command]
pub fn vcs_merge_start(app: AppHandle, st: State<AppState>, branch: String) -> Res<MergeStart> {
    let mut s = st.session.lock().unwrap();
    let store = s.store().map_err(e)?;
    if read_merge_state(&store).is_some() {
        return Err("a merge is already in progress".into());
    }
    if store.is_dirty(&s.project).map_err(e)? {
        return Err("commit or stash your changes before merging".into());
    }
    let ours = store.head_revision().map_err(e)?.ok_or("commit a revision before merging")?;
    let theirs = store.branch_head(&branch).map_err(e)?.ok_or("that branch has no revisions")?;
    let base = store.merge_base(&ours, &theirs).map_err(e)?.ok_or("these branches share no history")?;
    let current = store.current_branch().map_err(e)?;
    let result = if base == theirs {
        "up_to_date".to_string()
    } else if base == ours {
        store.set_branch(&current, &theirs).map_err(e)?;
        let snap = store.get(&theirs).map_err(e)?.body.snapshot;
        s.head_cache = None;
        s.replace(&format!("Fast-forward to {branch}"), snap);
        save_working(&mut s).map_err(e)?;
        "fast_forward".to_string()
    } else {
        let ms = MergeState { base, ours, theirs, theirs_branch: branch.clone(), choices: HashMap::new() };
        let mv = merge_view(&store, &ms).map_err(e)?;
        atomic_write(&merge_state_path(&store), serde_json::to_string_pretty(&ms).unwrap().as_bytes()).map_err(e)?;
        if let Some(m) = mv.outcome.merged {
            s.replace(&format!("Merge {branch}"), m);
            "merged".to_string()
        } else {
            "conflicts".to_string()
        }
    };
    Ok(MergeStart { result, view: build_view(&app, &st, &mut s) })
}

#[tauri::command]
pub fn vcs_merge_resolve(app: AppHandle, st: State<AppState>, choices: HashMap<String, Side>) -> Res<View> {
    let mut s = st.session.lock().unwrap();
    let store = s.store().map_err(e)?;
    let mut ms = read_merge_state(&store).ok_or("no merge in progress")?;
    ms.choices.extend(choices);
    let mv = merge_view(&store, &ms).map_err(e)?;
    atomic_write(&merge_state_path(&store), serde_json::to_string_pretty(&ms).unwrap().as_bytes()).map_err(e)?;
    if let Some(m) = mv.outcome.merged {
        if s.project != m {
            s.replace(&format!("Merge {}", ms.theirs_branch), m);
        }
    }
    Ok(build_view(&app, &st, &mut s))
}

#[tauri::command]
pub fn vcs_merge_commit(app: AppHandle, st: State<AppState>, message: String, author: String) -> Res<View> {
    let mut s = st.session.lock().unwrap();
    let store = s.store().map_err(e)?;
    let ms = read_merge_state(&store).ok_or("no merge in progress")?;
    let mv = merge_view(&store, &ms).map_err(e)?;
    let merged = mv.outcome.merged.ok_or("resolve every conflict before committing the merge")?;
    if s.project != merged {
        s.replace("Merge result", merged);
    }
    save_working(&mut s).map_err(e)?;
    let msg = if message.trim().is_empty() { format!("Merge {} into {}", ms.theirs_branch, store.current_branch().map_err(e)?) } else { message };
    store.commit(&s.project, &msg, &author_or_default(&author), Some(&ms.theirs)).map_err(e)?;
    let _ = std::fs::remove_file(merge_state_path(&store));
    Ok(build_view(&app, &st, &mut s))
}

#[tauri::command]
pub fn vcs_merge_abort(app: AppHandle, st: State<AppState>) -> Res<View> {
    let mut s = st.session.lock().unwrap();
    let store = s.store().map_err(e)?;
    let ms = read_merge_state(&store).ok_or("no merge in progress")?;
    let ours = store.get(&ms.ours).map_err(e)?.body.snapshot;
    if s.project != ours {
        s.replace("Abort merge", ours);
        save_working(&mut s).map_err(e)?;
    }
    std::fs::remove_file(merge_state_path(&store)).map_err(|x| x.to_string())?;
    Ok(build_view(&app, &st, &mut s))
}

// ---------------------------------------------------------------- bundles

#[tauri::command]
pub async fn bundle_export(app: AppHandle, branches: Vec<String>, include_media: bool, path: String, author: String) -> Res<BundleExportReport> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let (store, project) = {
            let s = st.session.lock().unwrap();
            (s.store().map_err(e)?, s.project.clone())
        };
        let mut out = PathBuf::from(path);
        if out.extension().map(|x| x != "pirosman-bundle").unwrap_or(true) {
            out.set_extension("pirosman-bundle");
        }
        bundles::export_bundle(&store, &project, &branches, include_media, &author_or_default(&author), &out).map_err(e)
    })
    .await
    .map_err(|x| x.to_string())?
}

#[derive(Serialize)]
pub struct BundleImportResult {
    pub report: BundleImportReport,
    pub view: View,
}

#[tauri::command]
pub async fn bundle_import(app: AppHandle, path: String) -> Res<BundleImportResult> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let st = app2.state::<AppState>();
        let (store, project, proj_path) = {
            let s = st.session.lock().unwrap();
            (s.store().map_err(e)?, s.project.clone(), s.path.clone().unwrap())
        };
        let media_dir = proj_path.parent().unwrap_or(Path::new(".")).join("bundle-media");
        let report = bundles::import_bundle(&store, &project, Path::new(&path), &media_dir).map_err(e)?;
        // Auto-relink missing media to hash-verified extracted files.
        let mut s = st.session.lock().unwrap();
        let missing: Vec<(Uuid, String)> = s
            .project
            .media
            .iter()
            .filter(|m| !Path::new(&m.path).is_file())
            .filter_map(|m| m.sha256.clone().map(|h| (m.id, h)))
            .collect();
        if !missing.is_empty() {
            if let Ok(tools) = st.tools() {
                for (id, sha) in missing {
                    if let Some(found) = media::find_by_hash(&media_dir, &sha) {
                        let _ = s.edit("Relink from bundle", |p| media::relink(p, id, &found, &tools, false));
                    }
                }
            }
        }
        let view = build_view(&app2, &st, &mut s);
        Ok(BundleImportResult { report, view })
    })
    .await
    .map_err(|x| x.to_string())?
}


// ---------------------------------------------------------------- diagnostics

#[derive(Serialize)]
pub struct Diagnostics {
    pub tools: ToolReport,
    pub ofx: OfxReport,
    pub parity: serde_json::Value,
    pub app_version: String,
    pub cache_dir: String,
    pub history_dir: Option<String>,
}

#[tauri::command]
pub async fn diagnostics(app: AppHandle) -> Diagnostics {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let tools = diagnostics::tool_report();
        if let (Some(a), Some(b)) = (&tools.ffmpeg_path, &tools.ffprobe_path) {
            *st.tools.lock().unwrap() = Some(media::Tools { ffmpeg: a.into(), ffprobe: b.into() });
        }
        let history = st.session.lock().unwrap().path.as_ref().map(|p| history_dir(p).to_string_lossy().to_string());
        Diagnostics {
            tools,
            ofx: diagnostics::discover_ofx(&diagnostics::ofx_search_dirs()),
            parity: diagnostics::parity_registry(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            cache_dir: st.cache_dir.to_string_lossy().into(),
            history_dir: history,
        }
    })
    .await
    .expect("diagnostics task")
}

/// Re-detect the (bundled) FFmpeg and refresh the cached tools. Nothing is
/// installed or downloaded: the app ships its own FFmpeg.
#[tauri::command]
pub async fn ffmpeg_status(app: AppHandle) -> ToolReport {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let report = diagnostics::tool_report();
        *st.tools.lock().unwrap() = match (&report.ffmpeg_path, &report.ffprobe_path) {
            (Some(a), Some(b)) => Some(media::Tools { ffmpeg: a.into(), ffprobe: b.into() }),
            _ => None,
        };
        report
    })
    .await
    .expect("ffmpeg status task")
}

/// Open the bundled FFmpeg folder in Explorer (for the error dialog).
#[tauri::command]
pub fn reveal_ffmpeg_folder() -> Res<()> {
    let dir = media::bundled_dir().ok_or("no bundled FFmpeg folder is configured")?;
    let target = if dir.exists() { dir.to_path_buf() } else { dir.parent().map(Path::to_path_buf).ok_or("no install folder")? };
    #[cfg(windows)]
    let r = std::process::Command::new("explorer").arg(&target).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(&target).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(&target).spawn();
    r.map(|_| ()).map_err(|x| x.to_string())
}

#[tauri::command]
pub fn reveal_path(path: String) -> Res<()> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("{path} does not exist"));
    }
    #[cfg(windows)]
    let r = std::process::Command::new("explorer").arg(format!("/select,{}", p.display())).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg("-R").arg(&p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(p.parent().unwrap_or(&p)).spawn();
    r.map(|_| ()).map_err(|x| x.to_string())
}

// ---------------------------------------------------------------- clipboard

#[tauri::command]
pub fn copy_items(st: State<AppState>, ids: Vec<Uuid>) -> Res<usize> {
    let s = st.session.lock().unwrap();
    let cb = timeline::copy_items(&s.project, &ids).map_err(e)?;
    let n = cb.clips.len() + cb.titles.len();
    *st.clipboard.lock().unwrap() = Some(cb);
    Ok(n)
}

#[tauri::command]
pub fn paste_items(app: AppHandle, st: State<AppState>, at: i64) -> Res<View> {
    let cb = st.clipboard.lock().unwrap().clone().ok_or("the clipboard is empty; copy clips first (Ctrl+C)")?;
    edit(&app, &st, "Paste", |p| timeline::paste_items(p, &cb, at).map(|_| ()))
}

#[tauri::command]
pub fn duplicate_items(app: AppHandle, st: State<AppState>, ids: Vec<Uuid>) -> Res<View> {
    edit(&app, &st, "Duplicate", |p| timeline::duplicate_items(p, &ids).map(|_| ()))
}

// ---------------------------------------------------------------- recent projects

fn recent_file(st: &AppState) -> PathBuf {
    st.config_dir.join("recent-projects.json")
}

pub(crate) fn read_recent(st: &AppState) -> Vec<String> {
    std::fs::read_to_string(recent_file(st)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub(crate) fn write_recent(st: &AppState, list: Vec<String>) {
    let _ = atomic_write(&recent_file(st), serde_json::to_string_pretty(&list).unwrap().as_bytes());
}

pub(crate) fn remember_recent(st: &AppState, path: &str) {
    let mut list = read_recent(st);
    list.retain(|p| !p.eq_ignore_ascii_case(path));
    list.insert(0, path.to_string());
    list.truncate(12);
    write_recent(st, list);
}

#[derive(Serialize)]
pub struct RecentProject {
    pub path: String,
    pub name: String,
    pub exists: bool,
}

#[tauri::command]
pub fn recent_projects(st: State<AppState>) -> Vec<RecentProject> {
    read_recent(&st)
        .into_iter()
        .map(|p| RecentProject { exists: Path::new(&p).is_file(), name: Path::new(&p).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), path: p })
        .collect()
}

// ---------------------------------------------------------------- interchange

#[derive(Serialize)]
pub struct TimelineImport {
    pub view: View,
    pub format: String,
    pub warnings: Vec<String>,
}

/// Open an FCP 7 XML / OTIO / Resolve .drp/.drt file as a new, unsaved project.
#[tauri::command]
pub async fn import_timeline(app: AppHandle, path: String) -> Res<TimelineImport> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let st = app2.state::<AppState>();
        let mut r = pirosman_core::interchange::import_timeline_file(Path::new(&path)).map_err(e)?;
        if let Ok(tools) = st.tools() {
            r.warnings.extend(pirosman_core::interchange::enrich_media(&tools, &mut r.project));
        }
        r.project.validate().map_err(e)?;
        let media = r.project.media.clone();
        let mut s = st.session.lock().unwrap();
        *s = Session::new(r.project);
        let view = build_view(&app2, &st, &mut s);
        drop(s);
        spawn_media_jobs(app2.clone(), media);
        Ok(TimelineImport { view, format: r.format, warnings: r.warnings })
    })
    .await
    .map_err(|x| x.to_string())?
}

/// Write the graded, composited frame at `frame` as a PNG via the export pipeline.
#[tauri::command]
pub async fn grab_still(app: AppHandle, frame: i64, path: String) -> Res<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let tools = st.tools().map_err(e)?;
        let p = st.session.lock().unwrap().project.clone();
        let mut out = PathBuf::from(path);
        if out.extension().map(|x| !x.eq_ignore_ascii_case("png")).unwrap_or(true) {
            out.set_extension("png");
        }
        let mut s = RenderSettings::from_project(&p, &out.to_string_lossy());
        s.video_codec = render::VideoCodec::Png;
        s.audio_codec = render::AudioCodec::None;
        s.overwrite = true;
        s.range_in = Some(frame.max(0));
        s.range_out = Some(frame.max(0) + 1);
        let pre = st.ofx_prerender(&tools, &p, &s, &AtomicBool::new(false), |_, _, _| {}).map_err(e)?;
        let plan = render::build_plan_with(&p, &s, true, &pre).map_err(e)?;
        render::execute(&tools, &plan, true, Arc::new(AtomicBool::new(false)), |_| {}).map_err(e)?;
        Ok(out.to_string_lossy().into())
    })
    .await
    .map_err(|x| x.to_string())?
}

// ---------------------------------------------------------------- OpenFX

/// Installed OpenFX plugins for the Effects library. `rescan` re-describes every
/// plugin; otherwise unchanged binaries come from the scan cache.
#[tauri::command]
pub async fn ofx_catalog(app: AppHandle, rescan: bool) -> pirosman_core::ofx::Catalog {
    tauri::async_runtime::spawn_blocking(move || app.state::<AppState>().ofx_catalog(rescan)).await.expect("OpenFX scan task")
}

/// The exact composited frame at `frame`, rendered through OpenFX, for the
/// viewer while paused. `None` when no clip there has an enabled OpenFX effect.
#[tauri::command]
pub async fn ofx_preview_frame(app: AppHandle, frame: i64) -> Res<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let p = st.session.lock().unwrap().project.clone();
        let hit = p.tracks.iter().filter(|t| t.kind == TrackKind::Video && !t.muted).any(|t| t.clips.iter().any(|c| c.start <= frame && frame < c.end() && c.ofx.iter().any(|f| f.enabled)));
        if !hit {
            return Ok(None);
        }
        let tools = st.tools().map_err(e)?;
        let mut n = st.ofx_preview.lock().unwrap();
        *n += 1;
        let dir = st.cache_dir.join("preview");
        std::fs::create_dir_all(&dir).map_err(|x| x.to_string())?;
        let out = dir.join(format!("ofx-frame-{}.png", *n));
        let mut s = RenderSettings::from_project(&p, &out.to_string_lossy());
        let w = p.settings.width.min(1280);
        s.width = w & !1;
        s.height = (((p.settings.height as u64 * w as u64 / p.settings.width as u64) as u32 + 1) & !1).max(16);
        s.video_codec = render::VideoCodec::Png;
        s.audio_codec = render::AudioCodec::None;
        s.overwrite = true;
        s.range_in = Some(frame.max(0));
        s.range_out = Some(frame.max(0) + 1);
        let pre = st.ofx_prerender(&tools, &p, &s, &AtomicBool::new(false), |_, _, _| {}).map_err(e)?;
        let plan = render::build_plan_with(&p, &s, true, &pre).map_err(e)?;
        render::execute(&tools, &plan, true, Arc::new(AtomicBool::new(false)), |_| {}).map_err(e)?;
        // Handed over inline: a data URL needs no asset-protocol access to the cache.
        let bytes = std::fs::read(&out).map_err(|x| x.to_string())?;
        let _ = std::fs::remove_file(&out);
        Ok(Some(format!("data:image/png;base64,{}", base64(&bytes))))
    })
    .await
    .map_err(|x| x.to_string())?
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

#[tauri::command]
pub async fn resolve_inspect() -> pirosman_core::resolve_re::ResolveReport {
    tauri::async_runtime::spawn_blocking(pirosman_core::resolve_re::inspect_default).await.expect("inspection task")
}

// ---------------------------------------------------------------- live sync

#[derive(Serialize)]
pub struct HostInfo {
    pub port: u16,
    pub code: String,
    pub local_url: String,
    pub lan_urls: Vec<String>,
}

#[tauri::command]
pub fn collab_host_start(st: State<AppState>, port: Option<u16>) -> Res<HostInfo> {
    let mut hub = st.hub.lock().unwrap();
    if hub.is_none() {
        *hub = Some(pirosman_core::collab::start(port.unwrap_or(0)).map_err(|x| format!("could not start the Live Sync hub: {x}"))?);
    }
    let h = hub.as_ref().unwrap();
    Ok(HostInfo {
        port: h.port,
        code: h.code.clone(),
        local_url: format!("ws://127.0.0.1:{}", h.port),
        lan_urls: pirosman_core::collab::lan_addresses().into_iter().map(|ip| format!("ws://{ip}:{}", h.port)).collect(),
    })
}

#[tauri::command]
pub fn collab_host_stop(st: State<AppState>) -> bool {
    match st.hub.lock().unwrap().take() {
        Some(h) => {
            h.stop();
            true
        }
        None => false,
    }
}

/// Launch a second Pirosman process that joins this session (side-by-side testing).
#[tauri::command]
pub fn collab_spawn_peer(st: State<AppState>, name: String) -> Res<()> {
    let (url, code) = {
        let hub = st.hub.lock().unwrap();
        let h = hub.as_ref().ok_or("start hosting first")?;
        (format!("ws://127.0.0.1:{}", h.port), h.code.clone())
    };
    let exe = std::env::current_exe().map_err(|x| x.to_string())?;
    std::process::Command::new(exe)
        .args(["--join", &url, "--code", &code, "--name", &name])
        .spawn()
        .map_err(|x| format!("could not open a second window: {x}"))?;
    Ok(())
}

/// Replace the working project with a collaborator's snapshot (undoable).
#[tauri::command]
pub fn collab_apply_remote(app: AppHandle, st: State<AppState>, project: Project, author: String) -> Res<View> {
    project.validate().map_err(e)?;
    let mut s = st.session.lock().unwrap();
    if s.project != project {
        let media = project.media.clone();
        s.replace(&format!("Live edit by {}", author.chars().take(40).collect::<String>()), project);
        spawn_media_jobs(app.clone(), media);
    }
    Ok(build_view(&app, &st, &mut s))
}

#[derive(Serialize, Default)]
pub struct StartupArgs {
    pub join: Option<String>,
    pub code: Option<String>,
    pub name: Option<String>,
}

#[tauri::command]
pub fn startup_args() -> StartupArgs {
    let a: Vec<String> = std::env::args().collect();
    let get = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    StartupArgs { join: get("--join"), code: get("--code"), name: get("--name") }
}
//! Project Hub commands: the start screen's project shelf (recent projects
//! with metadata and thumbnails), project-file housekeeping (duplicate,
//! rename, move to Recycle Bin), creating a project from a revision bundle,
//! the cache/storage manager, and recent revisions across projects.

use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use pirosman_core::bundles;
use pirosman_core::media;
use pirosman_core::model::{MediaKind, Project, TrackKind};
use pirosman_core::project::{self, history_dir, PROJECT_EXTENSION};
use pirosman_core::revisions::RevisionStore;

use crate::commands::{read_recent, remember_recent, spawn_media_jobs, write_recent};
use crate::state::AppState;

type Res<T> = Result<T, String>;

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

fn mtime(p: &Path) -> Option<i64> {
    fs::metadata(p).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

fn thumb_for(st: &AppState, project_path: &str) -> PathBuf {
    let mut h = DefaultHasher::new();
    project_path.to_lowercase().hash(&mut h);
    st.cache_dir.join("project-thumbs").join(format!("{:016x}.jpg", h.finish()))
}

/// Picture for a project card: the first visible video clip, a third of the way in.
fn thumb_source(p: &Project) -> Option<(PathBuf, f64)> {
    let rate = p.settings.frame_rate;
    p.tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video && !t.muted)
        .flat_map(|t| t.clips.iter())
        .min_by_key(|c| c.start)
        .and_then(|c| {
            let m = p.media.iter().find(|m| m.id == c.media_id)?;
            let path = PathBuf::from(&m.path);
            if !path.is_file() {
                return None;
            }
            let at = if m.kind == MediaKind::Image { 0.0 } else { (c.source_in + c.source_len() / 3) as f64 * rate.den as f64 / rate.num as f64 };
            Some((path, at))
        })
}

#[derive(Serialize, Clone)]
pub struct HubProject {
    pub path: String,
    pub name: String,
    pub exists: bool,
    /// Unix seconds of the project file's last write.
    pub modified: Option<i64>,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub duration_frames: i64,
    pub clip_count: usize,
    pub media_count: usize,
    pub branch: Option<String>,
    pub revisions: usize,
    pub thumb: Option<String>,
    /// Why the file could not be read, if it exists but is unreadable.
    pub error: Option<String>,
    /// This is the project currently open in the editor.
    pub open: bool,
}

/// Recent projects with what the hub shows about each. Missing thumbnails are
/// rendered in the background; "hub-updated" fires when they are ready.
#[tauri::command]
pub async fn hub_projects(app: AppHandle) -> Vec<HubProject> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let st = app2.state::<AppState>();
        let open_path = st.session.lock().unwrap().path.clone();
        let mut jobs = vec![];
        let mut out = vec![];
        for path in read_recent(&st) {
            let p = Path::new(&path);
            let open = open_path.as_deref().map(|o| same_path(o, p)).unwrap_or(false);
            let mut card = HubProject {
                name: p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                exists: p.is_file(),
                modified: mtime(p),
                width: 0,
                height: 0,
                fps_num: 0,
                fps_den: 1,
                duration_frames: 0,
                clip_count: 0,
                media_count: 0,
                branch: None,
                revisions: 0,
                thumb: None,
                error: None,
                open,
                path: path.clone(),
            };
            if card.exists {
                match project::load_project(p) {
                    Ok(proj) => {
                        card.name = proj.name.clone();
                        card.width = proj.settings.width;
                        card.height = proj.settings.height;
                        card.fps_num = proj.settings.frame_rate.num;
                        card.fps_den = proj.settings.frame_rate.den;
                        card.duration_frames = proj.duration();
                        card.clip_count = proj.tracks.iter().map(|t| t.clips.len()).sum();
                        card.media_count = proj.media.len();
                        let hist = history_dir(p);
                        if hist.join("HEAD").is_file() {
                            if let Ok(store) = RevisionStore::open_or_init(&hist) {
                                card.branch = store.current_branch().ok();
                                card.revisions = store.log().map(|l| l.len()).unwrap_or(0);
                            }
                        }
                        let thumb = thumb_for(&st, &path);
                        let fresh = mtime(&thumb).zip(card.modified).map(|(t, m)| t >= m).unwrap_or(false);
                        card.thumb = st.cache_ref(&thumb);
                        if !fresh {
                            match thumb_source(&proj) {
                                Some((src, at)) => jobs.push((src, at, thumb)),
                                None => {
                                    // Nothing to show any more: drop a stale picture.
                                    let _ = fs::remove_file(&thumb);
                                    card.thumb = None;
                                }
                            }
                        }
                    }
                    Err(err) => card.error = Some(err.to_string()),
                }
            }
            out.push(card);
        }
        if !jobs.is_empty() {
            if let Ok(tools) = st.tools() {
                let app3 = app2.clone();
                std::thread::spawn(move || {
                    let mut any = false;
                    for (src, at, thumb) in jobs {
                        any |= media::make_thumbnail(&tools, &src, at, &thumb).is_ok();
                    }
                    if any {
                        let _ = app3.emit("hub-updated", ());
                    }
                });
            }
        }
        out
    })
    .await
    .unwrap_or_default()
}

fn unique_path(dir: &Path, stem: &str) -> PathBuf {
    let mut p = dir.join(format!("{stem}.{PROJECT_EXTENSION}"));
    let mut n = 2;
    while p.exists() || history_dir(&p).exists() {
        p = dir.join(format!("{stem} {n}.{PROJECT_EXTENSION}"));
        n += 1;
    }
    p
}

/// Copy a project file (not its history) as a new, independent project.
#[tauri::command]
pub fn hub_duplicate(st: State<AppState>, path: String) -> Res<String> {
    let src = Path::new(&path);
    let mut p = project::load_project(src).map_err(|x| x.to_string())?;
    p.id = Uuid::new_v4();
    p.name = format!("{} copy", p.name);
    let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Project".into());
    let target = unique_path(src.parent().unwrap_or(Path::new(".")), &format!("{stem} copy"));
    project::save_project(&p, &target).map_err(|x| x.to_string())?;
    let t = target.to_string_lossy().to_string();
    remember_recent(&st, &t);
    Ok(t)
}

fn valid_file_name(name: &str) -> Res<&str> {
    let n = name.trim();
    if n.is_empty() || n.len() > 120 || n.chars().any(|c| "<>:\"/\\|?*".contains(c) || c.is_control()) || n.ends_with('.') {
        return Err("Use a name without < > : \" / \\ | ? * and not ending in a dot.".into());
    }
    Ok(n)
}

/// Rename a project: the file, its backup and its history folder move
/// together, and the project's own name changes to match.
#[tauri::command]
pub fn hub_rename(st: State<AppState>, path: String, name: String) -> Res<String> {
    let name = valid_file_name(&name)?;
    let old = PathBuf::from(&path);
    let new = old.with_file_name(format!("{name}.{PROJECT_EXTENSION}"));
    if !same_path(&old, &new) && (new.exists() || history_dir(&new).exists()) {
        return Err(format!("A project named {name} already exists in that folder."));
    }
    let (old_hist, new_hist) = (history_dir(&old), history_dir(&new));
    fs::rename(&old, &new).map_err(|x| format!("Could not rename the project file: {x}"))?;
    let old_bak = old.with_file_name(format!("{}.bak", old.file_name().unwrap().to_string_lossy()));
    if old_bak.is_file() {
        let _ = fs::rename(&old_bak, new.with_file_name(format!("{}.bak", new.file_name().unwrap().to_string_lossy())));
    }
    if old_hist.is_dir() && !same_path(&old_hist, &new_hist) {
        if let Err(x) = fs::rename(&old_hist, &new_hist) {
            let _ = fs::rename(&new, &old); // keep file and history together
            return Err(format!("Could not move the revision history folder: {x}"));
        }
    }
    let mut p = project::load_project(&new).map_err(|x| x.to_string())?;
    p.name = name.to_string();
    project::save_project(&p, &new).map_err(|x| x.to_string())?;
    let new_s = new.to_string_lossy().to_string();
    let list = read_recent(&st).into_iter().map(|r| if same_path(Path::new(&r), &old) { new_s.clone() } else { r }).collect();
    write_recent(&st, list);
    // If it is open in the editor, follow the move without touching unsaved work.
    let mut s = st.session.lock().unwrap();
    if s.path.as_deref().map(|o| same_path(o, &old)).unwrap_or(false) {
        s.path = Some(new.clone());
        s.project.name = name.to_string();
        if let Some(saved) = s.saved.as_mut() {
            saved.name = name.to_string();
        }
        s.head_cache = None;
    }
    Ok(new_s)
}

/// Move a project file, its backup and its history to the Recycle Bin.
#[tauri::command]
pub fn hub_trash(st: State<AppState>, path: String) -> Res<()> {
    let p = PathBuf::from(&path);
    if st.session.lock().unwrap().path.as_deref().map(|o| same_path(o, &p)).unwrap_or(false) {
        return Err("This project is open in the editor. Open or create another project first.".into());
    }
    let mut items = vec![];
    if p.exists() {
        items.push(p.clone());
    }
    let bak = p.with_file_name(format!("{}.bak", p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()));
    if bak.exists() {
        items.push(bak);
    }
    if history_dir(&p).is_dir() {
        items.push(history_dir(&p));
    }
    if !items.is_empty() {
        trash::delete_all(&items).map_err(|x| format!("Could not move to the Recycle Bin: {x}"))?;
    }
    hub_forget(st, path);
    Ok(())
}

#[tauri::command]
pub fn hub_forget(st: State<AppState>, path: String) {
    let list = read_recent(&st).into_iter().filter(|r| !same_path(Path::new(r), Path::new(&path))).collect();
    write_recent(&st, list);
}

/// Create a new project from a `.pirosman-bundle`: the bundle's main (or first)
/// branch becomes the working project, its history is imported, and media
/// shipped in the bundle is linked by content hash. Returns the project path.
#[tauri::command]
pub async fn hub_import_archive(app: AppHandle, bundle: String, dest: String) -> Res<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let mut dest = PathBuf::from(dest);
        if dest.extension().map(|x| x != PROJECT_EXTENSION).unwrap_or(true) {
            dest.set_extension(PROJECT_EXTENSION);
        }
        if dest.exists() || history_dir(&dest).exists() {
            return Err(format!("{} already exists; choose a new name.", dest.display()));
        }
        let (branch, head, mut snapshot) = {
            let b = bundles::open_bundle(Path::new(&bundle)).map_err(|x| x.to_string())?;
            let (branch, head) = b
                .manifest
                .branches
                .get_key_value("main")
                .or_else(|| b.manifest.branches.iter().next())
                .map(|(k, v)| (k.clone(), v.clone()))
                .ok_or("The bundle contains no branches.")?;
            let snap = b.revisions.get(&head).ok_or("The bundle's branch head is missing.")?.body.snapshot.clone();
            (branch, head, snap)
        };
        project::save_project(&snapshot, &dest).map_err(|x| x.to_string())?;
        let store = RevisionStore::open_or_init(&history_dir(&dest)).map_err(|x| x.to_string())?;
        let media_dir = dest.parent().unwrap_or(Path::new(".")).join("bundle-media");
        bundles::import_bundle(&store, &snapshot, Path::new(&bundle), &media_dir).map_err(|x| x.to_string())?;
        store.set_branch("main", &head).map_err(|x| x.to_string())?;
        let _ = branch;
        let mut relinked = false;
        for m in snapshot.media.iter_mut() {
            if !Path::new(&m.path).is_file() {
                if let Some(found) = m.sha256.as_deref().and_then(|h| media::find_by_hash(&media_dir, h)) {
                    m.path = found.to_string_lossy().to_string();
                    relinked = true;
                }
            }
        }
        if relinked {
            project::save_project(&snapshot, &dest).map_err(|x| x.to_string())?;
        }
        let d = dest.to_string_lossy().to_string();
        remember_recent(&st, &d);
        Ok(d)
    })
    .await
    .map_err(|x| x.to_string())?
}

// ---------------------------------------------------------------- storage

#[derive(Serialize)]
pub struct StorageItem {
    pub key: String,
    pub label: String,
    pub detail: String,
    pub bytes: u64,
    pub files: u64,
    pub clearable: bool,
}

#[derive(Serialize)]
pub struct StorageReport {
    pub cache_dir: String,
    pub config_dir: String,
    pub items: Vec<StorageItem>,
    pub total: u64,
}

const CACHE_KINDS: [(&str, &str, &str); 7] = [
    ("proxies", "Preview proxies", "H.264 copies of media the viewer cannot play directly. Rebuilt when needed."),
    ("thumbs", "Media thumbnails", "Pictures in the media pool."),
    ("strips", "Timeline filmstrips", "Frames drawn along video clips on the timeline."),
    ("ofx", "OpenFX renders", "Clips rendered through OpenFX plugins, reused until the clip or its effects change."),
    ("waves", "Audio waveforms", "Waveform strips for audio clips."),
    ("preview", "Rendered previews", "The last “Render preview” of the timeline."),
    ("project-thumbs", "Project thumbnails", "Pictures on the Project Hub cards."),
];

fn dir_size(p: &Path) -> (u64, u64) {
    let mut bytes = 0;
    let mut files = 0;
    if let Ok(rd) = fs::read_dir(p) {
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                let (b, f) = dir_size(&path);
                bytes += b;
                files += f;
            } else if let Ok(md) = e.metadata() {
                bytes += md.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

#[tauri::command]
pub async fn storage_report(app: AppHandle) -> StorageReport {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let mut items = vec![];
        let (all, all_files) = dir_size(&st.cache_dir);
        let mut known = 0;
        let mut known_files = 0;
        for (key, label, detail) in CACHE_KINDS {
            let (bytes, files) = dir_size(&st.cache_dir.join(key));
            known += bytes;
            known_files += files;
            items.push(StorageItem { key: key.into(), label: label.into(), detail: detail.into(), bytes, files, clearable: true });
        }
        items.push(StorageItem {
            key: "other".into(),
            label: "Other app data".into(),
            detail: "Title text, scope frames and other working files.".into(),
            bytes: all.saturating_sub(known),
            files: all_files.saturating_sub(known_files),
            clearable: false,
        });
        StorageReport { cache_dir: st.cache_dir.to_string_lossy().into(), config_dir: st.config_dir.to_string_lossy().into(), items, total: all }
    })
    .await
    .expect("storage task")
}

/// Delete one cache category. Media for the open project is rebuilt in the background.
#[tauri::command]
pub fn storage_clear(app: AppHandle, st: State<AppState>, key: String) -> Res<()> {
    if !CACHE_KINDS.iter().any(|(k, _, _)| *k == key) {
        return Err(format!("{key} cannot be cleared"));
    }
    let dir = st.cache_dir.join(&key);
    if dir.is_dir() {
        fs::remove_dir_all(&dir).map_err(|x| format!("Could not clear {key}: {x} (a file may be in use)"))?;
    }
    if key == "ofx" {
        *st.ofx.lock().unwrap() = None;
    }
    if matches!(key.as_str(), "proxies" | "thumbs" | "waves" | "strips") {
        let media = st.session.lock().unwrap().project.media.clone();
        spawn_media_jobs(app.clone(), media);
    }
    if key == "preview" {
        let _ = app.emit("preview-cleared", ());
    }
    Ok(())
}

// ---------------------------------------------------------------- recent revisions

#[derive(Serialize)]
pub struct RecentRevision {
    pub project_path: String,
    pub project_name: String,
    pub id: String,
    pub message: String,
    pub author: String,
    pub created_at: String,
    pub branches: Vec<String>,
    pub clip_count: usize,
}

/// The newest revisions across recent projects, newest first.
#[tauri::command]
pub async fn recent_revisions(app: AppHandle, limit: Option<usize>) -> Vec<RecentRevision> {
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let mut out = vec![];
        for path in read_recent(&st) {
            let p = Path::new(&path);
            let hist = history_dir(p);
            if !p.is_file() || !hist.join("HEAD").is_file() {
                continue;
            }
            let name = project::load_project(p).map(|x| x.name).unwrap_or_else(|_| p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
            if let Ok(log) = RevisionStore::open_or_init(&hist).and_then(|s| s.log()) {
                for r in log.into_iter().take(8) {
                    out.push(RecentRevision {
                        project_path: path.clone(),
                        project_name: name.clone(),
                        id: r.id,
                        message: r.message,
                        author: r.author,
                        created_at: r.created_at,
                        branches: r.branches,
                        clip_count: r.clip_count,
                    });
                }
            }
        }
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        out.truncate(limit.unwrap_or(40));
        out
    })
    .await
    .unwrap_or_default()
}

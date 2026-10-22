//! Session state held by the desktop app: the authoritative working project,
//! its undo/redo history, and caches derived from it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use pirosman_core::history::EditHistory;
use pirosman_core::media::Tools;
use pirosman_core::model::{Project, ProjectSettings};
use pirosman_core::ofx::{self, Catalog};
use pirosman_core::render::{Prerendered, RenderSettings};
use uuid::Uuid;
use pirosman_core::project::history_dir;
use pirosman_core::revisions::RevisionStore;
use pirosman_core::{CoreError, CoreResult};

pub struct Session {
    pub project: Project,
    pub path: Option<PathBuf>,
    /// Project as last written to disk (for the "unsaved" indicator).
    pub saved: Option<Project>,
    pub history: EditHistory,
    /// Snapshot of the current branch head, cached for change marks.
    pub head_cache: Option<(String, Project)>,
}

impl Session {
    pub fn new(project: Project) -> Self {
        Self { project, path: None, saved: None, history: EditHistory::new(), head_cache: None }
    }

    /// Run an edit; on success the previous state becomes undoable.
    pub fn edit<T>(&mut self, label: &str, f: impl FnOnce(&mut Project) -> CoreResult<T>) -> CoreResult<T> {
        self.history.edit(&mut self.project, label, f)
    }

    /// Replace the whole working project (restore, switch, merge) as one undoable step.
    pub fn replace(&mut self, label: &str, p: Project) {
        self.history.replace(&mut self.project, label, p);
    }

    pub fn store(&self) -> CoreResult<RevisionStore> {
        let path = self.path.as_ref().ok_or_else(|| CoreError::Invalid("save the project before using revisions".into()))?;
        RevisionStore::open_or_init(&history_dir(path))
    }

    pub fn head_snapshot(&mut self, store: &RevisionStore) -> CoreResult<Option<Project>> {
        let Some(h) = store.head_revision()? else {
            self.head_cache = None;
            return Ok(None);
        };
        if let Some((id, p)) = &self.head_cache {
            if *id == h {
                return Ok(Some(p.clone()));
            }
        }
        let p = store.get(&h)?.body.snapshot;
        self.head_cache = Some((h, p.clone()));
        Ok(Some(p))
    }
}

pub struct AppState {
    pub session: Mutex<Session>,
    pub tools: Mutex<Option<Tools>>,
    pub render_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub cache_dir: PathBuf,
    pub config_dir: PathBuf,
    pub clipboard: Mutex<Option<pirosman_core::timeline::Clipboard>>,
    pub hub: Mutex<Option<pirosman_core::collab::Hub>>,
    /// Installed OpenFX plugins, from the last scan.
    pub ofx: Mutex<Option<Catalog>>,
    /// One OpenFX viewer frame at a time.
    pub ofx_preview: Mutex<u64>,
}

impl AppState {
    pub fn new(cache_dir: PathBuf, config_dir: PathBuf) -> Self {
        Self {
            session: Mutex::new(Session::new(Project::new_minimal("Untitled", ProjectSettings::default()))),
            tools: Mutex::new(pirosman_core::media::discover_tools().ok()),
            render_cancel: Mutex::new(None),
            cache_dir,
            config_dir,
            clipboard: Mutex::new(None),
            hub: Mutex::new(None),
            ofx: Mutex::new(None),
            ofx_preview: Mutex::new(0),
        }
    }

    /// The OpenFX catalog; scans (in plugin subprocesses) when there is none yet or `force`.
    pub fn ofx_catalog(&self, force: bool) -> Catalog {
        if !force {
            if let Some(c) = self.ofx.lock().unwrap().clone() {
                return c;
            }
        }
        let c = ofx::scan_catalog(&self.cache_dir.join("ofx").join("scan.json"), force);
        *self.ofx.lock().unwrap() = Some(c.clone());
        c
    }

    /// Run the clips in range that carry OpenFX effects through their plugins,
    /// returning the intermediates the render plan should read instead.
    pub fn ofx_prerender(&self, tools: &Tools, p: &Project, s: &RenderSettings, cancel: &AtomicBool, progress: impl FnMut(i64, i64, String)) -> CoreResult<HashMap<Uuid, Prerendered>> {
        let any = p.tracks.iter().any(|t| t.clips.iter().any(|c| c.ofx.iter().any(|f| f.enabled)));
        if !any {
            return Ok(HashMap::new());
        }
        let cat = self.ofx_catalog(false);
        let jobs = ofx::plan_jobs(p, s, &cat, &self.cache_dir.join("ofx").join("renders"))?;
        ofx::prepare(tools, &jobs, cancel, progress)
    }

    pub fn tools(&self) -> CoreResult<Tools> {
        let mut t = self.tools.lock().unwrap();
        if t.is_none() {
            *t = Some(pirosman_core::media::discover_tools()?);
        }
        Ok(t.clone().unwrap())
    }

    pub fn proxy_path(&self, sha: &str) -> PathBuf {
        self.cache_dir.join("proxies").join(format!("{sha}.mp4"))
    }
    pub fn thumb_path(&self, sha: &str) -> PathBuf {
        self.cache_dir.join("thumbs").join(format!("{sha}.jpg"))
    }
    pub fn wave_path(&self, sha: &str) -> PathBuf {
        self.cache_dir.join("waves").join(format!("{sha}.png"))
    }
    pub fn strip_path(&self, sha: &str, frames: u32) -> PathBuf {
        self.cache_dir.join("strips").join(format!("{sha}-{frames}.jpg"))
    }
    /// A cache file as the UI loads it: `pcache:<path relative to the cache>`
    /// (served by the app's own scheme), or None when the file does not exist.
    pub fn cache_ref(&self, p: &Path) -> Option<String> {
        if !p.is_file() {
            return None;
        }
        let rel = p.strip_prefix(&self.cache_dir).ok()?;
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        Some(format!("pcache:{}", parts.join("/")))
    }
    pub fn preview_render_path(&self) -> PathBuf {
        self.cache_dir.join("preview").join("timeline-preview.mp4")
    }
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

//! Project-aware revision store.
//!
//! Layout of `<project>.history/`:
//! ```text
//! HEAD                      "ref: <branch>"
//! objects/<id>.json         immutable revision (id = SHA-256 of its body)
//! refs/heads/<branch>       revision id the branch points at
//! stash/<timestamp>.json    working copies preserved before a destructive switch
//! MERGE_STATE.json          in-progress merge (see merge.rs)
//! ```
//! Revisions are write-once and content-addressed: reading one recomputes its
//! hash and rejects tampered or corrupted objects. Parents must already exist,
//! so history is always a DAG. Media files are never copied into history;
//! revisions record media identities (id + SHA-256) only.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};
use crate::model::Project;
use crate::project::atomic_write_no_backup as atomic_write;

pub const DEFAULT_BRANCH: &str = "main";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaManifestEntry {
    pub media_id: Uuid,
    pub name: String,
    pub sha256: Option<String>,
    pub size: u64,
}

/// Everything hashed into a revision id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RevisionBody {
    pub project_id: Uuid,
    pub parents: Vec<String>,
    pub created_at: String,
    pub author: String,
    pub message: String,
    pub media_manifest: Vec<MediaManifestEntry>,
    pub snapshot: Project,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Revision {
    pub id: String,
    #[serde(flatten)]
    pub body: RevisionBody,
}

impl Revision {
    pub fn compute_id(body: &RevisionBody) -> CoreResult<String> {
        let bytes = serde_json::to_vec(body)?;
        Ok(hex::encode(Sha256::digest(&bytes)))
    }

    pub fn verify(&self) -> CoreResult<()> {
        let id = Self::compute_id(&self.body)?;
        if id != self.id {
            return Err(CoreError::Integrity(format!("revision {} content hash is {id}; object is corrupted or was altered", short(&self.id))));
        }
        Ok(())
    }
}

pub fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionSummary {
    pub id: String,
    pub parents: Vec<String>,
    pub created_at: String,
    pub author: String,
    pub message: String,
    pub branches: Vec<String>,
    pub clip_count: usize,
    pub duration_frames: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    pub name: String,
    pub head: Option<String>,
    pub current: bool,
}

pub struct RevisionStore {
    root: PathBuf,
}

pub fn validate_branch_name(name: &str) -> CoreResult<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.contains("..")
        && !name.contains("//")
        && !name.split('/').any(|seg| seg.starts_with('.') || seg.ends_with(".bak") || seg.contains(".tmp-"))
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'));
    if ok {
        Ok(())
    } else {
        Err(CoreError::Invalid(format!("branch name {name:?} must be 1–64 characters of letters, digits, - _ . /")))
    }
}

fn is_hex_id(id: &str) -> bool {
    id.len() == 64 && id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

impl RevisionStore {
    /// Open the store at `root`, creating it (with an unborn `main` branch) if absent.
    pub fn open_or_init(root: &Path) -> CoreResult<Self> {
        let s = Self { root: root.to_path_buf() };
        if !root.join("HEAD").exists() {
            fs::create_dir_all(root.join("objects"))?;
            fs::create_dir_all(root.join("refs").join("heads"))?;
            fs::create_dir_all(root.join("stash"))?;
            atomic_write(&root.join("HEAD"), format!("ref: {DEFAULT_BRANCH}\n").as_bytes())?;
        }
        Ok(s)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn obj_path(&self, id: &str) -> PathBuf {
        self.root.join("objects").join(format!("{id}.json"))
    }

    fn ref_path(&self, branch: &str) -> PathBuf {
        let mut p = self.root.join("refs").join("heads");
        for part in branch.split('/') {
            p.push(part);
        }
        p
    }

    pub fn current_branch(&self) -> CoreResult<String> {
        let head = fs::read_to_string(self.root.join("HEAD"))?;
        let name = head.trim().strip_prefix("ref: ").ok_or_else(|| CoreError::Integrity("HEAD is malformed".into()))?.to_string();
        validate_branch_name(&name)?;
        Ok(name)
    }

    pub fn branch_head(&self, branch: &str) -> CoreResult<Option<String>> {
        validate_branch_name(branch)?;
        let p = self.ref_path(branch);
        if !p.exists() {
            return Ok(None);
        }
        let id = fs::read_to_string(p)?.trim().to_string();
        if !is_hex_id(&id) {
            return Err(CoreError::Integrity(format!("branch {branch} points at malformed id {id:?}")));
        }
        Ok(Some(id))
    }

    pub fn head_revision(&self) -> CoreResult<Option<String>> {
        self.branch_head(&self.current_branch()?)
    }

    pub fn exists(&self, id: &str) -> bool {
        is_hex_id(id) && self.obj_path(id).exists()
    }

    /// Read and integrity-check a revision.
    pub fn get(&self, id: &str) -> CoreResult<Revision> {
        if !is_hex_id(id) {
            return Err(CoreError::Invalid(format!("{id:?} is not a revision id")));
        }
        let text = fs::read_to_string(self.obj_path(id)).map_err(|_| CoreError::NotFound(format!("revision {}", short(id))))?;
        let rev: Revision = serde_json::from_str(&text).map_err(|e| CoreError::Integrity(format!("revision {} is unreadable: {e}", short(id))))?;
        if rev.id != id {
            return Err(CoreError::Integrity(format!("object file {} contains revision {}", short(id), short(&rev.id))));
        }
        rev.verify()?;
        Ok(rev)
    }

    /// Write a revision object. Existing identical objects are left alone;
    /// parents must exist. Returns true when newly written.
    pub fn put(&self, rev: &Revision) -> CoreResult<bool> {
        rev.verify()?;
        for p in &rev.body.parents {
            if !self.exists(p) {
                return Err(CoreError::Integrity(format!("revision {} has unknown parent {}", short(&rev.id), short(p))));
            }
        }
        let path = self.obj_path(&rev.id);
        if path.exists() {
            return Ok(false);
        }
        rev.body.snapshot.validate()?;
        // create_new makes revisions write-once even under races.
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        f.write_all(serde_json::to_string_pretty(rev)?.as_bytes())?;
        f.sync_all()?;
        Ok(true)
    }

    pub fn set_branch(&self, branch: &str, id: &str) -> CoreResult<()> {
        validate_branch_name(branch)?;
        if !self.exists(id) {
            return Err(CoreError::NotFound(format!("revision {}", short(id))));
        }
        let p = self.ref_path(branch);
        if let Some(d) = p.parent() {
            fs::create_dir_all(d)?;
        }
        atomic_write(&p, format!("{id}\n").as_bytes())
    }

    /// Create an immutable revision of `project` on the current branch.
    /// `extra_parent` is the other side of a merge.
    pub fn commit(&self, project: &Project, message: &str, author: &str, extra_parent: Option<&str>) -> CoreResult<Revision> {
        let message = message.trim();
        if message.is_empty() {
            return Err(CoreError::Invalid("a revision needs a description".into()));
        }
        project.validate()?;
        let branch = self.current_branch()?;
        let mut parents = vec![];
        if let Some(h) = self.branch_head(&branch)? {
            let head = self.get(&h)?;
            if head.body.project_id != project.id {
                return Err(CoreError::Integrity("this history belongs to a different project".into()));
            }
            if extra_parent.is_none() && head.body.snapshot == *project {
                return Err(CoreError::Invalid("nothing changed since the last revision".into()));
            }
            parents.push(h);
        }
        if let Some(x) = extra_parent {
            if !self.exists(x) {
                return Err(CoreError::NotFound(format!("merge parent {}", short(x))));
            }
            parents.push(x.to_string());
        }
        let body = RevisionBody {
            project_id: project.id,
            parents,
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            author: author.trim().chars().take(80).collect(),
            message: message.chars().take(2000).collect(),
            media_manifest: project
                .media
                .iter()
                .map(|m| MediaManifestEntry { media_id: m.id, name: m.name.clone(), sha256: m.sha256.clone(), size: m.size })
                .collect(),
            snapshot: project.clone(),
        };
        let rev = Revision { id: Revision::compute_id(&body)?, body };
        self.put(&rev)?;
        self.set_branch(&branch, &rev.id)?;
        Ok(rev)
    }

    pub fn branches(&self) -> CoreResult<Vec<BranchInfo>> {
        let cur = self.current_branch()?;
        let mut out = BTreeMap::new();
        let base = self.root.join("refs").join("heads");
        let mut stack = vec![base.clone()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d)?.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(rel) = p.strip_prefix(&base) {
                    let name = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/");
                    if validate_branch_name(&name).is_ok() {
                        out.insert(name.clone(), self.branch_head(&name)?);
                    }
                }
            }
        }
        out.entry(cur.clone()).or_insert(None);
        Ok(out.into_iter().map(|(name, head)| BranchInfo { current: name == cur, name, head }).collect())
    }

    pub fn create_branch(&self, name: &str, from: &str) -> CoreResult<()> {
        validate_branch_name(name)?;
        if self.ref_path(name).exists() {
            return Err(CoreError::Conflict(format!("branch {name} already exists")));
        }
        self.get(from)?;
        self.set_branch(name, from)
    }

    /// Point HEAD at another branch and return that branch's snapshot.
    /// The caller is responsible for preserving uncommitted work first.
    pub fn switch_branch(&self, name: &str) -> CoreResult<Option<Project>> {
        validate_branch_name(name)?;
        let head = self.branch_head(name)?;
        if head.is_none() && self.current_branch()? != name {
            return Err(CoreError::NotFound(format!("branch {name}")));
        }
        let snap = match &head {
            Some(id) => Some(self.get(id)?.body.snapshot),
            None => None,
        };
        atomic_write(&self.root.join("HEAD"), format!("ref: {name}\n").as_bytes())?;
        Ok(snap)
    }

    /// Delete a branch. Refuses the current branch, and refuses a branch
    /// whose head is not reachable from any other branch unless `force`.
    pub fn delete_branch(&self, name: &str, force: bool) -> CoreResult<()> {
        if self.current_branch()? == name {
            return Err(CoreError::Invalid("cannot delete the branch you are on".into()));
        }
        let head = self.branch_head(name)?.ok_or_else(|| CoreError::NotFound(format!("branch {name}")))?;
        if !force {
            let mut reachable = false;
            for b in self.branches()?.into_iter().filter(|b| b.name != name) {
                if let Some(h) = b.head {
                    if self.ancestors(&h)?.contains(&head) {
                        reachable = true;
                        break;
                    }
                }
            }
            if !reachable {
                return Err(CoreError::Conflict(format!("branch {name} has revisions no other branch contains; delete anyway to discard them from the graph")));
            }
        }
        fs::remove_file(self.ref_path(name))?;
        Ok(())
    }

    /// All ancestors of `id`, including itself.
    pub fn ancestors(&self, id: &str) -> CoreResult<HashSet<String>> {
        let mut seen = HashSet::new();
        let mut q = VecDeque::from([id.to_string()]);
        while let Some(x) = q.pop_front() {
            if seen.insert(x.clone()) {
                for p in self.get(&x)?.body.parents {
                    q.push_back(p);
                }
            }
        }
        Ok(seen)
    }

    /// Nearest common ancestor (breadth-first from `b`).
    pub fn merge_base(&self, a: &str, b: &str) -> CoreResult<Option<String>> {
        let anc = self.ancestors(a)?;
        let mut seen = HashSet::new();
        let mut q = VecDeque::from([b.to_string()]);
        while let Some(x) = q.pop_front() {
            if anc.contains(&x) {
                return Ok(Some(x));
            }
            if seen.insert(x.clone()) {
                for p in self.get(&x)?.body.parents {
                    q.push_back(p);
                }
            }
        }
        Ok(None)
    }

    /// Every revision reachable from any branch, newest first.
    pub fn log(&self) -> CoreResult<Vec<RevisionSummary>> {
        let branches = self.branches()?;
        let mut labels: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut all = HashSet::new();
        for b in &branches {
            if let Some(h) = &b.head {
                labels.entry(h.clone()).or_default().push(b.name.clone());
                all.extend(self.ancestors(h)?);
            }
        }
        let mut out = vec![];
        for id in all {
            let r = self.get(&id)?;
            out.push(RevisionSummary {
                branches: labels.get(&id).cloned().unwrap_or_default(),
                clip_count: r.body.snapshot.tracks.iter().map(|t| t.clips.len()).sum(),
                duration_frames: r.body.snapshot.duration(),
                id: r.id,
                parents: r.body.parents,
                created_at: r.body.created_at,
                author: r.body.author,
                message: r.body.message,
            });
        }
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        Ok(out)
    }

    /// Preserve a working copy before it would be replaced.
    pub fn stash(&self, project: &Project, reason: &str) -> CoreResult<PathBuf> {
        let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ");
        let p = self.root.join("stash").join(format!("{ts}.json"));
        let v = serde_json::json!({ "reason": reason, "project": project });
        fs::create_dir_all(self.root.join("stash"))?;
        atomic_write(&p, serde_json::to_string_pretty(&v)?.as_bytes())?;
        Ok(p)
    }

    pub fn is_dirty(&self, project: &Project) -> CoreResult<bool> {
        match self.head_revision()? {
            None => Ok(true),
            Some(h) => Ok(self.get(&h)?.body.snapshot != *project),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixtures::*;
    use crate::timeline::*;

    fn store() -> (tempfile::TempDir, RevisionStore) {
        let d = tempfile::tempdir().unwrap();
        let s = RevisionStore::open_or_init(&d.path().join("film.history")).unwrap();
        (d, s)
    }

    #[test]
    fn commit_creates_immutable_chain() {
        let (_d, s) = store();
        let (mut p, mid) = project();
        let r1 = s.commit(&p, "empty timeline", "alice", None).unwrap();
        assert!(r1.body.parents.is_empty());
        assert!(s.commit(&p, "again", "alice", None).is_err(), "nothing changed");
        let v1 = p.tracks[0].id;
        place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: None, source_out: Some(50) }).unwrap();
        let r2 = s.commit(&p, "add intro", "alice", None).unwrap();
        assert_eq!(r2.body.parents, vec![r1.id.clone()]);
        assert_eq!(s.head_revision().unwrap(), Some(r2.id.clone()));
        assert_eq!(s.get(&r1.id).unwrap().body.snapshot.tracks[0].clips.len(), 0, "r1 unchanged");
        assert!(s.ancestors(&r2.id).unwrap().contains(&r1.id));
        let log = s.log().unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].branches, vec!["main".to_string()]);
        assert!(!s.is_dirty(&p).unwrap());
        // Ref updates leave no backup or temp files that could pose as branches.
        let names: Vec<String> = s.branches().unwrap().into_iter().map(|b| b.name).collect();
        assert_eq!(names, vec!["main".to_string()]);
        let stray = fs::read_dir(s.root().join("refs").join("heads")).unwrap().count();
        assert_eq!(stray, 1);
        assert!(validate_branch_name("main.bak").is_err() && validate_branch_name(".hidden").is_err());
    }

    #[test]
    fn tampered_objects_are_rejected() {
        let (_d, s) = store();
        let (p, _) = project();
        let r = s.commit(&p, "first", "a", None).unwrap();
        let path = s.obj_path(&r.id);
        let text = fs::read_to_string(&path).unwrap().replace("\"first\"", "\"forged\"");
        fs::write(&path, text).unwrap();
        assert!(matches!(s.get(&r.id), Err(CoreError::Integrity(_))));
    }

    #[test]
    fn branches_switch_and_restore() {
        let (_d, s) = store();
        let (mut p, mid) = project();
        let base = s.commit(&p, "base", "a", None).unwrap();
        s.create_branch("alt-ending", &base.id).unwrap();
        let v1 = p.tracks[0].id;
        place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: None, source_out: Some(50) }).unwrap();
        let main2 = s.commit(&p, "main edit", "a", None).unwrap();
        let alt = s.switch_branch("alt-ending").unwrap().unwrap();
        assert_eq!(alt.tracks[0].clips.len(), 0);
        assert_eq!(s.current_branch().unwrap(), "alt-ending");
        // Commit on alt does not move main.
        let mut q = alt.clone();
        place_clip(&mut q, PlaceClip { media_id: mid, track_id: v1, start: 100, source_in: None, source_out: Some(20) }).unwrap();
        let alt2 = s.commit(&q, "alt edit", "a", None).unwrap();
        assert_eq!(s.branch_head("main").unwrap(), Some(main2.id.clone()));
        assert_eq!(s.merge_base(&main2.id, &alt2.id).unwrap(), Some(base.id.clone()));
        // Restore = read an old snapshot; history is untouched.
        let restored = s.get(&base.id).unwrap().body.snapshot;
        assert_eq!(restored.tracks[0].clips.len(), 0);
        assert!(s.exists(&main2.id));
        // Deletion safety.
        assert!(s.delete_branch("alt-ending", false).is_err(), "current branch");
        s.switch_branch("main").unwrap();
        assert!(matches!(s.delete_branch("alt-ending", false), Err(CoreError::Conflict(_))));
        s.delete_branch("alt-ending", true).unwrap();
        assert!(s.create_branch("../evil", &base.id).is_err());
    }

    #[test]
    fn stash_preserves_work() {
        let (_d, s) = store();
        let (p, _) = project();
        let path = s.stash(&p, "switch").unwrap();
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(v["project"]["id"], serde_json::json!(p.id));
    }
}

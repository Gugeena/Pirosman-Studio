//! `.pirosman-bundle` revision exchange (format v1).
//!
//! A bundle is a ZIP archive with exactly this layout:
//! ```text
//! manifest.json                 BundleManifest
//! revisions/<sha256>.json       one immutable revision per file
//! media/<sha256>.<ext>          optional media payloads, named by content hash
//! ```
//! Import validates every entry name against that layout (rejecting path
//! traversal and anything unexpected), checks the schema version and project
//! identity, verifies each revision's content hash and ancestry, verifies
//! media hashes, and never overwrites local branches: incoming branch heads
//! land under `incoming/<name>`.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};
use crate::media::sha256_file;
use crate::model::Project;
use crate::revisions::{short, validate_branch_name, Revision, RevisionStore};

pub const BUNDLE_FORMAT: &str = "pirosman-bundle";
pub const BUNDLE_VERSION: u32 = 1;
const MAX_MANIFEST: u64 = 8 << 20;
const MAX_REVISION: u64 = 64 << 20;
const MAX_MEDIA: u64 = 64 << 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleMedia {
    pub media_id: Uuid,
    pub name: String,
    pub sha256: Option<String>,
    pub size: u64,
    /// Archive entry when the file itself is included.
    pub entry: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    pub format: String,
    pub version: u32,
    pub project_id: Uuid,
    pub project_name: String,
    pub created_at: String,
    pub author: String,
    pub branches: BTreeMap<String, String>,
    /// Revision ids, parents before children.
    pub revisions: Vec<String>,
    pub media: Vec<BundleMedia>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleExportReport {
    pub path: String,
    pub revisions: usize,
    pub media_included: usize,
    pub bytes: u64,
}

fn topo(store: &RevisionStore, heads: &[String]) -> CoreResult<Vec<Revision>> {
    let mut out = vec![];
    let mut done = HashSet::new();
    fn visit(store: &RevisionStore, id: &str, done: &mut HashSet<String>, out: &mut Vec<Revision>) -> CoreResult<()> {
        if done.contains(id) {
            return Ok(());
        }
        let r = store.get(id)?;
        for p in &r.body.parents {
            visit(store, p, done, out)?;
        }
        done.insert(id.to_string());
        out.push(r);
        Ok(())
    }
    for h in heads {
        visit(store, h, &mut done, &mut out)?;
    }
    Ok(out)
}

fn ext_of(path: &str) -> String {
    let e = Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !e.is_empty() && e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()) {
        e
    } else {
        "bin".into()
    }
}

/// Export the given branches (and all their ancestry) to a bundle.
pub fn export_bundle(store: &RevisionStore, project: &Project, branches: &[String], include_media: bool, author: &str, out: &Path) -> CoreResult<BundleExportReport> {
    if branches.is_empty() {
        return Err(CoreError::Invalid("choose at least one branch to export".into()));
    }
    let mut heads = BTreeMap::new();
    for b in branches {
        let h = store.branch_head(b)?.ok_or_else(|| CoreError::NotFound(format!("branch {b} has no revisions yet")))?;
        heads.insert(b.clone(), h);
    }
    let revs = topo(store, &heads.values().cloned().collect::<Vec<_>>())?;
    // Union of media referenced by any exported revision.
    let mut media: BTreeMap<Uuid, BundleMedia> = BTreeMap::new();
    let mut payloads: Vec<(String, PathBuf, String)> = vec![];
    for r in &revs {
        for m in &r.body.snapshot.media {
            media.entry(m.id).or_insert_with(|| BundleMedia { media_id: m.id, name: m.name.clone(), sha256: m.sha256.clone(), size: m.size, entry: None });
        }
    }
    if include_media {
        for bm in media.values_mut() {
            let Some(sha) = bm.sha256.clone() else { continue };
            // Prefer the working project's current path for this media id.
            let path = project.media(bm.media_id).map(|m| m.path.clone());
            if let Some(p) = path.filter(|p| Path::new(p).is_file()) {
                let entry = format!("media/{sha}.{}", ext_of(&p));
                payloads.push((entry.clone(), PathBuf::from(&p), sha));
                bm.entry = Some(entry);
            }
        }
    }
    let manifest = BundleManifest {
        format: BUNDLE_FORMAT.into(),
        version: BUNDLE_VERSION,
        project_id: project.id,
        project_name: project.name.clone(),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        author: author.into(),
        branches: heads,
        revisions: revs.iter().map(|r| r.id.clone()).collect(),
        media: media.into_values().collect(),
    };

    let tmp = out.with_extension("partial");
    {
        let f = fs::File::create(&tmp)?;
        let mut zw = zip::ZipWriter::new(f);
        let deflate = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
        let z = |e: zip::result::ZipError| CoreError::Bundle(e.to_string());
        zw.start_file("manifest.json", deflate).map_err(z)?;
        zw.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
        for r in &revs {
            zw.start_file(format!("revisions/{}.json", r.id), deflate).map_err(z)?;
            zw.write_all(serde_json::to_string_pretty(r)?.as_bytes())?;
        }
        for (entry, path, sha) in &payloads {
            if sha256_file(path)? != *sha {
                return Err(CoreError::Bundle(format!("{} changed since it was imported; relink or re-hash it before bundling", path.display())));
            }
            zw.start_file(entry.as_str(), stored).map_err(z)?;
            std::io::copy(&mut fs::File::open(path)?, &mut zw)?;
        }
        zw.finish().map_err(z)?;
    }
    fs::rename(&tmp, out)?;
    Ok(BundleExportReport { path: out.to_string_lossy().into(), revisions: revs.len(), media_included: payloads.len(), bytes: fs::metadata(out)?.len() })
}

enum Entry {
    Manifest,
    Revision(String),
    Media(String),
}

fn classify(name: &str) -> CoreResult<Entry> {
    let bad = || CoreError::Bundle(format!("unexpected archive entry {name:?}"));
    if name == "manifest.json" {
        return Ok(Entry::Manifest);
    }
    let hex64 = |s: &str| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
    if let Some(rest) = name.strip_prefix("revisions/") {
        let id = rest.strip_suffix(".json").ok_or_else(bad)?;
        return if hex64(id) { Ok(Entry::Revision(id.into())) } else { Err(bad()) };
    }
    if let Some(rest) = name.strip_prefix("media/") {
        let (sha, ext) = rest.split_once('.').ok_or_else(bad)?;
        return if hex64(sha) && !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
            Ok(Entry::Media(sha.into()))
        } else {
            Err(bad())
        };
    }
    Err(bad())
}

fn read_limited(r: &mut impl Read, limit: u64, what: &str) -> CoreResult<Vec<u8>> {
    let mut buf = vec![];
    r.take(limit + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > limit {
        return Err(CoreError::Bundle(format!("{what} exceeds the size limit")));
    }
    Ok(buf)
}

pub struct OpenedBundle {
    pub manifest: BundleManifest,
    pub revisions: HashMap<String, Revision>,
    archive: zip::ZipArchive<fs::File>,
    media_entries: HashMap<String, String>,
}

/// Open and fully validate a bundle without changing anything on disk.
pub fn open_bundle(path: &Path) -> CoreResult<OpenedBundle> {
    let f = fs::File::open(path).map_err(|e| CoreError::Bundle(format!("cannot open {}: {e}", path.display())))?;
    let mut za = zip::ZipArchive::new(f).map_err(|e| CoreError::Bundle(format!("not a valid bundle archive: {e}")))?;
    let mut manifest: Option<BundleManifest> = None;
    let mut revisions = HashMap::new();
    let mut media_entries = HashMap::new();
    for i in 0..za.len() {
        let mut e = za.by_index(i).map_err(|e| CoreError::Bundle(e.to_string()))?;
        let name = e.name().to_string();
        if e.is_dir() {
            continue;
        }
        if e.enclosed_name().is_none() || name.contains('\\') || name.starts_with('/') || name.contains("..") {
            return Err(CoreError::Bundle(format!("unsafe archive path {name:?}")));
        }
        match classify(&name)? {
            Entry::Manifest => {
                let bytes = read_limited(&mut e, MAX_MANIFEST, "manifest")?;
                manifest = Some(serde_json::from_slice(&bytes).map_err(|e| CoreError::Bundle(format!("manifest is malformed: {e}")))?);
            }
            Entry::Revision(id) => {
                let bytes = read_limited(&mut e, MAX_REVISION, "revision")?;
                let r: Revision = serde_json::from_slice(&bytes).map_err(|e| CoreError::Bundle(format!("revision {} is malformed: {e}", short(&id))))?;
                if r.id != id {
                    return Err(CoreError::Bundle(format!("entry {name} holds revision {}", short(&r.id))));
                }
                r.verify().map_err(|e| CoreError::Bundle(e.to_string()))?;
                revisions.insert(id, r);
            }
            Entry::Media(sha) => {
                if e.size() > MAX_MEDIA {
                    return Err(CoreError::Bundle(format!("{name} exceeds the size limit")));
                }
                media_entries.insert(name, sha);
            }
        }
    }
    let manifest = manifest.ok_or_else(|| CoreError::Bundle("manifest.json is missing".into()))?;
    if manifest.format != BUNDLE_FORMAT {
        return Err(CoreError::Bundle(format!("format tag {:?} is not {BUNDLE_FORMAT:?}", manifest.format)));
    }
    if manifest.version != BUNDLE_VERSION {
        return Err(CoreError::Bundle(format!("bundle version {} is not supported (this build reads v{BUNDLE_VERSION})", manifest.version)));
    }
    let listed: HashSet<&String> = manifest.revisions.iter().collect();
    if listed.len() != revisions.len() || !revisions.keys().all(|k| listed.contains(k)) {
        return Err(CoreError::Bundle("manifest revision list does not match archive contents".into()));
    }
    for (b, h) in &manifest.branches {
        validate_branch_name(b).map_err(|e| CoreError::Bundle(e.to_string()))?;
        if !revisions.contains_key(h) {
            return Err(CoreError::Bundle(format!("branch {b} points outside the bundle")));
        }
    }
    for r in revisions.values() {
        if r.body.project_id != manifest.project_id {
            return Err(CoreError::Bundle(format!("revision {} belongs to another project", short(&r.id))));
        }
        r.body.snapshot.validate().map_err(|e| CoreError::Bundle(format!("revision {} has an invalid project: {e}", short(&r.id))))?;
    }
    for m in &manifest.media {
        if let Some(e) = &m.entry {
            if media_entries.get(e) != m.sha256.as_ref() {
                return Err(CoreError::Bundle(format!("media entry {e} does not match its manifest record")));
            }
        }
    }
    Ok(OpenedBundle { manifest, revisions, archive: za, media_entries })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleImportReport {
    pub project_name: String,
    pub author: String,
    pub new_revisions: usize,
    pub known_revisions: usize,
    /// Local branch name → incoming head id.
    pub branches: BTreeMap<String, String>,
    pub media_extracted: Vec<String>,
    pub media_missing: Vec<String>,
}

/// Import a validated bundle into `store`. Incoming branches are written as
/// `incoming/<name>`; local branches are never touched. Included media is
/// extracted into `media_dir` (hash-verified).
pub fn import_bundle(store: &RevisionStore, local_project: &Project, path: &Path, media_dir: &Path) -> CoreResult<BundleImportReport> {
    let mut b = open_bundle(path)?;
    if b.manifest.project_id != local_project.id {
        return Err(CoreError::Bundle(format!(
            "this bundle belongs to project {:?} ({}), not the open project",
            b.manifest.project_name, b.manifest.project_id
        )));
    }
    // Ancestry: every parent must be in the bundle or already local.
    for r in b.revisions.values() {
        for p in &r.body.parents {
            if !b.revisions.contains_key(p) && !store.exists(p) {
                return Err(CoreError::Bundle(format!("revision {} needs parent {} which neither side has", short(&r.id), short(p))));
            }
        }
    }
    let mut new = 0;
    let mut known = 0;
    for id in &b.manifest.revisions {
        let r = &b.revisions[id];
        if store.put(r)? {
            new += 1;
        } else {
            known += 1;
        }
    }
    let mut branches = BTreeMap::new();
    for (name, head) in &b.manifest.branches {
        let local = format!("incoming/{name}");
        store.set_branch(&local, head)?;
        branches.insert(local, head.clone());
    }
    let mut extracted = vec![];
    let mut missing = vec![];
    let entries: Vec<(String, String)> = b.media_entries.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for m in b.manifest.media.clone() {
        let local_ok = local_project.media(m.media_id).map(|lm| Path::new(&lm.path).is_file()).unwrap_or(false);
        match &m.entry {
            Some(entry) if !local_ok => {
                let sha = entries.iter().find(|(k, _)| k == entry).map(|(_, v)| v.clone()).unwrap_or_default();
                fs::create_dir_all(media_dir)?;
                let safe_name: String = m.name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
                let dest = media_dir.join(format!("{}-{safe_name}", &sha[..8]));
                if !dest.exists() {
                    let tmp = dest.with_extension("partial");
                    {
                        let mut e = b.archive.by_name(entry).map_err(|e| CoreError::Bundle(e.to_string()))?;
                        let mut f = fs::File::create(&tmp)?;
                        std::io::copy(&mut e, &mut f)?;
                    }
                    if sha256_file(&tmp)? != sha {
                        let _ = fs::remove_file(&tmp);
                        return Err(CoreError::Bundle(format!("media {} failed its SHA-256 check", m.name)));
                    }
                    fs::rename(&tmp, &dest)?;
                }
                extracted.push(dest.to_string_lossy().to_string());
            }
            None if !local_ok => missing.push(m.name.clone()),
            _ => {}
        }
    }
    Ok(BundleImportReport {
        project_name: b.manifest.project_name.clone(),
        author: b.manifest.author.clone(),
        new_revisions: new,
        known_revisions: known,
        branches,
        media_extracted: extracted,
        media_missing: missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixtures::*;
    use crate::timeline::*;

    fn setup() -> (tempfile::TempDir, RevisionStore, Project) {
        let d = tempfile::tempdir().unwrap();
        let s = RevisionStore::open_or_init(&d.path().join("a.history")).unwrap();
        let (mut p, mid) = project();
        let media_file = d.path().join("intro.mp4");
        fs::write(&media_file, b"fake media bytes").unwrap();
        p.media[0].path = media_file.to_string_lossy().into();
        p.media[0].sha256 = Some(sha256_file(&media_file).unwrap());
        s.commit(&p, "first", "ana", None).unwrap();
        let v1 = p.tracks[0].id;
        place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: None, source_out: Some(25) }).unwrap();
        s.commit(&p, "second", "ana", None).unwrap();
        (d, s, p)
    }

    #[test]
    fn export_import_roundtrip() {
        let (d, s, p) = setup();
        let out = d.path().join("cut.pirosman-bundle");
        let rep = export_bundle(&s, &p, &["main".into()], true, "ana", &out).unwrap();
        assert_eq!((rep.revisions, rep.media_included), (2, 1));
        // Collaborator with an empty history and no media on disk.
        let s2 = RevisionStore::open_or_init(&d.path().join("b.history")).unwrap();
        let mut theirs_view = p.clone();
        theirs_view.media[0].path = "/nowhere/intro.mp4".into();
        let imp = import_bundle(&s2, &theirs_view, &out, &d.path().join("b-media")).unwrap();
        assert_eq!(imp.new_revisions, 2);
        assert_eq!(imp.branches.keys().next().unwrap(), "incoming/main");
        assert_eq!(imp.media_extracted.len(), 1);
        assert_eq!(s2.get(&imp.branches["incoming/main"]).unwrap().body.snapshot, p);
        // Re-import is idempotent.
        let again = import_bundle(&s2, &theirs_view, &out, &d.path().join("b-media")).unwrap();
        assert_eq!((again.new_revisions, again.known_revisions), (0, 2));
    }

    #[test]
    fn rejects_foreign_project() {
        let (d, s, p) = setup();
        let out = d.path().join("x.pirosman-bundle");
        export_bundle(&s, &p, &["main".into()], false, "ana", &out).unwrap();
        let (other, _) = project();
        let s2 = RevisionStore::open_or_init(&d.path().join("c.history")).unwrap();
        assert!(matches!(import_bundle(&s2, &other, &out, d.path()), Err(CoreError::Bundle(m)) if m.contains("belongs to project")));
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zw = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (n, b) in entries {
            zw.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            zw.write_all(b).unwrap();
        }
        zw.finish().unwrap();
    }

    #[test]
    fn rejects_traversal_and_malformed() {
        let d = tempfile::tempdir().unwrap();
        let z = d.path().join("evil.zip");
        write_zip(&z, &[("../../evil.txt", b"x")]);
        assert!(matches!(open_bundle(&z), Err(CoreError::Bundle(m)) if m.contains("unsafe") || m.contains("unexpected")));
        write_zip(&z, &[("notes/readme.txt", b"x")]);
        assert!(matches!(open_bundle(&z), Err(CoreError::Bundle(m)) if m.contains("unexpected archive entry")));
        write_zip(&z, &[("manifest.json", b"{not json")]);
        assert!(matches!(open_bundle(&z), Err(CoreError::Bundle(m)) if m.contains("malformed")));
        let m = serde_json::json!({"format":"pirosman-bundle","version":9,"project_id":Uuid::new_v4(),"project_name":"x","created_at":"","author":"","branches":{},"revisions":[],"media":[]});
        write_zip(&z, &[("manifest.json", m.to_string().as_bytes())]);
        assert!(matches!(open_bundle(&z), Err(CoreError::Bundle(m)) if m.contains("version 9")));
        fs::write(&z, b"not a zip").unwrap();
        assert!(open_bundle(&z).is_err());
    }

    #[test]
    fn rejects_tampered_revision() {
        let (d, s, p) = setup();
        let out = d.path().join("t.pirosman-bundle");
        export_bundle(&s, &p, &["main".into()], false, "ana", &out).unwrap();
        // Rewrite the archive with one revision message altered.
        let mut za = zip::ZipArchive::new(fs::File::open(&out).unwrap()).unwrap();
        let mut entries: Vec<(String, Vec<u8>)> = vec![];
        for i in 0..za.len() {
            let mut e = za.by_index(i).unwrap();
            let mut b = vec![];
            e.read_to_end(&mut b).unwrap();
            let name = e.name().to_string();
            if name.starts_with("revisions/") {
                b = String::from_utf8(b).unwrap().replace("\"second\"", "\"forged\"").into_bytes();
            }
            entries.push((name, b));
        }
        let t2 = d.path().join("t2.pirosman-bundle");
        let refs: Vec<(&str, &[u8])> = entries.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
        write_zip(&t2, &refs);
        assert!(matches!(open_bundle(&t2), Err(CoreError::Bundle(_))));
    }
}

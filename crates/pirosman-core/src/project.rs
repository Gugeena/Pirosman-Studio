//! Project file persistence: versioned JSON with validation on load and
//! atomic writes on save.

use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{CoreError, CoreResult};
use crate::model::{Project, PROJECT_FORMAT, SCHEMA_VERSION};

pub const PROJECT_EXTENSION: &str = "pirosman";

/// Parse and validate a project from JSON text, with readable errors.
pub fn parse_project(text: &str) -> CoreResult<Project> {
    let v: Value = serde_json::from_str(text).map_err(|e| CoreError::Format(format!("not valid JSON ({e})")))?;
    let format = v.get("format").and_then(Value::as_str).unwrap_or("");
    if format != PROJECT_FORMAT {
        return Err(CoreError::Format(format!(
            "this is not a Pirosman project (format tag {format:?}, expected {PROJECT_FORMAT:?})"
        )));
    }
    let ver = v.get("schema_version").and_then(Value::as_u64).unwrap_or(0);
    if ver == 0 || ver > SCHEMA_VERSION as u64 {
        return Err(CoreError::Format(format!(
            "unsupported schema version {ver}; this build reads versions 1–{SCHEMA_VERSION}"
        )));
    }
    let mut p: Project = serde_json::from_value(v).map_err(|e| CoreError::Format(format!("project structure is invalid: {e}")))?;
    p.sort_clips();
    p.validate()?;
    Ok(p)
}

pub fn load_project(path: &Path) -> CoreResult<Project> {
    let text = fs::read_to_string(path).map_err(|e| CoreError::Format(format!("cannot read {}: {e}", path.display())))?;
    parse_project(&text)
}

/// Canonical serialization (stable field order, pretty-printed).
pub fn to_json(p: &Project) -> CoreResult<String> {
    Ok(serde_json::to_string_pretty(p)?)
}

/// Write `bytes` to `path` atomically: write a sibling temp file, fsync it,
/// keep the previous version as `<file>.bak`, then rename into place.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> CoreResult<()> {
    atomic_write_opts(path, bytes, true)
}

/// Like [`atomic_write`] but without the `.bak` copy (used for small
/// metadata files such as revision refs, where a stray backup would be
/// mistaken for data).
pub fn atomic_write_no_backup(path: &Path, bytes: &[u8]) -> CoreResult<()> {
    atomic_write_opts(path, bytes, false)
}

fn atomic_write_opts(path: &Path, bytes: &[u8], backup: bool) -> CoreResult<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let file_name = path.file_name().ok_or_else(|| CoreError::Invalid("path has no file name".into()))?.to_string_lossy();
    let tmp = dir.join(format!(".{file_name}.tmp-{}", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if backup && path.exists() {
        let bak = dir.join(format!("{file_name}.bak"));
        let _ = fs::copy(path, &bak);
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        CoreError::Io(e)
    })
}

pub fn save_project(p: &Project, path: &Path) -> CoreResult<()> {
    p.validate()?;
    atomic_write(path, to_json(p)?.as_bytes())
}

/// Directory holding the revision store for a project file:
/// `<dir>/<stem>.history/`.
pub fn history_dir(project_path: &Path) -> PathBuf {
    let stem = project_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "project".into());
    project_path.parent().unwrap_or(Path::new(".")).join(format!("{stem}.history"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixtures::*;
    use crate::timeline::{place_clip, PlaceClip};

    #[test]
    fn save_and_reopen_preserves_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cut.pirosman");
        let (mut p, mid) = project();
        let v1 = p.tracks[0].id;
        place_clip(&mut p, PlaceClip { media_id: mid, track_id: v1, start: 12, source_in: Some(3), source_out: Some(90) }).unwrap();
        save_project(&p, &path).unwrap();
        save_project(&p, &path).unwrap();
        assert!(dir.path().join("cut.pirosman.bak").exists());
        let back = load_project(&path).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn load_reports_useful_errors() {
        assert!(matches!(parse_project("{"), Err(CoreError::Format(m)) if m.contains("not valid JSON")));
        assert!(matches!(parse_project(r#"{"format":"other"}"#), Err(CoreError::Format(m)) if m.contains("not a Pirosman project")));
        let r = parse_project(r#"{"format":"pirosman-project","schema_version":7}"#);
        assert!(matches!(r, Err(CoreError::Format(m)) if m.contains("unsupported schema version 7")));
    }

    #[test]
    fn history_dir_is_sibling() {
        let p = Path::new("/a/b/film.pirosman");
        assert_eq!(history_dir(p), Path::new("/a/b/film.history"));
    }
}

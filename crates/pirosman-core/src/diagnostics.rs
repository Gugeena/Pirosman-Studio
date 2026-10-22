//! Dependency checks, OpenFX plugin discovery, and the feature-status registry.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use crate::media::{bundled_dir, command, discover, tool_version, Discovery, ToolSource};

/// The feature-status registry, embedded at compile time. It is the single
/// source of truth for the Feature status tab of the in-app Diagnostics dialog.
pub const PARITY_JSON: &str = include_str!("parity.json");

pub const STATUSES: [&str; 6] = [
    "IMPLEMENTED AND TESTED",
    "IMPLEMENTED, NOT YET VERIFIED",
    "PROTOTYPE",
    "DEMO MODE",
    "PLANNED",
    "BLOCKED",
];

pub fn parity_registry() -> Value {
    serde_json::from_str(PARITY_JSON).expect("parity.json is valid JSON (checked by tests)")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolReport {
    pub found: bool,
    pub ffmpeg_path: Option<String>,
    pub ffprobe_path: Option<String>,
    pub ffmpeg_version: Option<String>,
    pub ffprobe_version: Option<String>,
    pub encoders: Vec<String>,
    pub error: Option<String>,
    /// "bundled", "override" or "system".
    #[serde(default)]
    pub source: Option<ToolSource>,
    /// Why the bundled FFmpeg was not used, when a fallback is active.
    #[serde(default)]
    pub warning: Option<String>,
    /// Folder the app looks in for its bundled FFmpeg.
    #[serde(default)]
    pub bundled_dir: Option<String>,
}

pub fn tool_report() -> ToolReport {
    let bundled_dir = bundled_dir().map(|p| p.to_string_lossy().to_string());
    match discover() {
        Err(e) => ToolReport {
            found: false,
            ffmpeg_path: None,
            ffprobe_path: None,
            ffmpeg_version: None,
            ffprobe_version: None,
            encoders: vec![],
            error: Some(e.to_string()),
            source: None,
            warning: None,
            bundled_dir,
        },
        Ok(Discovery { tools: t, source, warning, .. }) => {
            let enc = command(&t.ffmpeg).args(["-hide_banner", "-encoders"]).output().ok();
            let listing = enc.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
            let encoders = ["libx264", "libx265", "mpeg4", "prores_ks", "dnxhd", "aac", "pcm_s16le"]
                .iter()
                .filter(|e| listing.lines().any(|l| l.split_whitespace().nth(1) == Some(**e)))
                .map(|s| s.to_string())
                .collect();
            ToolReport {
                found: true,
                ffmpeg_version: tool_version(&t.ffmpeg),
                ffprobe_version: tool_version(&t.ffprobe),
                ffmpeg_path: Some(t.ffmpeg.to_string_lossy().into()),
                ffprobe_path: Some(t.ffprobe.to_string_lossy().into()),
                encoders,
                error: None,
                source: Some(source),
                warning,
                bundled_dir,
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfxPlugin {
    pub name: String,
    pub path: String,
    /// Binaries found under Contents/<arch>/ (listed, never loaded).
    pub binaries: Vec<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfxReport {
    pub searched: Vec<String>,
    pub plugins: Vec<OfxPlugin>,
    pub note: String,
}

pub fn ofx_search_dirs() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Ok(extra) = std::env::var("OFX_PLUGIN_PATH") {
        v.extend(std::env::split_paths(&extra));
    }
    if cfg!(windows) {
        let cf = std::env::var("CommonProgramFiles").unwrap_or_else(|_| "C:\\Program Files\\Common Files".into());
        v.push(Path::new(&cf).join("OFX").join("Plugins"));
    } else if cfg!(target_os = "macos") {
        v.push(PathBuf::from("/Library/OFX/Plugins"));
    } else {
        v.push(PathBuf::from("/usr/OFX/Plugins"));
    }
    v
}

/// Enumerate `*.ofx.bundle` directories. Plugin binaries are never opened,
/// loaded, or executed: discovery is not hosting.
pub fn discover_ofx(dirs: &[PathBuf]) -> OfxReport {
    let mut plugins = vec![];
    for d in dirs {
        let Ok(rd) = fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if !p.is_dir() || !name.to_lowercase().ends_with(".ofx.bundle") {
                continue;
            }
            let mut binaries = vec![];
            if let Ok(arches) = fs::read_dir(p.join("Contents")) {
                for a in arches.flatten().filter(|a| a.path().is_dir()) {
                    if let Ok(files) = fs::read_dir(a.path()) {
                        for f in files.flatten() {
                            if f.file_name().to_string_lossy().to_lowercase().ends_with(".ofx") {
                                binaries.push(format!("{}/{}", a.file_name().to_string_lossy(), f.file_name().to_string_lossy()));
                            }
                        }
                    }
                }
            }
            let status = if binaries.is_empty() { "bundle found, no binary for a known architecture" } else { "discovered (not loaded)" };
            plugins.push(OfxPlugin { name: name.trim_end_matches(".ofx.bundle").trim_end_matches(".OFX.bundle").into(), path: p.to_string_lossy().into(), binaries, status: status.into() });
        }
    }
    plugins.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    OfxReport {
        searched: dirs.iter().map(|d| d.to_string_lossy().to_string()).collect(),
        plugins,
        note: "Bundles found in these folders. Their plugins are described and rendered by Pirosman's OpenFX host in a separate process; see the Effects library for what each one offers.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_valid_and_uses_only_known_statuses() {
        let reg = parity_registry();
        let features = reg["features"].as_array().expect("features array");
        assert!(features.len() >= 11);
        let mut ids = std::collections::HashSet::new();
        for f in features {
            let id = f["id"].as_str().unwrap();
            assert!(ids.insert(id.to_string()), "duplicate id {id}");
            let st = f["status"].as_str().unwrap();
            assert!(STATUSES.contains(&st), "{id} has unknown status {st}");
            for k in ["subsystem", "reference", "evidence_type", "evidence", "notes", "approach", "module", "verification", "limitations"] {
                assert!(f.get(k).is_some(), "{id} lacks {k}");
            }
        }
    }

    #[test]
    fn ofx_discovery_lists_without_loading() {
        let d = tempfile::tempdir().unwrap();
        let b = d.path().join("Grain.ofx.bundle").join("Contents").join("Win64");
        fs::create_dir_all(&b).unwrap();
        fs::write(b.join("Grain.ofx"), b"not a real dll").unwrap();
        fs::create_dir_all(d.path().join("Empty.ofx.bundle")).unwrap();
        fs::create_dir_all(d.path().join("NotAPlugin")).unwrap();
        let r = discover_ofx(&[d.path().to_path_buf(), d.path().join("missing")]);
        assert_eq!(r.plugins.len(), 2);
        assert_eq!(r.plugins[1].name, "Grain");
        assert_eq!(r.plugins[1].binaries, vec!["Win64/Grain.ofx".to_string()]);
    }
}

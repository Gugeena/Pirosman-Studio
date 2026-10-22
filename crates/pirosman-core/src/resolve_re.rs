//! Read-only inspection of a local DaVinci Resolve installation, for
//! interoperability research.
//!
//! What this does: lists the installed binaries and folders, reads each PE
//! file's *headers* (machine, timestamp, version resource) and *export-name
//! table*, counts bundled LUTs / plugins, and reads the documented interchange
//! constants from Blackmagic's own scripting stub (`DaVinciResolveScript.pyi`).
//! What it never does: disassemble or decompile code, load or execute any
//! Resolve binary, touch licence data, or copy anything out of the install.
//! Findings are shown in Help → Resolve binary & interop inspector; no Resolve
//! code is reused.

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::error::{CoreError, CoreResult};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeInfo {
    pub machine: String,
    /// COFF link timestamp (unix seconds).
    pub timestamp: u32,
    pub file_version: Option<String>,
    pub export_count: usize,
    /// First export names (sorted as stored), for evidence.
    pub exports_sample: Vec<String>,
}

fn rd<const N: usize>(f: &mut File, off: u64) -> CoreResult<[u8; N]> {
    let mut b = [0u8; N];
    f.seek(SeekFrom::Start(off))?;
    f.read_exact(&mut b)?;
    Ok(b)
}
fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

struct Section {
    va: u32,
    vsize: u32,
    raw: u32,
    rsize: u32,
}

fn rva_to_off(secs: &[Section], rva: u32) -> Option<u64> {
    secs.iter()
        .find(|s| rva >= s.va && rva < s.va + s.vsize.max(s.rsize))
        .map(|s| (rva - s.va + s.raw) as u64)
}

fn read_cstr(f: &mut File, off: u64, max: usize) -> CoreResult<String> {
    f.seek(SeekFrom::Start(off))?;
    let mut buf = vec![0u8; max];
    let n = f.read(&mut buf)?;
    let end = buf[..n].iter().position(|b| *b == 0).unwrap_or(n);
    Ok(String::from_utf8_lossy(&buf[..end]).into_owned())
}

/// Parse PE headers, the export-name table and the fixed version resource.
/// Reads only headers and the few bytes those tables point at (a 600 MB
/// executable costs a handful of small reads).
pub fn inspect_pe(path: &Path, sample: usize) -> CoreResult<PeInfo> {
    let mut f = File::open(path)?;
    let dos: [u8; 64] = rd(&mut f, 0)?;
    if &dos[0..2] != b"MZ" {
        return Err(CoreError::Invalid(format!("{} is not a PE file", path.display())));
    }
    let pe_off = u32le(&dos, 0x3c) as u64;
    let hdr: [u8; 24] = rd(&mut f, pe_off)?;
    if &hdr[0..4] != b"PE\0\0" {
        return Err(CoreError::Invalid("missing PE signature".into()));
    }
    let machine = match u16le(&hdr, 4) {
        0x8664 => "x86-64".to_string(),
        0x14c => "x86".to_string(),
        0xaa64 => "ARM64".to_string(),
        m => format!("0x{m:04x}"),
    };
    let nsec = u16le(&hdr, 6) as usize;
    let timestamp = u32le(&hdr, 8);
    let opt_size = u16le(&hdr, 20) as usize;
    let opt_off = pe_off + 24;
    let mut opt = vec![0u8; opt_size];
    f.seek(SeekFrom::Start(opt_off))?;
    f.read_exact(&mut opt)?;
    let pe32plus = u16le(&opt, 0) == 0x20b;
    let dd = if pe32plus { 112 } else { 96 };
    let dir = |i: usize| -> (u32, u32) {
        let o = dd + i * 8;
        if o + 8 <= opt.len() {
            (u32le(&opt, o), u32le(&opt, o + 4))
        } else {
            (0, 0)
        }
    };
    let mut secs = vec![];
    let sec_off = opt_off + opt_size as u64;
    for i in 0..nsec {
        let s: [u8; 40] = rd(&mut f, sec_off + (i * 40) as u64)?;
        secs.push(Section { vsize: u32le(&s, 8), va: u32le(&s, 12), rsize: u32le(&s, 16), raw: u32le(&s, 20) });
    }

    // Export names.
    let mut exports = vec![];
    let mut export_count = 0;
    let (erva, _) = dir(0);
    if erva != 0 {
        if let Some(eo) = rva_to_off(&secs, erva) {
            let ed: [u8; 40] = rd(&mut f, eo)?;
            export_count = u32le(&ed, 24) as usize;
            if let Some(names) = rva_to_off(&secs, u32le(&ed, 32)) {
                for i in 0..export_count.min(sample) {
                    let nrva = u32::from_le_bytes(rd(&mut f, names + (i * 4) as u64)?);
                    if let Some(no) = rva_to_off(&secs, nrva) {
                        exports.push(read_cstr(&mut f, no, 256)?);
                    }
                }
            }
        }
    }

    // RT_VERSION (16) → first name → first language → VS_FIXEDFILEINFO.
    let mut file_version = None;
    let (rrva, _) = dir(2);
    if let Some(root) = (rrva != 0).then(|| rva_to_off(&secs, rrva)).flatten() {
        let find = |f: &mut File, dir_off: u64, want: Option<u32>| -> CoreResult<Option<u32>> {
            let h: [u8; 16] = rd(f, dir_off)?;
            let n = u16le(&h, 12) as u64 + u16le(&h, 14) as u64;
            for i in 0..n.min(512) {
                let e: [u8; 8] = rd(f, dir_off + 16 + i * 8)?;
                let id = u32le(&e, 0);
                if want.is_none() || want == Some(id) {
                    return Ok(Some(u32le(&e, 4)));
                }
            }
            Ok(None)
        };
        let sub = |v: u32| root + (v & 0x7fff_ffff) as u64;
        if let Some(t) = find(&mut f, root, Some(16))? {
            if let Some(nm) = find(&mut f, sub(t), None)? {
                if let Some(lang) = find(&mut f, sub(nm), None)? {
                    let de: [u8; 16] = rd(&mut f, sub(lang))?;
                    if let Some(vo) = rva_to_off(&secs, u32le(&de, 0)) {
                        let size = (u32le(&de, 4) as usize).min(4096);
                        let mut buf = vec![0u8; size];
                        f.seek(SeekFrom::Start(vo))?;
                        f.read_exact(&mut buf)?;
                        if let Some(p) = buf.windows(4).position(|w| w == [0xbd, 0x04, 0xef, 0xfe]) {
                            if p + 16 <= buf.len() {
                                let ms = u32le(&buf, p + 8);
                                let ls = u32le(&buf, p + 12);
                                file_version = Some(format!("{}.{}.{}.{}", ms >> 16, ms & 0xffff, ls >> 16, ls & 0xffff));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(PeInfo { machine, timestamp, file_version, export_count, exports_sample: exports })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryFinding {
    pub file: String,
    pub size: u64,
    pub role: String,
    pub pe: Option<PeInfo>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveReport {
    pub install_dir: String,
    pub found: bool,
    pub product_version: Option<String>,
    pub binaries: Vec<BinaryFinding>,
    /// Third-party libraries recognised from file names, with versions where the name or resource gives one.
    pub bundled_libraries: Vec<String>,
    pub fusion_plugins: Vec<String>,
    pub lut_files: usize,
    pub lut_dirs: Vec<String>,
    pub ofx_bundles: Vec<String>,
    /// `resolve.EXPORT_*` timeline export types documented in the shipped scripting stub.
    pub documented_export_types: Vec<String>,
    pub dll_count: usize,
    pub notes: Vec<String>,
}

const KEY_BINARIES: [(&str, &str); 9] = [
    ("Resolve.exe", "main application"),
    ("fusionsystem.dll", "Fusion compositing core"),
    ("fusionscript.dll", "Fusion / Resolve scripting bridge"),
    ("FusionPage.dll", "Fusion page UI"),
    ("FairlightPage.dll", "Fairlight audio page"),
    ("OFXLoader.exe", "OpenFX plugin host process"),
    ("OpenColorIO_2_5.dll", "colour management (OpenColorIO)"),
    ("avcodec-60.dll", "bundled FFmpeg codecs"),
    ("Qt5Core.dll", "Qt 5 application framework"),
];

fn count_ext(dir: &Path, ext: &str, depth: usize) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() && depth > 0 {
            n += count_ext(&p, ext, depth - 1);
        } else if p.extension().map(|x| x.to_string_lossy().eq_ignore_ascii_case(ext)).unwrap_or(false) {
            n += 1;
        }
    }
    n
}

/// Default locations for this machine (install dir first).
pub fn default_install_dirs() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Ok(p) = std::env::var("PIROSMAN_RESOLVE_DIR") {
        v.push(PathBuf::from(p));
    }
    v.push(PathBuf::from(r"C:\Program Files\Blackmagic Design\DaVinci Resolve"));
    v.push(PathBuf::from("/Applications/DaVinci Resolve/DaVinci Resolve.app"));
    v.push(PathBuf::from("/opt/resolve"));
    v
}

pub fn support_dirs() -> Vec<PathBuf> {
    vec![PathBuf::from(r"C:\ProgramData\Blackmagic Design\DaVinci Resolve"), PathBuf::from("/Library/Application Support/Blackmagic Design/DaVinci Resolve")]
}

/// Inspect an installation directory (and the shared support directory).
pub fn inspect_install(install: &Path, support: &[PathBuf]) -> ResolveReport {
    let mut r = ResolveReport {
        install_dir: install.to_string_lossy().into(),
        found: install.is_dir(),
        product_version: None,
        binaries: vec![],
        bundled_libraries: vec![],
        fusion_plugins: vec![],
        lut_files: 0,
        lut_dirs: vec![],
        ofx_bundles: vec![],
        documented_export_types: vec![],
        dll_count: 0,
        notes: vec![
            "Read-only: PE headers, version resources and export-name tables only. Nothing was disassembled, decompiled, loaded, executed or copied.".into(),
        ],
    };
    if !r.found {
        r.notes.push("Installation directory not found.".into());
        return r;
    }
    for (name, role) in KEY_BINARIES {
        let p = install.join(name);
        let Ok(md) = std::fs::metadata(&p) else { continue };
        let (pe, error) = match inspect_pe(&p, 24) {
            Ok(i) => (Some(i), None),
            Err(e) => (None, Some(e.to_string())),
        };
        if name == "Resolve.exe" {
            r.product_version = pe.as_ref().and_then(|p| p.file_version.clone());
        }
        r.binaries.push(BinaryFinding { file: name.into(), size: md.len(), role: role.into(), pe, error });
    }
    if let Ok(rd) = std::fs::read_dir(install) {
        let names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        r.dll_count = names.iter().filter(|n| n.to_lowercase().ends_with(".dll")).count();
        let known: [(&str, &str); 12] = [
            ("Qt5Core.dll", "Qt 5"),
            ("avcodec-60.dll", "FFmpeg libavcodec 60 (FFmpeg 6.x)"),
            ("avformat-60.dll", "FFmpeg libavformat 60"),
            ("OpenColorIO_2_5.dll", "OpenColorIO 2.5"),
            ("OpenImageIO.dll", "OpenImageIO"),
            ("OpenEXR.dll", "OpenEXR"),
            ("openvdb.dll", "OpenVDB"),
            ("onnxruntime.dll", "ONNX Runtime"),
            ("torch_cpu.dll", "LibTorch"),
            ("lua5.1.dll", "Lua 5.1 (Fusion scripting)"),
            ("libxml2.dll", "libxml2"),
            ("opencv_core490.dll", "OpenCV 4.9"),
        ];
        for (file, label) in known {
            if names.iter().any(|n| n.eq_ignore_ascii_case(file)) {
                r.bundled_libraries.push(label.into());
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(install.join("Plugins")) {
        let mut v: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.ends_with(".plugin")).collect();
        v.sort();
        r.fusion_plugins = v;
    }
    let mut lut_roots = vec![install.join("CanonLUTs"), install.join("Fusion").join("LUTs")];
    for s in support {
        lut_roots.push(s.join("Support").join("LUT"));
        lut_roots.push(s.join("Fusion").join("LUTs"));
    }
    for d in lut_roots.into_iter().filter(|d| d.is_dir()) {
        r.lut_files += count_ext(&d, "cube", 3);
        r.lut_dirs.push(d.to_string_lossy().into());
    }
    let ofx = crate::diagnostics::discover_ofx(&crate::diagnostics::ofx_search_dirs());
    r.ofx_bundles = ofx.plugins.into_iter().map(|p| p.name).collect();
    for s in support {
        let stub = s.join("Support").join("Developer").join("Scripting").join("DaVinciResolveScript.pyi");
        if let Ok(text) = std::fs::read_to_string(&stub) {
            r.documented_export_types = documented_exports(&text);
            r.notes.push(format!("Export constants read from Blackmagic's documented scripting stub {}", stub.display()));
        }
    }
    r
}

/// `EXPORT_*` timeline export types declared as `TimelineExportType` in the stub.
pub fn documented_exports(pyi: &str) -> Vec<String> {
    let mut v: Vec<String> = pyi
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let (name, ty) = l.split_once(':')?;
            (ty.trim() == "TimelineExportType" && name.starts_with("EXPORT_")).then(|| name.trim().to_string())
        })
        .collect();
    v.dedup();
    v
}

/// Inspect the first installation found among the default locations.
pub fn inspect_default() -> ResolveReport {
    let dirs = default_install_dirs();
    let dir = dirs.iter().find(|d| d.is_dir()).cloned().unwrap_or_else(|| dirs[0].clone());
    inspect_install(&dir, &support_dirs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_export_constants() {
        let pyi = "class Resolve:\n\tEXPORT_AAF: TimelineExportType\n\tEXPORT_OTIO: TimelineExportType\n\tEXPORT_NONE: TimelineExportSubtype\n";
        assert_eq!(documented_exports(pyi), vec!["EXPORT_AAF", "EXPORT_OTIO"]);
    }

    #[test]
    fn rejects_non_pe() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("x.dll");
        std::fs::write(&f, vec![0u8; 128]).unwrap();
        assert!(inspect_pe(&f, 4).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn reads_a_real_system_dll() {
        let k = Path::new(r"C:\Windows\System32\kernel32.dll");
        if !k.is_file() {
            eprintln!("SKIPPED: no kernel32.dll");
            return;
        }
        let i = inspect_pe(k, 4000).unwrap();
        assert_eq!(i.machine, "x86-64");
        assert!(i.export_count > 500);
        assert!(i.exports_sample.iter().any(|e| e == "CreateFileW"));
        let v = i.file_version.unwrap();
        assert!(v.starts_with("10.") || v.starts_with("6."), "{v}");
    }

    #[test]
    fn missing_install_is_reported_not_fatal() {
        let r = inspect_install(Path::new("/definitely/not/here"), &[]);
        assert!(!r.found && r.binaries.is_empty());
    }
}

//! OpenFX effects: the plugin catalog and pre-rendering clips through plugins.
//!
//! Plugins never load into the editor. `pirosman-ofxhost` (a separate
//! executable shipped next to the app) describes each `.ofx` binary in its own
//! process, and renders frames streamed to it by FFmpeg, so a crashing plugin
//! fails one scan or one render instead of the editor.
//!
//! Rendering a clip with OpenFX effects:
//! 1. FFmpeg decodes the clip's source range (with transition handles) at the
//!    rate it will play (`project rate / speed`), scaled to fit the output, as
//!    `rgba64le` frames.
//! 2. The host runs the frames through the clip's effects in order.
//! 3. FFmpeg encodes the result to a cached intermediate (H.264 4:4:4, CRF 10).
//! 4. The render plan reads the intermediate instead of the media, and applies
//!    crop, grade, LUT, built-in effects, fades and transform as usual.
//!
//! Intermediates are keyed by everything that affects their pixels, so an
//! unchanged clip renders once. Plugin time is the frame index from 0.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};
use uuid::Uuid;

pub use pirosman_ofxhost::protocol::{EffectSpec, ParamInfo, PluginInfo, RenderSpec, ScanReport, PROTOCOL};

use crate::error::{CoreError, CoreResult};
use crate::media::{command, Tools};
use crate::model::{MediaKind, Project, TrackKind};
use crate::render::{handles, Prerendered, RenderSettings};
use crate::time::ffmpeg_time;

// ---------------------------------------------------------------- locations

static APP_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The folder that holds `ofx/pirosman-ofxhost` and `ofx/Plugins` (call once at start-up).
pub fn set_app_dir(dir: PathBuf) {
    let _ = APP_DIR.set(dir);
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf))
}

fn host_name() -> String {
    format!("pirosman-ofxhost{}", std::env::consts::EXE_SUFFIX)
}

/// The host executable: `PIROSMAN_OFXHOST`, `<app>/ofx/`, else next to the
/// running executable (development builds).
pub fn host_exe() -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = std::env::var_os("PIROSMAN_OFXHOST").map(PathBuf::from).into_iter().collect();
    if let Some(d) = APP_DIR.get() {
        c.push(d.join("ofx").join(host_name()));
    }
    if let Some(d) = exe_dir() {
        c.push(d.join("ofx").join(host_name()));
        c.push(d.join(host_name()));
    }
    c.into_iter().find(|p| p.is_file())
}

/// Plugin folders: the bundled samples, `OFX_PLUGIN_PATH`, and the system folder.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut v = vec![];
    for d in [APP_DIR.get().cloned(), exe_dir()].into_iter().flatten() {
        let p = d.join("ofx").join("Plugins");
        if p.is_dir() && !v.contains(&p) {
            v.push(p);
        }
    }
    for p in crate::diagnostics::ofx_search_dirs() {
        if !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

fn arch_dir() -> &'static str {
    if cfg!(windows) {
        "Win64"
    } else if cfg!(target_os = "macos") {
        "MacOS"
    } else {
        "Linux-x86-64"
    }
}

/// (bundle name, binary path) for every plugin binary for this platform.
pub fn binaries(dirs: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut out = vec![];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !e.path().is_dir() || !name.to_lowercase().ends_with(".ofx.bundle") {
                continue;
            }
            let bundle = name[..name.len() - ".ofx.bundle".len()].to_string();
            let Ok(arches) = std::fs::read_dir(e.path().join("Contents")) else { continue };
            for a in arches.flatten() {
                if !a.file_name().to_string_lossy().eq_ignore_ascii_case(arch_dir()) {
                    continue;
                }
                let Ok(files) = std::fs::read_dir(a.path()) else { continue };
                for f in files.flatten() {
                    if f.file_name().to_string_lossy().to_lowercase().ends_with(".ofx") {
                        out.push((bundle.clone(), f.path()));
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then(a.1.cmp(&b.1)));
    out
}

// ---------------------------------------------------------------- catalog

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogPlugin {
    #[serde(flatten)]
    pub info: PluginInfo,
    pub binary: String,
    pub bundle: String,
    /// Library tab, e.g. "Sapphire", "BCC" or "Pirosman Samples".
    pub vendor: String,
    /// Section inside the tab, e.g. "Blur+Sharpen".
    pub category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogFailure {
    pub bundle: String,
    pub binary: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Catalog {
    /// The host executable, when found.
    pub host: Option<String>,
    pub searched: Vec<String>,
    pub plugins: Vec<CatalogPlugin>,
    pub failures: Vec<CatalogFailure>,
}

impl Catalog {
    /// The installed plugin for `id`, preferring the binary it was added from.
    pub fn find(&self, id: &str, binary: &str) -> Option<&CatalogPlugin> {
        self.plugins.iter().find(|p| p.info.id == id && p.binary == binary).or_else(|| self.plugins.iter().find(|p| p.info.id == id))
    }
}

/// Split a plugin's grouping into (library tab, section).
pub fn vendor_category(grouping: &str, bundle: &str) -> (String, String) {
    let g = grouping.trim().trim_matches('/').trim();
    if let Some((v, rest)) = g.split_once('/') {
        let rest = rest.trim();
        return (v.trim().to_string(), if rest.is_empty() { "General".into() } else { rest.to_string() });
    }
    let first = g.split_whitespace().next().unwrap_or("");
    let known = ["bcc", "sapphire", "neat", "re:vision", "boris", "universe", "continuum", "mocha", "magic"];
    if !first.is_empty() && (bundle.to_lowercase().starts_with(&first.to_lowercase()) || known.contains(&first.to_lowercase().as_str())) {
        let rest = g[first.len()..].trim();
        return (first.to_string(), if rest.is_empty() { "General".into() } else { rest.to_string() });
    }
    if g.is_empty() {
        (bundle.to_string(), "General".into())
    } else {
        (bundle.to_string(), g.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    size: u64,
    modified: u64,
    report: ScanReport,
}

fn stamp(p: &Path) -> (u64, u64) {
    std::fs::metadata(p)
        .map(|m| (m.len(), m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)))
        .unwrap_or((0, 0))
}

/// How long one binary may take to describe its plugins.
const SCAN_TIMEOUT: Duration = Duration::from_secs(90);

fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" | ")
}

fn exit_text(code: Option<i32>) -> String {
    match code {
        Some(c) if c < 0 || c > 255 => format!("crashed (exit code 0x{:08X})", c as u32),
        Some(c) => format!("exit code {c}"),
        None => "was stopped".into(),
    }
}

/// Run `pirosman-ofxhost scan` on one binary in its own process.
pub fn scan_one(host: &Path, binary: &Path) -> Result<ScanReport, String> {
    let mut child = command(host)
        .args(["scan".as_ref(), binary.as_os_str()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start the OpenFX host: {e}"))?;
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let ot = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let et = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let started = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            break st;
        }
        if started.elapsed() > SCAN_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("describing its plugins took longer than {} s; it was stopped", SCAN_TIMEOUT.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = ot.join().unwrap_or_default();
    let stderr = et.join().unwrap_or_default();
    match serde_json::from_str::<ScanReport>(&stdout) {
        Ok(r) if r.protocol == PROTOCOL => Ok(r),
        Ok(r) => Err(format!("the OpenFX host speaks protocol {}, the editor {PROTOCOL}; reinstall Pirosman Studio", r.protocol)),
        Err(_) => Err(format!("the plugin {} while being described{}", exit_text(status.code()), if stderr.trim().is_empty() { String::new() } else { format!(": {}", tail(&stderr, 3)) })),
    }
}

/// Describe every installed plugin. Binaries unchanged since the last scan
/// (same size and modification time) reuse the cached description unless `force`.
pub fn scan_catalog(cache_file: &Path, force: bool) -> Catalog {
    let dirs = search_dirs();
    let mut cat = Catalog { host: None, searched: dirs.iter().map(|d| d.to_string_lossy().into()).collect(), plugins: vec![], failures: vec![] };
    let Some(host) = host_exe() else {
        cat.failures.push(CatalogFailure { bundle: String::new(), binary: String::new(), error: format!("the OpenFX host ({}) is missing from this installation", host_name()) });
        return cat;
    };
    cat.host = Some(host.to_string_lossy().into());
    let mut cache: HashMap<String, CacheEntry> = if force { HashMap::new() } else { std::fs::read_to_string(cache_file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default() };
    let mut fresh = HashMap::new();
    for (bundle, bin) in binaries(&dirs) {
        let key = bin.to_string_lossy().to_string();
        let (size, modified) = stamp(&bin);
        let report = match cache.remove(&key).filter(|c| c.size == size && c.modified == modified && c.report.protocol == PROTOCOL) {
            Some(c) => Ok(c.report),
            None => scan_one(&host, &bin),
        };
        match report {
            Err(e) => cat.failures.push(CatalogFailure { bundle, binary: key, error: e }),
            Ok(r) => {
                if let Some(e) = &r.error {
                    cat.failures.push(CatalogFailure { bundle: bundle.clone(), binary: key.clone(), error: e.clone() });
                }
                for info in &r.plugins {
                    let (vendor, category) = vendor_category(&info.grouping, &bundle);
                    cat.plugins.push(CatalogPlugin { info: info.clone(), binary: key.clone(), bundle: bundle.clone(), vendor, category });
                }
                fresh.insert(key, CacheEntry { size, modified, report: r });
            }
        }
    }
    if let Some(d) = cache_file.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(t) = serde_json::to_string(&fresh) {
        let _ = std::fs::write(cache_file, t);
    }
    cat.plugins.sort_by(|a, b| (a.vendor.to_lowercase(), a.category.to_lowercase(), a.info.label.to_lowercase()).cmp(&(b.vendor.to_lowercase(), b.category.to_lowercase(), b.info.label.to_lowercase())));
    cat
}

// ---------------------------------------------------------------- pre-render jobs

/// One clip to run through its OpenFX effects.
#[derive(Debug, Clone)]
pub struct Job {
    pub clip_id: Uuid,
    pub clip_name: String,
    /// FFmpeg input arguments (seek, loop, path).
    pub input: Vec<String>,
    pub width: u32,
    pub height: u32,
    pub frames: i64,
    /// Rate the intermediate plays at (`num/den`).
    pub rate: (i64, i64),
    pub effects: Vec<EffectSpec>,
    pub labels: Vec<String>,
    pub out: PathBuf,
    pub pad_front: i64,
}

impl Job {
    pub fn prerendered(&self) -> Prerendered {
        Prerendered { path: self.out.to_string_lossy().into(), pad_front: self.pad_front }
    }
}

fn even(v: f64) -> u32 {
    ((v.round() as u32).max(2) + 1) & !1
}

/// The clips in the render range that have enabled OpenFX effects, as jobs.
pub fn plan_jobs(p: &Project, s: &RenderSettings, catalog: &Catalog, cache_dir: &Path) -> CoreResult<Vec<Job>> {
    let rate = p.settings.frame_rate;
    let total = p.duration();
    let r0 = s.range_in.unwrap_or(0).max(0);
    let r1 = s.range_out.unwrap_or(total).min(total);
    let mut jobs = vec![];
    if s.video_codec == crate::render::VideoCodec::None {
        return Ok(jobs);
    }
    for t in p.tracks.iter().filter(|t| t.kind == TrackKind::Video && !t.muted) {
        for c in &t.clips {
            let fxs: Vec<_> = c.ofx.iter().filter(|f| f.enabled).collect();
            if fxs.is_empty() {
                continue;
            }
            let (lead, tail) = handles(t, c);
            let dur = c.duration() + lead + tail;
            let at = c.start - lead;
            if at >= r1 || at + dur <= r0 {
                continue;
            }
            let m = p.media(c.media_id).ok_or_else(|| CoreError::NotFound(format!("media for clip {:?}", c.name)))?;
            let mut effects = vec![];
            let mut labels = vec![];
            for f in &fxs {
                let entry = catalog.find(&f.plugin_id, &f.binary).ok_or_else(|| {
                    CoreError::NotFound(format!("clip {:?} uses the OpenFX effect {:?} ({}), which is not installed on this computer; install the plugin or remove the effect", c.name, f.label, f.plugin_id))
                })?;
                if !entry.info.supported {
                    return Err(CoreError::Invalid(format!("OpenFX effect {:?} on clip {:?} cannot run here: {}", f.label, c.name, entry.info.reason.clone().unwrap_or_default())));
                }
                effects.push(EffectSpec { binary: entry.binary.clone(), index: entry.info.index, id: f.plugin_id.clone(), params: f.params.clone() });
                labels.push(f.label.clone());
            }
            let (w, h) = match (m.width, m.height) {
                (Some(mw), Some(mh)) if mw > 0 && mh > 0 => {
                    let k = (s.width as f64 / mw as f64).min(s.height as f64 / mh as f64);
                    (even(mw as f64 * k).min(s.width), even(mh as f64 * k).min(s.height))
                }
                _ => (s.width, s.height),
            };
            // Only what the range needs, when timeline and source frames map one to one.
            let trim = c.speed == 1.0 && !c.reversed;
            let (pad_front, frames) = if trim {
                let t0 = at.max(r0);
                let t1 = (at + dur).min(r1);
                (t0 - at, t1 - t0)
            } else {
                (0, dur)
            };
            // Plays at project rate / speed so that `setpts=PTS/speed` lands on the project rate.
            let r = if c.speed == 1.0 { (rate.num as i64, rate.den as i64) } else { (rate.num as i64 * 100, ((rate.den as f64) * c.speed * 100.0).round() as i64) };
            let mut input = vec![];
            if m.kind == MediaKind::Image {
                input.extend(["-loop".into(), "1".into(), "-framerate".into(), format!("{}/{}", r.0, r.1)]);
            } else {
                input.extend(["-ss".into(), ffmpeg_time(c.source_in - lead + if trim { pad_front } else { 0 }, rate)]);
            }
            input.extend(["-i".into(), m.path.clone()]);
            let (size, modified) = stamp(Path::new(&m.path));
            let plugin_stamps: Vec<(u64, u64)> = effects.iter().map(|e| stamp(Path::new(&e.binary))).collect();
            let key = serde_json::json!({
                "v": 1, "protocol": PROTOCOL, "media": m.path, "size": size, "modified": modified, "input": input,
                "w": w, "h": h, "frames": frames, "rate": [r.0, r.1], "effects": effects, "plugins": plugin_stamps,
            });
            let digest = hex::encode(Sha256::digest(key.to_string().as_bytes()));
            jobs.push(Job {
                clip_id: c.id,
                clip_name: c.name.clone(),
                input,
                width: w,
                height: h,
                frames,
                rate: r,
                effects,
                labels,
                out: cache_dir.join(format!("{}.mkv", &digest[..32])),
                pad_front,
            });
        }
    }
    Ok(jobs)
}

fn encoder_args(tools: &Tools) -> Vec<&'static str> {
    static X264: OnceLock<bool> = OnceLock::new();
    let x264 = *X264.get_or_init(|| {
        command(&tools.ffmpeg)
            .args(["-hide_banner", "-encoders"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.split_whitespace().nth(1) == Some("libx264")))
            .unwrap_or(false)
    });
    if x264 {
        vec!["-c:v", "libx264", "-preset", "veryfast", "-crf", "10", "-pix_fmt", "yuv444p"]
    } else {
        vec!["-c:v", "ffv1", "-pix_fmt", "yuv444p"]
    }
}

fn read_all(mut r: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = r.read_to_string(&mut s);
        s
    })
}

/// Run one job: FFmpeg decode → OpenFX host → FFmpeg encode. Skips work when the
/// intermediate already exists. `on_frame` gets the count of frames done.
pub fn run_job(tools: &Tools, host: &Path, job: &Job, cancel: &AtomicBool, mut on_frame: impl FnMut(i64)) -> CoreResult<()> {
    if job.out.is_file() {
        on_frame(job.frames);
        return Ok(());
    }
    if job.frames <= 0 {
        return Err(CoreError::Invalid(format!("clip {:?} has no frames to render", job.clip_name)));
    }
    let dir = job.out.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let spec_path = job.out.with_extension("json");
    let spec = RenderSpec {
        protocol: PROTOCOL,
        width: job.width,
        height: job.height,
        frames: job.frames as u32,
        frame_rate: job.rate.0 as f64 / job.rate.1 as f64,
        effects: job.effects.clone(),
    };
    std::fs::write(&spec_path, serde_json::to_string(&spec).map_err(|e| CoreError::Media(e.to_string()))?)?;
    let partial = job.out.with_extension("partial.mkv");
    let rate = format!("{}/{}", job.rate.0, job.rate.1);
    let size = format!("{}x{}", job.width, job.height);
    let filter = format!("fps={rate},tpad=stop_mode=clone:stop=-1,scale={}:{}:flags=bicubic,setsar=1,format=rgba64le", job.width, job.height);

    let mut dec = command(&tools.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-v", "error"])
        .args(&job.input)
        .args(["-vf", &filter, "-frames:v", &job.frames.to_string(), "-f", "rawvideo", "-pix_fmt", "rgba64le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CoreError::Media(format!("could not start ffmpeg: {e}")))?;
    let mut hst = match command(host).arg("render").arg(&spec_path).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = dec.kill();
            return Err(CoreError::Media(format!("could not start the OpenFX host: {e}")));
        }
    };
    let mut enc = match command(&tools.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba64le", "-s", &size, "-r", &rate, "-i", "-"])
        .args(encoder_args(tools))
        .arg(&partial)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = dec.kill();
            let _ = hst.kill();
            return Err(CoreError::Media(format!("could not start ffmpeg: {e}")));
        }
    };
    let dec_err = read_all(dec.stderr.take().expect("piped"));
    let hst_err = read_all(hst.stderr.take().expect("piped"));
    let enc_err = read_all(enc.stderr.take().expect("piped"));
    let mut dec_out = dec.stdout.take().expect("piped");
    let mut hst_in = hst.stdin.take().expect("piped");
    let feeder = std::thread::spawn(move || {
        let _ = std::io::copy(&mut dec_out, &mut hst_in);
    });
    let mut hst_out = hst.stdout.take().expect("piped");
    let mut enc_in = enc.stdin.take().expect("piped");

    // Kill all three if the render is cancelled while a plugin is busy.
    let children: Arc<Mutex<Vec<Child>>> = Arc::new(Mutex::new(vec![dec, hst, enc]));
    let done = Arc::new(AtomicBool::new(false));
    let cancelled = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (children, done, cancelled) = (children.clone(), done.clone(), cancelled.clone());
        let cancel_ptr = cancel as *const AtomicBool as usize;
        std::thread::spawn(move || {
            // SAFETY: `cancel` outlives this thread; run_job joins it before returning.
            let cancel = unsafe { &*(cancel_ptr as *const AtomicBool) };
            while !done.load(Ordering::SeqCst) {
                if cancel.load(Ordering::SeqCst) {
                    cancelled.store(true, Ordering::SeqCst);
                    for c in children.lock().unwrap().iter_mut() {
                        let _ = c.kill();
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };

    let frame_bytes = job.width as usize * job.height as usize * 8;
    let mut buf = vec![0u8; frame_bytes];
    let mut written = 0i64;
    let mut pipe_error = None;
    while written < job.frames {
        if let Err(e) = hst_out.read_exact(&mut buf) {
            pipe_error = Some(e.to_string());
            break;
        }
        if let Err(e) = enc_in.write_all(&buf) {
            pipe_error = Some(e.to_string());
            break;
        }
        written += 1;
        on_frame(written);
    }
    drop(enc_in);
    drop(hst_out);
    let statuses: Vec<_> = {
        let mut cs = children.lock().unwrap();
        if pipe_error.is_some() {
            // Something upstream failed: stop the rest so wait() cannot hang.
            for c in cs.iter_mut() {
                let _ = c.kill();
            }
        }
        cs.iter_mut().map(|c| c.wait().ok()).collect()
    };
    done.store(true, Ordering::SeqCst);
    let _ = watcher.join();
    let _ = feeder.join();
    let (de, he, ee) = (dec_err.join().unwrap_or_default(), hst_err.join().unwrap_or_default(), enc_err.join().unwrap_or_default());
    let _ = std::fs::remove_file(&spec_path);
    let fail = |msg: String| {
        let _ = std::fs::remove_file(&partial);
        Err(CoreError::Media(msg))
    };
    if cancelled.load(Ordering::SeqCst) {
        return fail("render cancelled".into());
    }
    let host_ok = statuses.get(1).and_then(|s| s.as_ref()).is_some_and(|s| s.success());
    if !host_ok || written < job.frames {
        let host_msg: Vec<&str> = he.lines().filter(|l| l.starts_with("pirosman-ofxhost:")).map(|l| l.trim_start_matches("pirosman-ofxhost:").trim()).collect();
        let why = if let Some(m) = host_msg.last() {
            m.to_string()
        } else if !host_ok {
            format!("the OpenFX host {}{}", exit_text(statuses.get(1).and_then(|s| s.as_ref()).and_then(|s| s.code())), if he.trim().is_empty() { String::new() } else { format!(": {}", tail(&he, 3)) })
        } else if !de.trim().is_empty() {
            format!("decoding failed: {}", tail(&de, 3))
        } else {
            format!("stopped after {written} of {} frames ({})", job.frames, pipe_error.unwrap_or_default())
        };
        return fail(format!("OpenFX ({}) on clip {:?}: {why}", job.labels.join(" → "), job.clip_name));
    }
    let enc_ok = statuses.get(2).and_then(|s| s.as_ref()).is_some_and(|s| s.success());
    if !enc_ok {
        return fail(format!("encoding the OpenFX result for clip {:?} failed: {}", job.clip_name, tail(&ee, 3)));
    }
    std::fs::rename(&partial, &job.out)?;
    Ok(())
}

/// Run every job for a render, reporting (frames done, frames total, label).
pub fn prepare(tools: &Tools, jobs: &[Job], cancel: &AtomicBool, mut progress: impl FnMut(i64, i64, String)) -> CoreResult<HashMap<Uuid, Prerendered>> {
    let mut out = HashMap::new();
    if jobs.is_empty() {
        return Ok(out);
    }
    let host = host_exe().ok_or_else(|| CoreError::Media(format!("the OpenFX host ({}) is missing from this installation; reinstall Pirosman Studio", host_name())))?;
    let total: i64 = jobs.iter().map(|j| j.frames).sum();
    let mut before = 0i64;
    for (i, j) in jobs.iter().enumerate() {
        let label = format!("OpenFX: {} ({} of {})", j.clip_name, i + 1, jobs.len());
        progress(before, total, label.clone());
        run_job(tools, &host, j, cancel, |n| progress(before + n, total, label.clone()))?;
        before += j.frames;
        out.insert(j.clip_id, j.prerendered());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_tabs_from_groupings() {
        assert_eq!(vendor_category("Sapphire Blur+Sharpen", "Sapphire"), ("Sapphire".into(), "Blur+Sharpen".into()));
        assert_eq!(vendor_category("BCC Lights", "BCC_OFX"), ("BCC".into(), "Lights".into()));
        assert_eq!(vendor_category("Pirosman Samples/Color", "Pirosman Samples"), ("Pirosman Samples".into(), "Color".into()));
        assert_eq!(vendor_category("", "Grain"), ("Grain".into(), "General".into()));
        assert_eq!(vendor_category("Stylize", "Acme FX"), ("Acme FX".into(), "Stylize".into()));
    }

    /// End to end: FFmpeg → sample OpenFX plugins → render plan → verified file.
    /// Needs `cargo build -p pirosman-ofxhost -p pirosman-ofx-samples` and the
    /// bundled FFmpeg; skips (with a note) when they are absent.
    #[test]
    fn clip_renders_through_sample_plugins() {
        use crate::model::{fixtures, Clip, OfxEffect};
        use crate::render::{build_plan_with, execute, RenderSettings};
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let target = root.join("target").join("debug");
        let host = target.join(host_name());
        let dll = target.join(if cfg!(windows) { "pirosman_ofx_samples.dll" } else if cfg!(target_os = "macos") { "libpirosman_ofx_samples.dylib" } else { "libpirosman_ofx_samples.so" });
        let ff = root.join("src-tauri").join("ffmpeg");
        if !host.is_file() || !dll.is_file() || !ff.join(format!("ffmpeg{}", std::env::consts::EXE_SUFFIX)).is_file() {
            eprintln!("skipped: build pirosman-ofxhost and pirosman-ofx-samples and fetch FFmpeg first");
            return;
        }
        crate::media::set_bundled_dir(ff);
        let tools = crate::media::discover().expect("ffmpeg").tools;
        let tmp = tempfile::tempdir().unwrap();
        let plugins = tmp.path().join("Plugins");
        let bin = plugins.join("Pirosman Samples.ofx.bundle").join("Contents").join(arch_dir());
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::copy(&dll, bin.join("Pirosman Samples.ofx")).unwrap();
        std::env::set_var("OFX_PLUGIN_PATH", &plugins);
        std::env::set_var("PIROSMAN_OFXHOST", &host);

        let cat = scan_catalog(&tmp.path().join("scan.json"), true);
        assert!(cat.failures.is_empty(), "{:?}", cat.failures);
        let tint = cat.plugins.iter().find(|p| p.info.id == "studio.pirosman.samples.Tint").expect("Tint scanned").clone();
        assert_eq!((tint.vendor.as_str(), tint.category.as_str()), ("Pirosman Samples", "Color"));
        assert!(tint.info.supported);

        let src = tmp.path().join("src.mp4");
        let ok = command(&tools.ffmpeg)
            .args(["-v", "error", "-f", "lavfi", "-i", "color=c=white:s=160x120:r=25:d=1", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&src)
            .status()
            .unwrap();
        assert!(ok.success());
        let (mut p, _) = fixtures::project();
        p.media.clear();
        for t in &mut p.tracks {
            t.clips.clear();
            t.titles.clear();
        }
        p.settings.width = 160;
        p.settings.height = 120;
        let m = crate::media::import_media(&tools, &src, p.settings.frame_rate).unwrap();
        let mut c = Clip::new(m.id, "white", 0, 0, m.duration_frames.unwrap());
        let mut params = serde_json::Map::new();
        params.insert("color".into(), serde_json::json!([0.0, 0.0, 1.0]));
        params.insert("amount".into(), serde_json::json!(1.0));
        c.ofx.push(OfxEffect { id: Uuid::new_v4(), plugin_id: tint.info.id.clone(), label: "Tint".into(), grouping: tint.info.grouping.clone(), binary: tint.binary.clone(), index: tint.info.index, params, enabled: true });
        p.media.push(m);
        p.tracks[0].clips.push(c);
        p.validate().unwrap();

        let out = tmp.path().join("out.png");
        let mut s = RenderSettings::from_project(&p, &out.to_string_lossy());
        s.video_codec = crate::render::VideoCodec::Png;
        s.audio_codec = crate::render::AudioCodec::None;
        s.range_in = Some(10);
        s.range_out = Some(11);
        let jobs = plan_jobs(&p, &s, &cat, &tmp.path().join("cache")).unwrap();
        assert_eq!((jobs.len(), jobs[0].frames, jobs[0].pad_front), (1, 1, 10));
        let cancel = AtomicBool::new(false);
        let pre = prepare(&tools, &jobs, &cancel, |_, _, _| {}).unwrap();
        assert!(jobs[0].out.is_file());
        let plan = build_plan_with(&p, &s, true, &pre).unwrap();
        execute(&tools, &plan, true, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        // White multiplied by pure blue: the still must come out blue.
        let raw = command(&tools.ffmpeg).args(["-v", "error", "-i"]).arg(&out).args(["-vf", "crop=8:8:76:56,scale=1:1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]).output().unwrap().stdout;
        assert_eq!(raw.len(), 3);
        assert!(raw[0] < 40 && raw[1] < 40 && raw[2] > 200, "expected blue, got {raw:?}");
    }

    #[test]
    fn binaries_found_for_this_platform_only() {
        let d = tempfile::tempdir().unwrap();
        let b = d.path().join("Grain.ofx.bundle").join("Contents");
        std::fs::create_dir_all(b.join(arch_dir())).unwrap();
        std::fs::create_dir_all(b.join("Other")).unwrap();
        std::fs::write(b.join(arch_dir()).join("Grain.ofx"), b"x").unwrap();
        std::fs::write(b.join("Other").join("Grain.ofx"), b"x").unwrap();
        let found = binaries(&[d.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "Grain");
    }
}

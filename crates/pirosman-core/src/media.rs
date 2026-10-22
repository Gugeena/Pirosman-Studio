//! Media import: FFmpeg/FFprobe discovery, stream probing, content hashing,
//! relinking, thumbnails, and preview proxies.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};
use crate::model::{MediaKind, MediaRef, Project};
use crate::time::{seconds_to_frames, Rational};

#[cfg(windows)]
const EXE: &str = ".exe";
#[cfg(not(windows))]
const EXE: &str = "";

/// Resolved locations of the FFmpeg tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// Build a command that never flashes a console window on Windows.
pub fn command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c.stdin(Stdio::null());
    c
}

// ---------------------------------------------------------------- FFmpeg discovery
//
// Order: 1. PIROSMAN_FFMPEG_DIR (explicit advanced override), 2. the FFmpeg
// bundled with the app (`<resources>/ffmpeg`, set by the app at start-up and
// validated against its manifest), 3. a system FFmpeg (PATH, WinGet folder,
// common locations). Normal installs only ever use 2.

/// Where the FFmpeg tools came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSource {
    Override,
    Bundled,
    System,
}

/// Successful discovery: tools plus what was learned on the way.
#[derive(Debug, Clone)]
pub struct Discovery {
    pub tools: Tools,
    pub source: ToolSource,
    /// First line of `ffmpeg -version`.
    pub version: String,
    /// Set when the bundled copy was rejected and a system FFmpeg is used instead.
    pub warning: Option<String>,
}

static BUNDLED_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Tell discovery where the app's bundled FFmpeg lives (call once at start-up).
pub fn set_bundled_dir(dir: PathBuf) {
    let _ = BUNDLED_DIR.set(dir);
}

pub fn bundled_dir() -> Option<&'static Path> {
    BUNDLED_DIR.get().map(|p| p.as_path())
}

#[derive(Deserialize)]
struct BundleManifest {
    version: String,
    files: std::collections::BTreeMap<String, BundleFile>,
}
#[derive(Deserialize)]
struct BundleFile {
    size: u64,
}

const REINSTALL: &str = "Reinstall Pirosman Studio (or re-extract the portable zip) to restore it.";

/// Run `<tool> -version` and return its first line, or a readable reason.
fn run_version(tool: &Path) -> Result<String, String> {
    let out = command(tool).arg("-version").output().map_err(|e| format!("{} could not be started: {e}", tool.display()))?;
    if !out.status.success() {
        return Err(format!("{} exited with {} when asked for its version", tool.display(), out.status));
    }
    let line = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or_default().to_string();
    if !line.contains(" version ") {
        return Err(format!("{} did not identify itself as FFmpeg (got \"{line}\")", tool.display()));
    }
    Ok(line)
}

/// Validate the bundled FFmpeg folder against the manifest written by
/// scripts/fetch-ffmpeg.ps1: both executables present, the recorded size
/// (catches truncated or partially extracted files), and they run and report
/// the pinned version.
pub fn validate_bundled(dir: &Path) -> Result<(Tools, String), String> {
    let mpath = dir.join("manifest.json");
    let raw = fs::read_to_string(&mpath).map_err(|_| format!("The bundled FFmpeg is missing: {} was not found. {REINSTALL}", mpath.display()))?;
    let m: BundleManifest = serde_json::from_str(raw.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("The bundled FFmpeg manifest {} is unreadable ({e}). {REINSTALL}", mpath.display()))?;
    let mut paths = vec![];
    for name in [format!("ffmpeg{EXE}"), format!("ffprobe{EXE}")] {
        let p = dir.join(&name);
        let md = fs::metadata(&p).map_err(|_| format!("The bundled {name} is missing from {}. {REINSTALL}", dir.display()))?;
        if let Some(f) = m.files.get(&name) {
            if md.len() != f.size {
                return Err(format!("The bundled {name} is damaged: {} bytes instead of {}. {REINSTALL}", md.len(), f.size));
            }
        }
        let line = run_version(&p).map_err(|e| format!("The bundled {name} failed its self-check: {e}. {REINSTALL}"))?;
        if !line.contains(&m.version) {
            return Err(format!("The bundled {name} is the wrong build (\"{line}\", expected {}). {REINSTALL}", m.version));
        }
        paths.push((p, line));
    }
    let (ffprobe, _) = paths.pop().unwrap();
    let (ffmpeg, version) = paths.pop().unwrap();
    Ok((Tools { ffmpeg, ffprobe }, version))
}

/// Validate an arbitrary folder holding ffmpeg and ffprobe (override or system).
fn validate_dir(dir: &Path) -> Result<(Tools, String), String> {
    let ffmpeg = dir.join(format!("ffmpeg{EXE}"));
    let ffprobe = dir.join(format!("ffprobe{EXE}"));
    if !ffmpeg.is_file() || !ffprobe.is_file() {
        return Err(format!("{} does not contain both ffmpeg{EXE} and ffprobe{EXE}", dir.display()));
    }
    let version = run_version(&ffmpeg)?;
    run_version(&ffprobe)?;
    Ok((Tools { ffmpeg, ffprobe }, version))
}

/// Find FFmpeg using the order above. Errors are specific and actionable.
pub fn discover() -> CoreResult<Discovery> {
    discover_with(std::env::var_os("PIROSMAN_FFMPEG_DIR").map(PathBuf::from), bundled_dir(), system_dirs())
}

/// Testable core of [`discover`].
pub fn discover_with(override_dir: Option<PathBuf>, bundled: Option<&Path>, system: Vec<PathBuf>) -> CoreResult<Discovery> {
    if let Some(d) = override_dir {
        return validate_dir(&d)
            .map(|(tools, version)| Discovery { tools, source: ToolSource::Override, version, warning: None })
            .map_err(|e| CoreError::Media(format!("PIROSMAN_FFMPEG_DIR is set but unusable: {e}. Fix or remove the variable to use the bundled FFmpeg.")));
    }
    let mut bundled_err = None;
    if let Some(d) = bundled {
        match validate_bundled(d) {
            Ok((tools, version)) => return Ok(Discovery { tools, source: ToolSource::Bundled, version, warning: None }),
            Err(e) => bundled_err = Some(e),
        }
    }
    let find = |name: &str| system.iter().map(|d| d.join(format!("{name}{EXE}"))).find(|p| p.is_file());
    if let (Some(ffmpeg), Some(ffprobe)) = (find("ffmpeg"), find("ffprobe")) {
        if let (Ok(version), Ok(_)) = (run_version(&ffmpeg), run_version(&ffprobe)) {
            return Ok(Discovery { tools: Tools { ffmpeg, ffprobe }, source: ToolSource::System, version, warning: bundled_err });
        }
    }
    Err(CoreError::Media(bundled_err.unwrap_or_else(|| {
        "FFmpeg was not found. Installed and portable builds of Pirosman Studio include it in their ffmpeg folder; \
         for a source build run `npm run fetch-ffmpeg`, or set PIROSMAN_FFMPEG_DIR to a folder containing ffmpeg and ffprobe."
            .to_string()
    })))
}

/// A system FFmpeg, if any (fallback only).
fn system_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![];
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    // Windows: winget installs Gyan.FFmpeg under the user's WinGet packages dir,
    // and a freshly installed PATH entry is not visible to already-running apps.
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let pkgs = Path::new(&local).join("Microsoft").join("WinGet").join("Packages");
        if let Ok(rd) = fs::read_dir(&pkgs) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().to_lowercase().contains("ffmpeg") {
                    if let Ok(inner) = fs::read_dir(e.path()) {
                        for i in inner.flatten() {
                            dirs.push(i.path().join("bin"));
                        }
                    }
                }
            }
        }
    }
    for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "C:\\ffmpeg\\bin"] {
        dirs.push(PathBuf::from(d));
    }
    dirs
}

/// Find ffmpeg and ffprobe (see [`discover`]).
pub fn discover_tools() -> CoreResult<Tools> {
    discover().map(|d| d.tools)
}

pub fn tool_version(tool: &Path) -> Option<String> {
    let out = command(tool).arg("-version").output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().next().map(|s| s.to_string())
}

/// Streaming SHA-256 of the complete file.
pub fn sha256_file(path: &Path) -> CoreResult<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

fn file_stat(path: &Path) -> CoreResult<(u64, Option<i64>)> {
    let md = fs::metadata(path)?;
    let modified = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    Ok((md.len(), modified))
}

/// Run ffprobe and return its JSON output.
pub fn ffprobe_json(tools: &Tools, path: &Path) -> CoreResult<Value> {
    let out = command(&tools.ffprobe)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .output()
        .map_err(|e| CoreError::Media(format!("could not run ffprobe: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(CoreError::Media(format!(
            "ffprobe could not read {}: {}",
            path.display(),
            err.lines().last().unwrap_or("unknown error").trim()
        )));
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

const IMAGE_CODECS: &[&str] = &["png", "mjpeg", "jpegls", "bmp", "tiff", "webp", "gif"];

/// Fill a `MediaRef` from FFprobe JSON. Pure, so it is unit-tested with fixtures.
pub fn media_from_probe(path: &Path, probe: &Value, project_rate: Rational) -> CoreResult<MediaRef> {
    let streams = probe.get("streams").and_then(Value::as_array).cloned().unwrap_or_default();
    let format = probe.get("format").cloned().unwrap_or(Value::Null);
    let video = streams.iter().find(|s| {
        s.get("codec_type").and_then(Value::as_str) == Some("video")
            && s.get("disposition").and_then(|d| d.get("attached_pic")).and_then(Value::as_i64) != Some(1)
    });
    let audio = streams.iter().find(|s| s.get("codec_type").and_then(Value::as_str) == Some("audio"));
    if video.is_none() && audio.is_none() {
        return Err(CoreError::Media(format!("{} has no video or audio streams", path.display())));
    }
    let vcodec = video.and_then(|v| v.get("codec_name")).and_then(Value::as_str).map(str::to_string);
    let fmt_name = format.get("format_name").and_then(Value::as_str).unwrap_or("");
    let is_image = video.is_some()
        && audio.is_none()
        && (fmt_name.contains("image2") || fmt_name.ends_with("_pipe") || vcodec.as_deref().map(|c| IMAGE_CODECS.contains(&c)).unwrap_or(false))
        && format.get("duration").and_then(Value::as_str).and_then(|d| d.parse::<f64>().ok()).map(|d| d < 0.1).unwrap_or(true);
    let kind = if is_image {
        MediaKind::Image
    } else if video.is_some() {
        MediaKind::Video
    } else {
        MediaKind::Audio
    };
    let duration_s = format
        .get("duration")
        .and_then(Value::as_str)
        .and_then(|d| d.parse::<f64>().ok())
        .or_else(|| video.or(audio).and_then(|s| s.get("duration")).and_then(Value::as_str).and_then(|d| d.parse().ok()));
    let duration_frames = if kind == MediaKind::Image {
        None
    } else {
        let d = duration_s.ok_or_else(|| CoreError::Media(format!("{} reports no duration", path.display())))?;
        let f = seconds_to_frames(d, project_rate);
        if f <= 0 {
            return Err(CoreError::Media(format!("{} is shorter than one frame", path.display())));
        }
        Some(f)
    };
    let rate = video.and_then(|v| {
        v.get("avg_frame_rate")
            .and_then(Value::as_str)
            .and_then(Rational::parse)
            .or_else(|| v.get("r_frame_rate").and_then(Value::as_str).and_then(Rational::parse))
    });
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "media".into());
    Ok(MediaRef {
        id: Uuid::new_v4(),
        name,
        path: path.to_string_lossy().to_string(),
        sha256: None,
        size: format.get("size").and_then(Value::as_str).and_then(|s| s.parse().ok()).unwrap_or(0),
        modified: None,
        kind,
        duration_frames,
        has_video: video.is_some(),
        has_audio: audio.is_some(),
        width: video.and_then(|v| v.get("width")).and_then(Value::as_u64).map(|w| w as u32),
        height: video.and_then(|v| v.get("height")).and_then(Value::as_u64).map(|h| h as u32),
        frame_rate: if kind == MediaKind::Video { rate } else { None },
        video_codec: vcodec,
        audio_codec: audio.and_then(|a| a.get("codec_name")).and_then(Value::as_str).map(str::to_string),
        sample_rate: audio.and_then(|a| a.get("sample_rate")).and_then(Value::as_str).and_then(|s| s.parse().ok()),
        channels: audio.and_then(|a| a.get("channels")).and_then(Value::as_u64).map(|c| c as u32),
        container: Some(fmt_name.to_string()).filter(|s| !s.is_empty()),
        extra: Map::new(),
    })
}

/// Probe and hash a file, producing a new `MediaRef`.
pub fn import_media(tools: &Tools, path: &Path, project_rate: Rational) -> CoreResult<MediaRef> {
    if !path.is_file() {
        return Err(CoreError::NotFound(format!("{} is not a file", path.display())));
    }
    let probe = ffprobe_json(tools, path)?;
    let mut m = media_from_probe(path, &probe, project_rate)?;
    let (size, modified) = file_stat(path)?;
    m.size = size;
    m.modified = modified;
    m.sha256 = Some(sha256_file(path)?);
    Ok(m)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaStatus {
    Online,
    Missing,
    /// File exists but size or mtime changed since it was hashed.
    Modified,
}

pub fn media_status(m: &MediaRef) -> MediaStatus {
    let p = Path::new(&m.path);
    match file_stat(p) {
        Err(_) => MediaStatus::Missing,
        Ok((size, modified)) => {
            if size != m.size || (m.modified.is_some() && modified != m.modified) {
                MediaStatus::Modified
            } else {
                MediaStatus::Online
            }
        }
    }
}

/// Point a media reference at a new file. The new file must hash identically
/// to the recorded content hash unless `allow_different` is set, so unrelated
/// media is never silently substituted.
pub fn relink(project: &mut Project, media_id: Uuid, new_path: &Path, tools: &Tools, allow_different: bool) -> CoreResult<()> {
    let rate = project.settings.frame_rate;
    let m = project.media.iter_mut().find(|m| m.id == media_id).ok_or_else(|| CoreError::NotFound(format!("media {media_id}")))?;
    let hash = sha256_file(new_path)?;
    if let Some(expected) = &m.sha256 {
        if *expected != hash && !allow_different {
            return Err(CoreError::Conflict(format!(
                "{} has different content (SHA-256 {}…) than the original {} ({}…)",
                new_path.display(),
                &hash[..12],
                m.name,
                &expected[..12.min(expected.len())]
            )));
        }
    }
    let probe = ffprobe_json(tools, new_path)?;
    let fresh = media_from_probe(new_path, &probe, rate)?;
    let (size, modified) = file_stat(new_path)?;
    let keep_extra = std::mem::take(&mut m.extra);
    *m = MediaRef { id: media_id, sha256: Some(hash), size, modified, extra: keep_extra, ..fresh };
    project.validate()
}

/// Find a file with the given SHA-256 in `dir` (non-recursive): used to
/// auto-relink media that arrived inside a bundle.
pub fn find_by_hash(dir: &Path, sha: &str) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.is_file() && sha256_file(p).ok().as_deref() == Some(sha))
}

/// Can the system WebView (Chromium/WebView2/WebKit) play this file directly?
pub fn browser_playable(m: &MediaRef) -> bool {
    let ext = Path::new(&m.path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match m.kind {
        MediaKind::Image => matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp"),
        MediaKind::Audio => matches!(ext.as_str(), "mp3" | "wav" | "m4a" | "aac" | "ogg" | "opus" | "flac"),
        MediaKind::Video => {
            let v_ok = matches!(m.video_codec.as_deref(), Some("h264" | "vp8" | "vp9" | "av1"));
            let a_ok = matches!(m.audio_codec.as_deref(), None | Some("aac" | "mp3" | "opus" | "vorbis"));
            v_ok && a_ok && matches!(ext.as_str(), "mp4" | "m4v" | "webm" | "mov")
        }
    }
}

/// Generate an H.264/AAC MP4 preview proxy (720p max). Blocking.
pub fn make_proxy(tools: &Tools, src: &Path, out: &Path) -> CoreResult<()> {
    if let Some(d) = out.parent() {
        fs::create_dir_all(d)?;
    }
    let tmp = out.with_extension("partial.mp4");
    let st = command(&tools.ffmpeg)
        .args(["-v", "error", "-y", "-i"])
        .arg(src)
        .args([
            "-map", "0:v:0?", "-map", "0:a:0?", "-vf", "scale=-2:'min(720,ih)'", "-c:v", "libx264", "-preset", "veryfast",
            "-crf", "23", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart",
        ])
        .arg(&tmp)
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| CoreError::Media(format!("could not run ffmpeg: {e}")))?;
    if !st.status.success() {
        let _ = fs::remove_file(&tmp);
        return Err(CoreError::Media(format!("proxy failed: {}", String::from_utf8_lossy(&st.stderr).trim())));
    }
    fs::rename(&tmp, out)?;
    Ok(())
}

/// Extract a small JPEG thumbnail from `at_seconds` into the media.
pub fn make_thumbnail(tools: &Tools, src: &Path, at_seconds: f64, out: &Path) -> CoreResult<()> {
    if let Some(d) = out.parent() {
        fs::create_dir_all(d)?;
    }
    let st = command(&tools.ffmpeg)
        .args(["-v", "error", "-y", "-ss", &format!("{at_seconds:.3}"), "-i"])
        .arg(src)
        .args(["-frames:v", "1", "-vf", "scale=320:-2", "-q:v", "4"])
        .arg(out)
        .output()
        .map_err(|e| CoreError::Media(format!("could not run ffmpeg: {e}")))?;
    if !st.status.success() || !out.exists() {
        return Err(CoreError::Media("thumbnail extraction failed".into()));
    }
    Ok(())
}

/// Frames sampled for a media's timeline filmstrip: two per second, 6 to 100.
pub fn filmstrip_frames(duration_seconds: f64) -> u32 {
    (duration_seconds * 2.0).round().clamp(6.0, 100.0) as u32
}

/// Height of one filmstrip frame in pixels (crisp at the timeline's clip height on HiDPI).
pub const FILMSTRIP_HEIGHT: u32 = 72;

/// Render `frames` evenly spaced frames of a video side by side into one JPEG
/// (frame `i` shows the picture at `i / frames` of the duration). The timeline
/// picks the frame for each position along a clip from this strip.
pub fn make_filmstrip(tools: &Tools, src: &Path, duration_seconds: f64, frames: u32, out: &Path) -> CoreResult<()> {
    if duration_seconds <= 0.0 || frames == 0 {
        return Err(CoreError::Invalid("a filmstrip needs a duration and at least one frame".into()));
    }
    if let Some(d) = out.parent() {
        fs::create_dir_all(d)?;
    }
    let tmp = out.with_extension("partial.jpg");
    let vf = format!("fps={frames}/{duration_seconds:.6},scale=-2:{FILMSTRIP_HEIGHT}:flags=bicubic,setsar=1,tile={frames}x1");
    let st = command(&tools.ffmpeg)
        .args(["-v", "error", "-y", "-an", "-sn", "-i"])
        .arg(src)
        .args(["-vf", &vf, "-frames:v", "1", "-q:v", "5"])
        .arg(&tmp)
        .output()
        .map_err(|e| CoreError::Media(format!("could not run ffmpeg: {e}")))?;
    if !st.status.success() || !tmp.exists() {
        let _ = fs::remove_file(&tmp);
        return Err(CoreError::Media(format!("filmstrip rendering failed: {}", String::from_utf8_lossy(&st.stderr).lines().last().unwrap_or(""))));
    }
    fs::rename(&tmp, out)?;
    Ok(())
}

/// Render an audio waveform strip as PNG.
pub fn make_waveform(tools: &Tools, src: &Path, out: &Path) -> CoreResult<()> {
    if let Some(d) = out.parent() {
        fs::create_dir_all(d)?;
    }
    let st = command(&tools.ffmpeg)
        .args(["-v", "error", "-y", "-i"])
        .arg(src)
        .args(["-filter_complex", "[0:a:0]aformat=channel_layouts=mono,showwavespic=s=1600x120:colors=0x9fb8d6", "-frames:v", "1"])
        .arg(out)
        .output()
        .map_err(|e| CoreError::Media(format!("could not run ffmpeg: {e}")))?;
    if !st.status.success() || !out.exists() {
        return Err(CoreError::Media("waveform rendering failed".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn probe_av() -> Value {
        json!({
            "streams": [
                {"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"30000/1001","r_frame_rate":"30000/1001","duration":"10.010000"},
                {"codec_type":"audio","codec_name":"aac","sample_rate":"48000","channels":2}
            ],
            "format": {"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"10.010000","size":"123456"}
        })
    }

    #[test]
    fn probe_parses_av_file() {
        let m = media_from_probe(Path::new("/x/clip.mp4"), &probe_av(), Rational::new(25, 1)).unwrap();
        assert_eq!(m.kind, MediaKind::Video);
        assert_eq!(m.duration_frames, Some(250)); // 10.01 s @ 25 fps → 250.25 → 250
        assert_eq!(m.frame_rate, Some(Rational::new(30000, 1001)));
        assert_eq!((m.width, m.height), (Some(1920), Some(1080)));
        assert!(m.has_audio && m.has_video);
        assert_eq!(m.sample_rate, Some(48000));
        assert!(browser_playable(&m));
    }

    #[test]
    fn probe_detects_images_and_audio() {
        let img = json!({"streams":[{"codec_type":"video","codec_name":"png","width":800,"height":600,"avg_frame_rate":"0/0"}],"format":{"format_name":"png_pipe"}});
        let m = media_from_probe(Path::new("logo.png"), &img, Rational::new(25, 1)).unwrap();
        assert_eq!(m.kind, MediaKind::Image);
        assert_eq!(m.duration_frames, None);
        let aud = json!({"streams":[{"codec_type":"audio","codec_name":"pcm_s16le","sample_rate":"44100","channels":1}],"format":{"format_name":"wav","duration":"2.0"}});
        let m = media_from_probe(Path::new("vo.wav"), &aud, Rational::new(25, 1)).unwrap();
        assert_eq!((m.kind, m.duration_frames, m.has_video), (MediaKind::Audio, Some(50), false));
        let none = json!({"streams":[{"codec_type":"subtitle"}],"format":{}});
        assert!(media_from_probe(Path::new("x.srt"), &none, Rational::new(25, 1)).is_err());
    }

    #[test]
    fn prores_needs_proxy() {
        let mut m = media_from_probe(Path::new("/x/clip.mov"), &probe_av(), Rational::new(25, 1)).unwrap();
        m.video_codec = Some("prores".into());
        assert!(!browser_playable(&m));
    }

    /// The FFmpeg fetched by scripts/fetch-ffmpeg.ps1, if present in this checkout.
    fn real_bundle() -> Option<PathBuf> {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/ffmpeg");
        d.join("manifest.json").is_file().then_some(d)
    }

    /// A fake bundle folder: manifest + given executables (bytes).
    fn fake_bundle(files: &[(&str, &[u8])], sizes: &[(&str, u64)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (n, b) in files {
            fs::write(dir.path().join(n), b).unwrap();
        }
        let files_json: Map<String, Value> = sizes.iter().map(|(n, s)| (n.to_string(), json!({ "size": s, "sha256": "x" }))).collect();
        fs::write(dir.path().join("manifest.json"), json!({ "version": "9.0.2-test", "files": files_json }).to_string()).unwrap();
        dir
    }

    #[test]
    fn bundled_missing_or_damaged_ffmpeg_gives_specific_errors() {
        let ff = format!("ffmpeg{EXE}");
        let fp = format!("ffprobe{EXE}");
        // No manifest at all.
        let empty = tempfile::tempdir().unwrap();
        let e = validate_bundled(empty.path()).unwrap_err();
        assert!(e.contains("missing") && e.contains("Reinstall"), "{e}");
        // Manifest but no executables.
        let d = fake_bundle(&[], &[]);
        let e = validate_bundled(d.path()).unwrap_err();
        assert!(e.contains(&format!("bundled {ff} is missing")), "{e}");
        // Truncated executable (size differs from the manifest).
        let d = fake_bundle(&[(&ff, b"MZ"), (&fp, b"MZ")], &[(&ff, 1000), (&fp, 2)]);
        let e = validate_bundled(d.path()).unwrap_err();
        assert!(e.contains("damaged") && e.contains("2 bytes instead of 1000"), "{e}");
        // Right size but not a working program.
        let junk = b"this is not an executable";
        let d = fake_bundle(&[(&ff, junk), (&fp, junk)], &[(&ff, junk.len() as u64), (&fp, junk.len() as u64)]);
        let e = validate_bundled(d.path()).unwrap_err();
        assert!(e.contains("failed its self-check"), "{e}");
        // Discovery surfaces the bundled error rather than a generic one, and never mentions winget.
        let err = discover_with(None, Some(d.path()), vec![]).unwrap_err().to_string();
        assert!(err.contains("failed its self-check") && !err.to_lowercase().contains("winget"), "{err}");
    }

    #[test]
    fn nothing_found_message_is_actionable_without_winget() {
        let err = discover_with(None, None, vec![]).unwrap_err().to_string();
        assert!(err.contains("ffmpeg folder") && err.contains("PIROSMAN_FFMPEG_DIR") && !err.to_lowercase().contains("winget"), "{err}");
        let bad = tempfile::tempdir().unwrap();
        let err = discover_with(Some(bad.path().to_path_buf()), None, vec![]).unwrap_err().to_string();
        assert!(err.contains("PIROSMAN_FFMPEG_DIR is set but unusable"), "{err}");
    }

    #[test]
    fn bundled_ffmpeg_is_found_without_path_or_winget() {
        let Some(dir) = real_bundle() else {
            eprintln!("SKIPPED: run `npm run fetch-ffmpeg` to test the bundled FFmpeg");
            return;
        };
        // No override and no system directories: only the bundle can satisfy this.
        let d = discover_with(None, Some(&dir), vec![]).unwrap();
        assert_eq!(d.source, ToolSource::Bundled);
        assert!(d.version.contains("9.0.2"), "{}", d.version);
        assert!(d.tools.ffmpeg.starts_with(&dir) && d.tools.ffprobe.starts_with(&dir));
        assert!(d.warning.is_none());
        // Bundled copy broken but a system FFmpeg exists: fall back, with the reason attached.
        let broken = tempfile::tempdir().unwrap();
        let d = discover_with(None, Some(broken.path()), vec![dir.clone()]).unwrap();
        assert_eq!(d.source, ToolSource::System);
        assert!(d.warning.unwrap().contains("missing"));
        // An explicit override wins over the bundle.
        let d = discover_with(Some(dir.clone()), Some(broken.path()), vec![]).unwrap();
        assert_eq!(d.source, ToolSource::Override);
    }

    #[test]
    fn hash_is_full_content_sha256() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.bin");
        fs::write(&f, b"abc").unwrap();
        assert_eq!(sha256_file(&f).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(find_by_hash(dir.path(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"), Some(f.clone()));
    }

    #[test]
    fn status_detects_missing_and_modified() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.bin");
        fs::write(&f, b"abc").unwrap();
        let (size, modified) = file_stat(&f).unwrap();
        let mut m = media_from_probe(&f, &probe_av(), Rational::new(25, 1)).unwrap();
        m.size = size;
        m.modified = modified;
        assert_eq!(media_status(&m), MediaStatus::Online);
        m.size = 99;
        assert_eq!(media_status(&m), MediaStatus::Modified);
        m.path = dir.path().join("gone.bin").to_string_lossy().to_string();
        assert_eq!(media_status(&m), MediaStatus::Missing);
    }
}

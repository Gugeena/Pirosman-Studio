//! Loopback HTTP server for local media on Linux. WebKitGTK's GStreamer player
//! refuses custom URI schemes such as `asset://`, so the WebView loads files from
//! `http://127.0.0.1:<port>/<token>/<percent-encoded path>` instead (Range requests
//! supported, for seeking). Only files the asset-protocol scope allows are served,
//! and the random per-launch token keeps other local programs and web pages out.
//! Idle or slow connections time out, headers are size-limited and the number of
//! connections served at once is capped, so no client can pin threads or memory.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// The server's base URL (Linux), its start-up error, or None where it is not used.
pub struct MediaBase(pub Option<Result<String, String>>);

/// Where the WebView should load local files from: Ok(None) means `asset://`.
#[tauri::command]
pub fn media_base_url(base: tauri::State<MediaBase>) -> Result<Option<String>, String> {
    base.0.clone().transpose()
}

struct Limits {
    max_connections: usize,
    read_timeout: Duration,
    write_timeout: Duration,
}

const LIMITS: Limits = Limits { max_connections: 32, read_timeout: Duration::from_secs(10), write_timeout: Duration::from_secs(30) };
/// Request line plus headers; WebKit's requests are well under 2 KiB.
const MAX_HEADER_BYTES: u64 = 16 * 1024;

pub fn start(app: AppHandle) -> std::io::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let base = format!("http://127.0.0.1:{}/{token}", listener.local_addr()?.port());
    run(listener, token, LIMITS, move |p: &str| app.asset_protocol_scope().is_allowed(p));
    Ok(base)
}

/// Accept connections on `listener` in the background and serve each on its own thread.
fn run(listener: TcpListener, token: String, limits: Limits, allowed: impl Fn(&str) -> bool + Send + Sync + 'static) {
    let allowed = Arc::new(allowed);
    let active = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            if active.fetch_add(1, Ordering::SeqCst) >= limits.max_connections {
                active.fetch_sub(1, Ordering::SeqCst);
                let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                continue;
            }
            let _ = stream.set_read_timeout(Some(limits.read_timeout));
            let _ = stream.set_write_timeout(Some(limits.write_timeout));
            let (allowed, token, slot) = (allowed.clone(), token.clone(), Slot(active.clone()));
            std::thread::spawn(move || {
                let _slot = slot;
                let _ = serve(stream, &token, &*allowed);
            });
        }
    });
}

/// One counted connection; released when the serving thread ends, even by panic.
struct Slot(Arc<AtomicUsize>);
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn reply(stream: &mut TcpStream, status: &str) -> std::io::Result<()> {
    write!(stream, "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
}

fn serve(mut stream: TcpStream, token: &str, allowed: &dyn Fn(&str) -> bool) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?.take(MAX_HEADER_BYTES));
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let (mut range, mut complete) = (None, false);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break; // the client closed, or the header limit was reached
        }
        if line.trim().is_empty() {
            complete = true;
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("range") {
                range = parse_range(v.trim());
            }
        }
    }
    if !complete {
        return reply(&mut stream, "431 Request Header Fields Too Large");
    }
    let mut parts = request.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let path = target.strip_prefix('/').and_then(|t| t.strip_prefix(token)).and_then(|t| t.strip_prefix('/')).map(crate::percent_decode);
    let path = match path {
        Some(p) if (method == "GET" || method == "HEAD") && allowed(&p) && Path::new(&p).is_file() => p,
        _ => return reply(&mut stream, "404 Not Found"),
    };
    let mut file = File::open(&path)?;
    let len = file.metadata()?.len();
    let (status, start, end) = match range {
        Some((a, b)) if len == 0 || a >= len || b.is_some_and(|b| b < a) => {
            return write!(stream, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
        Some((a, b)) => ("206 Partial Content", a, b.map_or(len - 1, |b| b.min(len - 1))),
        None => ("200 OK", 0, len.saturating_sub(1)),
    };
    let count = if len == 0 { 0 } else { end - start + 1 };
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {}\r\nContent-Length: {count}\r\nAccept-Ranges: bytes\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-cache\r\nConnection: close\r\n",
        mime(Path::new(&path)).unwrap_or("application/octet-stream")
    );
    if status.starts_with("206") {
        head.push_str(&format!("Content-Range: bytes {start}-{end}/{len}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    if method == "GET" && count > 0 {
        file.seek(SeekFrom::Start(start))?;
        std::io::copy(&mut file.take(count), &mut stream)?;
    }
    Ok(())
}

/// `bytes=a-b` or `bytes=a-` (suffix and multi-range requests are answered in full).
fn parse_range(v: &str) -> Option<(u64, Option<u64>)> {
    let (a, b) = v.strip_prefix("bytes=")?.split_once('-')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()))
}

/// Content type by extension, for this server and the `pcache` scheme (lib.rs).
pub(crate) fn mime(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "m4a" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "flac" => "audio/flac",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99"), Some((0, Some(99))));
        assert_eq!(parse_range("bytes=100-"), Some((100, None)));
        assert_eq!(parse_range("bytes=-500"), None);
        assert_eq!(parse_range("items=0-1"), None);
    }

    /// A temp folder removed when dropped.
    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A server over a temp folder: `ok.mp4` is allowed, `secret.mp4` exists but is not.
    fn server(limits: Limits) -> (std::net::SocketAddr, TempDir, String) {
        let dir = TempDir(std::env::temp_dir().join(format!("pirosman-media-test-{}", uuid::Uuid::new_v4().simple())));
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(dir.path().join("ok.mp4"), b"0123456789").unwrap();
        std::fs::write(dir.path().join("secret.mp4"), b"secret").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let ok = dir.path().join("ok.mp4").to_string_lossy().into_owned();
        let allowed_path = ok.clone();
        run(listener, "tok".into(), limits, move |p: &str| p == allowed_path);
        (addr, dir, ok)
    }

    fn get(addr: std::net::SocketAddr, raw: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    fn enc(p: &str) -> String {
        p.bytes().map(|b| if b.is_ascii_alphanumeric() || b".-_".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
    }

    #[test]
    fn serves_only_allowed_files_with_the_token_and_honours_ranges() {
        let (addr, dir, ok) = server(LIMITS);
        let req = |extra: &str, path: &str, token: &str| get(addr, &format!("GET /{token}/{} HTTP/1.1\r\nHost: x\r\n{extra}\r\n", enc(path)));
        let full = req("", &ok, "tok");
        assert!(full.starts_with("HTTP/1.1 200 OK") && full.contains("Content-Type: video/mp4") && full.ends_with("\r\n\r\n0123456789"), "{full}");
        let part = req("Range: bytes=2-5\r\n", &ok, "tok");
        assert!(part.starts_with("HTTP/1.1 206") && part.contains("Content-Range: bytes 2-5/10") && part.ends_with("\r\n\r\n2345"), "{part}");
        let tail = req("Range: bytes=7-\r\n", &ok, "tok");
        assert!(tail.contains("Content-Range: bytes 7-9/10") && tail.ends_with("789"), "{tail}");
        for bad in ["Range: bytes=5-2\r\n", "Range: bytes=10-\r\n"] {
            let r = req(bad, &ok, "tok");
            assert!(r.starts_with("HTTP/1.1 416") && r.contains("Content-Range: bytes */10"), "{bad}: {r}");
        }
        assert!(req("", &ok, "nope").starts_with("HTTP/1.1 404"));
        let secret = dir.path().join("secret.mp4").to_string_lossy().into_owned();
        assert!(req("", &secret, "tok").starts_with("HTTP/1.1 404"));
        let sneaky = format!("{}/../secret.mp4", ok);
        assert!(req("", &sneaky, "tok").starts_with("HTTP/1.1 404"));
        let head = get(addr, &format!("HEAD /tok/{} HTTP/1.1\r\n\r\n", enc(&ok)));
        assert!(head.starts_with("HTTP/1.1 200") && head.contains("Content-Length: 10") && head.ends_with("\r\n\r\n"), "{head}");
        let huge = get(addr, &format!("GET /tok/{} HTTP/1.1\r\nX-Junk: {}\r\n\r\n", enc(&ok), "a".repeat(20_000)));
        assert!(huge.starts_with("HTTP/1.1 431"), "{huge}");
    }

    #[test]
    fn idle_connections_time_out_and_the_connection_count_is_capped() {
        let (addr, _dir, ok) = server(Limits { max_connections: 1, read_timeout: Duration::from_millis(300), write_timeout: Duration::from_secs(5) });
        // An idle client holds the only slot: a second client is turned away at once.
        let mut idle = TcpStream::connect(addr).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(get(addr, &format!("GET /tok/{} HTTP/1.1\r\n\r\n", enc(&ok))).starts_with("HTTP/1.1 503"));
        // The idle client is dropped by the read timeout, which frees the slot.
        idle.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = Vec::new();
        let _ = idle.read_to_end(&mut buf);
        std::thread::sleep(Duration::from_millis(50));
        assert!(get(addr, &format!("GET /tok/{} HTTP/1.1\r\n\r\n", enc(&ok))).starts_with("HTTP/1.1 200"));
    }
}

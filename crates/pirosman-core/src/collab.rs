//! Live Sync hub: a small WebSocket relay that runs inside the hosting app.
//!
//! Every peer (including the host's own window) connects as a client. The
//! first message must be `{"type":"hello","code":…,"name":…,"color":…}` with
//! the session code; anything else closes the connection. After that the hub
//! relays each JSON message to every *other* peer, stamped with the sender's
//! id, and announces joins and leaves. Project state travels as whole-project
//! snapshots after each transactional edit (last writer wins); presence
//! (cursor, page, playhead, selection) travels as small frequent messages.
//! Revision mode never starts this hub.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tungstenite::{accept, Message};

type Peers = Arc<Mutex<HashMap<u64, (Sender<String>, Value)>>>;

pub struct Hub {
    pub port: u16,
    pub code: String,
    stop: Arc<AtomicBool>,
}

impl Hub {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        // Unblock accept().
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn session_code() -> String {
    let n = uuid::Uuid::new_v4().as_u128();
    const A: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    (0..6).map(|i| A[((n >> (i * 5)) & 31) as usize] as char).collect()
}

/// Start the hub on `port` (0 = pick a free one), listening on all interfaces
/// so collaborators on the LAN can join with the session code.
pub fn start(port: u16) -> std::io::Result<Hub> {
    let listener = TcpListener::bind(("0.0.0.0", port))?;
    let port = listener.local_addr()?.port();
    let code = session_code();
    let stop = Arc::new(AtomicBool::new(false));
    let peers: Peers = Arc::new(Mutex::new(HashMap::new()));
    let next = Arc::new(AtomicU64::new(1));
    let (code2, stop2) = (code.clone(), stop.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if stop2.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = stream else { continue };
            let (peers, code, stop, id) = (peers.clone(), code2.clone(), stop2.clone(), next.fetch_add(1, Ordering::SeqCst));
            std::thread::spawn(move || serve(stream, id, peers, code, stop));
        }
    });
    Ok(Hub { port, code, stop })
}

fn broadcast(peers: &Peers, from: u64, text: &str) {
    for (pid, (tx, _)) in peers.lock().unwrap().iter() {
        if *pid != from {
            let _ = tx.send(text.to_string());
        }
    }
}

fn serve(stream: TcpStream, id: u64, peers: Peers, code: String, stop: Arc<AtomicBool>) {
    let _ = stream.set_nodelay(true);
    let Ok(mut ws) = accept(stream) else { return };
    // Handshake: hello with the right code within 5 s.
    let _ = ws.get_ref().set_read_timeout(Some(Duration::from_secs(5)));
    let hello: Value = match ws.read() {
        Ok(Message::Text(t)) => serde_json::from_str(&t).unwrap_or(Value::Null),
        _ => return,
    };
    if hello["type"] != "hello" || hello["code"].as_str() != Some(code.as_str()) {
        let _ = ws.send(Message::Text(json!({ "type": "error", "message": "wrong session code" }).to_string().into()));
        let _ = ws.close(None);
        return;
    }
    let info = json!({ "id": id, "name": hello["name"].as_str().unwrap_or("Guest").chars().take(40).collect::<String>(), "color": hello["color"].as_str().unwrap_or("#5f9be6") });
    let (tx, rx): (Sender<String>, Receiver<String>) = channel();
    let others: Vec<Value> = peers.lock().unwrap().values().map(|(_, v)| v.clone()).collect();
    peers.lock().unwrap().insert(id, (tx, info.clone()));
    let _ = ws.send(Message::Text(json!({ "type": "welcome", "id": id, "peers": others }).to_string().into()));
    broadcast(&peers, id, &json!({ "type": "join", "peer": info }).to_string());
    let _ = ws.get_ref().set_read_timeout(Some(Duration::from_millis(15)));
    loop {
        if stop.load(Ordering::SeqCst) {
            let _ = ws.close(None);
            break;
        }
        match ws.read() {
            Ok(Message::Text(t)) => {
                if let Ok(mut v) = serde_json::from_str::<Value>(&t) {
                    v["from"] = json!(id);
                    broadcast(&peers, id, &v.to_string());
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        while let Ok(out) = rx.try_recv() {
            if ws.send(Message::Text(out.into())).is_err() {
                break;
            }
        }
    }
    peers.lock().unwrap().remove(&id);
    broadcast(&peers, id, &json!({ "type": "leave", "id": id }).to_string());
}

/// Non-loopback IPv4 addresses collaborators on the LAN can use.
pub fn lan_addresses() -> Vec<String> {
    // Connecting a UDP socket picks the outbound interface without sending anything.
    let mut v = vec![];
    if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if s.connect("192.0.2.1:9").is_ok() {
            if let Ok(a) = s.local_addr() {
                if !a.ip().is_loopback() {
                    v.push(a.ip().to_string());
                }
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use tungstenite::connect;

    fn hello(ws: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>, code: &str, name: &str) {
        ws.send(Message::Text(json!({ "type": "hello", "code": code, "name": name, "color": "#f4c430" }).to_string().into())).unwrap();
    }
    fn next_json(ws: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>) -> Value {
        loop {
            if let Message::Text(t) = ws.read().unwrap() {
                return serde_json::from_str(&t).unwrap();
            }
        }
    }

    #[test]
    fn relays_between_peers_and_rejects_wrong_code() {
        let hub = start(0).unwrap();
        let url = format!("ws://127.0.0.1:{}", hub.port);
        let (mut a, _) = connect(&url).unwrap();
        hello(&mut a, &hub.code, "Lasha");
        assert_eq!(next_json(&mut a)["type"], "welcome");
        let (mut b, _) = connect(&url).unwrap();
        hello(&mut b, &hub.code, "Nino");
        let w = next_json(&mut b);
        assert_eq!(w["peers"][0]["name"], "Lasha", "late joiner learns who is here");
        assert_eq!(next_json(&mut a)["type"], "join");
        a.send(Message::Text(json!({ "type": "presence", "playhead": 42 }).to_string().into())).unwrap();
        let p = next_json(&mut b);
        assert_eq!((p["type"].as_str(), p["playhead"].as_i64()), (Some("presence"), Some(42)));
        assert_eq!(p["from"], w["peers"][0]["id"]);
        let (mut c, _) = connect(&url).unwrap();
        hello(&mut c, "WRONG1", "Mallory");
        assert_eq!(next_json(&mut c)["type"], "error");
        drop(b);
        assert_eq!(next_json(&mut a)["type"], "leave");
        hub.stop();
    }
}

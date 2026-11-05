//! Test support for the voice chain: a stub moby-api on this computer (consents, speech to text,
//! text to speech, agent turn), a robot link that only records, and recorders. Nothing here talks
//! to a real server, a real speech provider or a sound device.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use moby_protocol::Expression;

use super::{RobotControl, Stage, ViewSink, VoiceView};

pub const STT_OK: &str =
    r#"{"text":"git holatini ayt","language":"uz","script":"latn","confidence":0.93}"#;
pub const ANSWER: &str = r#"{"event":"completed","speech_text":"Hammasi joyida.","display_text":"Hammasi joyida.","expression_hint":"happy"}"#;

#[derive(Clone)]
pub enum Reply {
    Json(u16, String),
    Wav(Vec<u8>),
    Sse(Vec<String>),
    /// Never answers. The stub notes when the client hangs up.
    Hang,
}

pub struct Config {
    pub stt: Reply,
    pub tts: Reply,
    pub agent: Vec<Reply>,
    pub consents_post: Reply,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
    /// The client closed the connection while the stub was holding the reply back.
    pub dropped: bool,
}

pub struct Stub {
    pub url: String,
    pub config: Arc<Mutex<Config>>,
    requests: Arc<Mutex<Vec<Request>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn tone_wav() -> Vec<u8> {
    let pcm: Vec<u8> = (0..1600_i16)
        .flat_map(|n| ((n % 50) * 100).to_le_bytes())
        .collect();
    super::wav::encode(&pcm).expect("a valid clip")
}

impl Stub {
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let config = Arc::new(Mutex::new(Config {
            stt: Reply::Json(200, STT_OK.to_owned()),
            tts: Reply::Wav(tone_wav()),
            agent: vec![Reply::Sse(vec![ANSWER.to_owned()])],
            consents_post: Reply::Json(204, String::new()),
        }));
        let requests = Arc::new(Mutex::new(Vec::<Request>::new()));
        let (cfg, seen) = (Arc::clone(&config), Arc::clone(&requests));
        std::thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(stream) = connection else { continue };
                let (cfg, seen) = (Arc::clone(&cfg), Arc::clone(&seen));
                std::thread::spawn(move || serve(stream, &cfg, &seen));
            }
        });
        Self {
            url,
            config,
            requests,
        }
    }

    pub fn set(&self, change: impl FnOnce(&mut Config)) {
        change(&mut lock(&self.config));
    }

    pub fn requests(&self) -> Vec<Request> {
        lock(&self.requests).clone()
    }

    pub fn to(&self, path: &str) -> Vec<Request> {
        self.requests()
            .into_iter()
            .filter(|r| r.path.starts_with(path))
            .collect()
    }

    pub async fn wait_for(&self, path: &str, count: usize) {
        let until = Instant::now() + Duration::from_secs(10);
        while self.to(path).len() < count {
            assert!(Instant::now() < until, "no request to {path}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Waits until the client has hung up on the request number `index` to `path`.
    pub async fn wait_dropped(&self, path: &str, index: usize) {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if self.to(path).get(index).is_some_and(|r| r.dropped) {
                return;
            }
            assert!(Instant::now() < until, "{path} was never dropped");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

fn serve(mut stream: std::net::TcpStream, cfg: &Mutex<Config>, seen: &Mutex<Vec<Request>>) {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 8192];
    let (head_end, length) = loop {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            return;
        }
        raw.extend_from_slice(&chunk[..n]);
        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..pos]).to_lowercase();
            let length = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            break (pos + 4, length);
        }
    };
    while raw.len() < head_end + length {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let mut parts = head.split_whitespace();
    let (method, path) = (
        parts.next().unwrap_or("").to_owned(),
        parts.next().unwrap_or("").to_owned(),
    );
    let index = {
        let mut list = lock(seen);
        list.push(Request {
            method: method.clone(),
            path: path.clone(),
            body: raw[head_end..].to_vec(),
            dropped: false,
        });
        list.len() - 1
    };
    let agent_index = lock(seen)
        .iter()
        .filter(|r| r.path.starts_with("/v1/agent"))
        .count()
        .saturating_sub(1);
    let reply = {
        let cfg = lock(cfg);
        let base = path.split('?').next().unwrap_or("");
        match (method.as_str(), base) {
            ("POST", "/v1/voice/stt") => cfg.stt.clone(),
            ("POST", "/v1/voice/tts") => cfg.tts.clone(),
            ("POST", "/v1/consents") => cfg.consents_post.clone(),
            ("GET", "/v1/consents") => Reply::Json(
                200,
                r#"{"current_versions":{"terms":"dev-1","privacy_policy":"dev-1","cloud_voice":"dev-1"},"records":[]}"#
                    .to_owned(),
            ),
            ("POST", "/v1/agent/turn") => cfg
                .agent
                .get(agent_index)
                .or(cfg.agent.last())
                .cloned()
                .unwrap_or(Reply::Json(500, "{}".to_owned())),
            _ => Reply::Json(404, r#"{"code":"not_found","message":"x","request_id":"r"}"#.to_owned()),
        }
    };
    let write = |stream: &mut std::net::TcpStream, status: u16, ctype: &str, body: &[u8]| {
        let head = format!(
            "HTTP/1.1 {status} X\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(body);
    };
    match reply {
        Reply::Json(status, body) => {
            write(&mut stream, status, "application/json", body.as_bytes());
        }
        Reply::Wav(bytes) => write(&mut stream, 200, "audio/wav", &bytes),
        Reply::Sse(frames) => {
            let mut body = String::new();
            for frame in frames {
                body.push_str(&format!("data: {frame}\n\n"));
            }
            write(&mut stream, 200, "text/event-stream", body.as_bytes());
        }
        Reply::Hang => {
            let _ = stream.set_nonblocking(true);
            let until = Instant::now() + Duration::from_secs(20);
            let mut one = [0_u8; 1];
            while Instant::now() < until {
                match stream.peek(&mut one) {
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    _ => {
                        lock(seen)[index].dropped = true;
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// A robot link that only writes down what the chain asked of it.
#[derive(Default)]
pub struct FakeRobot {
    pub cancels: Mutex<u32>,
    pub faces: Mutex<Vec<(Expression, u32)>>,
}

impl FakeRobot {
    pub fn faces(&self) -> Vec<Expression> {
        lock(&self.faces).iter().map(|(e, _)| *e).collect()
    }

    pub fn cancels(&self) -> u32 {
        *lock(&self.cancels)
    }
}

impl RobotControl for FakeRobot {
    fn cancel_capture(&self) {
        *lock(&self.cancels) += 1;
    }

    fn face(&self, expression: Expression, hold_ms: u32) {
        lock(&self.faces).push((expression, hold_ms));
    }
}

/// Writes down every view the interface would get.
#[derive(Default)]
pub struct Views(pub Mutex<Vec<VoiceView>>);

impl Views {
    pub fn stages(&self) -> Vec<Stage> {
        let mut out: Vec<Stage> = Vec::new();
        for view in lock(&self.0).iter() {
            if out.last() != Some(&view.stage) {
                out.push(view.stage);
            }
        }
        out
    }
}

impl ViewSink for Arc<Views> {
    fn emit(&self, view: &VoiceView) -> bool {
        lock(&self.0).push(view.clone());
        true
    }
}

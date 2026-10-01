//! Shared test helpers: the anonymised fixtures (fake players "Player1…",
//! real boss names) and a tiny HTTP stub standing in for mythics.gg. No test
//! talks to a real server.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub fn fixture(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let raw = std::fs::read(p).unwrap();
    // Git may check the fixtures out with either line ending; the game writes
    // CRLF, so the tests use CRLF.
    to_crlf(&raw)
}

pub fn to_crlf(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + raw.len() / 50);
    for (i, &b) in raw.iter().enumerate() {
        if b == b'\n' && (i == 0 || raw[i - 1] != b'\r') {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

/// Every line of `bytes`, with its line ending, and the offset it starts at.
pub fn lines(bytes: &[u8]) -> Vec<(u64, &[u8])> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            out.push((start as u64, &bytes[start..=i]));
            start = i + 1;
        }
    }
    if start < bytes.len() {
        out.push((start as u64, &bytes[start..]));
    }
    out
}

pub fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

pub type Handler = dyn Fn(&Request) -> (u16, String) + Send + Sync;
/// The same, replying with bytes (a zip).
type BytesHandler = dyn Fn(&Request) -> (u16, Vec<u8>) + Send + Sync;

/// A one-thread HTTP/1.1 stub on 127.0.0.1. Each response closes the
/// connection, so the client opens a new one per request.
pub struct Stub {
    pub origin: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
    handler: Arc<Mutex<Box<BytesHandler>>>,
}

impl Stub {
    pub fn start(handler: impl Fn(&Request) -> (u16, String) + Send + Sync + 'static) -> Self {
        Self::start_bytes(move |r| {
            let (status, body) = handler(r);
            (status, body.into_bytes())
        })
    }

    pub fn start_bytes(
        handler: impl Fn(&Request) -> (u16, Vec<u8>) + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Mutex<Box<BytesHandler>>> = Arc::new(Mutex::new(Box::new(handler)));
        let (reqs, h) = (requests.clone(), handler.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                serve(stream, &reqs, &h);
            }
        });
        Self {
            origin,
            requests,
            handler,
        }
    }

    pub fn set_handler(&self, handler: impl Fn(&Request) -> (u16, String) + Send + Sync + 'static) {
        *self.handler.lock().unwrap() = Box::new(move |r| {
            let (status, body) = handler(r);
            (status, body.into_bytes())
        });
    }

    pub fn taken(&self) -> Vec<Request> {
        std::mem::take(&mut *self.requests.lock().unwrap())
    }
}

fn serve(stream: TcpStream, reqs: &Mutex<Vec<Request>>, handler: &Mutex<Box<BytesHandler>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut first = String::new();
    if reader.read_line(&mut first).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut len = 0usize;
    let mut authorization = None;
    let mut content_type = None;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
            break;
        }
        let (k, v) = h.split_once(':').unwrap_or((&h, ""));
        let v = v.trim().to_string();
        match k.to_ascii_lowercase().as_str() {
            "content-length" => len = v.parse().unwrap_or(0),
            "authorization" => authorization = Some(v),
            "content-type" => content_type = Some(v),
            _ => {}
        }
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body).unwrap();
    let req = Request {
        method,
        path,
        authorization,
        content_type,
        body,
    };
    let (status, reply) = (handler.lock().unwrap())(&req);
    reqs.lock().unwrap().push(req);
    let mut s = stream;
    let _ = write!(
        s,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.len()
    );
    let _ = s.write_all(&reply);
    let _ = s.flush();
}

//! Logging in: PKCE with a loopback redirect (RFC 8252).
//!
//! The app listens on `127.0.0.1` on a free port, opens the browser at the
//! site's sign-in with a code challenge and a random state, and waits for the
//! browser to come back to `/callback?code=…&state=…`. It then swaps the code
//! and its verifier for an app token. The app never sees a Battle.net token
//! or the site's client secret.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

fn random_b64(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

impl Pkce {
    pub fn new() -> Self {
        let verifier = random_b64(32);
        Self {
            challenge: challenge_for(&verifier),
            verifier,
            state: random_b64(16),
        }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

/// S256: base64url(SHA-256(verifier)), unpadded.
pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("the sign-in took too long")]
    TimedOut,
    #[error("the sign-in came back from somewhere else")]
    StateMismatch,
    #[error("the sign-in was cancelled")]
    Denied,
    #[error("couldn't listen for the sign-in")]
    Listen,
}

pub struct Loopback {
    listener: TcpListener,
}

impl Loopback {
    /// Binds `127.0.0.1` on a port the system picks.
    pub fn bind() -> Result<Self, AuthError> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|_| AuthError::Listen)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| AuthError::Listen)?;
        Ok(Self { listener })
    }

    pub fn port(&self) -> u16 {
        self.listener.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// Waits for the browser's redirect and returns the code. Requests for
    /// anything but `/callback` (a browser's favicon request) get a 404 and
    /// the wait goes on. `cancelled` is checked about ten times a second.
    pub fn wait_for_code(
        &self,
        state: &str,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<String, AuthError> {
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() > deadline {
                return Err(AuthError::TimedOut);
            }
            if cancelled() {
                return Err(AuthError::Denied);
            }
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Some(result) = handle(stream, state) {
                        return result;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => return Err(AuthError::Listen),
            }
        }
    }
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>mythics.gg</title>\
<body style=\"font-family:system-ui,sans-serif;background:#0b0d11;color:#e8ebf1;display:grid;place-items:center;min-height:90vh\">\
<main style=\"text-align:center\"><h1 style=\"font-size:1.4rem\">You're logged in</h1>\
<p>You can close this tab and go back to the mythics.gg app.</p></main>";

const FAILED_PAGE: &str = "<!doctype html><meta charset=utf-8><title>mythics.gg</title>\
<body style=\"font-family:system-ui,sans-serif;background:#0b0d11;color:#e8ebf1;display:grid;place-items:center;min-height:90vh\">\
<main style=\"text-align:center\"><h1 style=\"font-size:1.4rem\">Logging in didn't work</h1>\
<p>Go back to the mythics.gg app and select Log in with Battle.net again.</p></main>";

fn handle(mut stream: TcpStream, state: &str) -> Option<Result<String, AuthError>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = [0u8; 8192];
    let mut n = 0;
    while n < buf.len() {
        match stream.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => {
                n += k;
                if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let req = String::from_utf8_lossy(&buf[..n]);
    let target = req
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("GET "))
        .and_then(|l| l.split(' ').next())
        .unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/callback" {
        respond(&mut stream, "404 Not Found", "");
        return None;
    }
    let param = |k: &str| {
        query
            .split('&')
            .filter_map(|p| p.split_once('='))
            .find(|(key, _)| *key == k)
            .map(|(_, v)| percent_decode(v))
    };
    let result = match (param("code"), param("state")) {
        (_, Some(s)) if s != state => Err(AuthError::StateMismatch),
        (Some(code), Some(_)) if !code.is_empty() => Ok(code),
        _ => Err(AuthError::Denied),
    };
    let page = if result.is_ok() {
        DONE_PAGE
    } else {
        FAILED_PAGE
    };
    respond(&mut stream, "200 OK", page);
    Some(result)
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_matches_rfc7636_example() {
        // RFC 7636, appendix B.
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    fn get(port: u16, path: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    #[test]
    fn returns_the_code_for_the_right_state() {
        let lb = Loopback::bind().unwrap();
        let port = lb.port();
        let t = std::thread::spawn(move || {
            let r404 = get(port, "/favicon.ico");
            let ok = get(port, "/callback?code=abc%2D123&state=s1");
            (r404, ok)
        });
        let code = lb
            .wait_for_code("s1", Duration::from_secs(10), &|| false)
            .unwrap();
        let (r404, ok) = t.join().unwrap();
        assert_eq!(code, "abc-123");
        assert!(r404.starts_with("HTTP/1.1 404"));
        assert!(ok.contains("You're logged in"));
    }

    #[test]
    fn refuses_a_wrong_state() {
        let lb = Loopback::bind().unwrap();
        let port = lb.port();
        let t = std::thread::spawn(move || get(port, "/callback?code=abc&state=other"));
        let r = lb.wait_for_code("s1", Duration::from_secs(10), &|| false);
        assert_eq!(r, Err(AuthError::StateMismatch));
        assert!(t.join().unwrap().contains("didn't work"));
    }

    #[test]
    fn times_out() {
        let lb = Loopback::bind().unwrap();
        let r = lb.wait_for_code("s1", Duration::from_millis(200), &|| false);
        assert_eq!(r, Err(AuthError::TimedOut));
    }
}

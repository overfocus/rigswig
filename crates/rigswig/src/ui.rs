//! `rigswig ui`: the same harness as a local, read-only page.
//!
//! One repo, chosen at start. Every request re-reads the setup and returns one HTML page with the harness JSON inside;
//! there is no API and nothing the page can change. The server:
//! - listens on 127.0.0.1 only, on a random port unless one is given;
//! - answers only requests that carry the random token (`?t=`) and a `Host` of exactly `127.0.0.1:<port>` or
//!   `localhost:<port>` (which stops DNS rebinding), and refuses any cross-site `Origin` or `Sec-Fetch-Site`;
//! - accepts `GET` only, with a capped request size and a read timeout;
//! - sends a CSP that allows only its own nonce'd script and style and no outside request of any kind;
//! - stops after `--idle` minutes without a request.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use rigswig_core::model::Harness;

const PAGE: &str = include_str!("../assets/ui.html");
const MAX_REQUEST: usize = 8 * 1024;

/// 128 random bits as hex: the OS's random source on Unix, else std's randomly keyed hasher.
pub fn random_hex() -> String {
    let mut bytes = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).is_ok();
    if !ok {
        use std::hash::{BuildHasher, Hasher};
        for chunk in bytes.chunks_mut(8) {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()),
            );
            chunk.copy_from_slice(&h.finish().to_le_bytes());
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compare without stopping at the first differing byte.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub struct Request {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn param(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() => {
                let hex = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
                out.push(hex(b[i + 1]) * 16 + hex(b[i + 2]));
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse a request head (request line and headers). `None` if it is not a well-formed HTTP/1.x head.
pub fn parse(head: &str) -> Option<Request> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?;
    if !first.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let (path, qs) = target.split_once('?').unwrap_or((target, ""));
    let query = qs
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (unescape(k), unescape(v))
        })
        .collect();
    let headers = lines
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect();
    Some(Request { method, path: path.to_string(), query, headers })
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

fn plain(status: u16, text: &str) -> Response {
    Response {
        status,
        headers: vec![("Content-Type", "text/plain; charset=utf-8".into()), ("Cache-Control", "no-store".into())],
        body: format!("{text}\n"),
    }
}

/// The harness JSON, safe inside a `<script type="application/json">`: `<`, `>`, `&` and the two line separators
/// are escaped so nothing in a file name or description can close the element.
pub fn embed_json(v: &impl serde::Serialize) -> String {
    let s = serde_json::to_string(v).expect("the model serialises");
    s.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// What the page needs besides the harness: the other tools to switch to.
#[derive(serde::Serialize)]
pub struct PageData<'a> {
    pub harness: &'a Harness,
    pub tools: Vec<ToolChoice>,
    pub version: &'static str,
}

#[derive(serde::Serialize)]
pub struct ToolChoice {
    pub id: String,
    pub score: u8,
    pub evidence: Vec<String>,
}

/// Answer one request. `render(tool)` resolves the harness for a tool id (or the default when `None`) and returns
/// the page data as embedded JSON, or `None` for an unknown tool.
pub fn handle(req: &Request, token: &str, port: u16, render: &dyn Fn(Option<&str>) -> Option<String>) -> Response {
    let host_ok =
        req.header("Host").is_some_and(|h| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}"));
    if !host_ok {
        return plain(421, "wrong host");
    }
    if let Some(origin) = req.header("Origin") {
        if origin != format!("http://127.0.0.1:{port}") && origin != format!("http://localhost:{port}") {
            return plain(403, "cross-origin request refused");
        }
    }
    if req.header("Sec-Fetch-Site").is_some_and(|s| s != "same-origin" && s != "none") {
        return plain(403, "cross-site request refused");
    }
    if req.method != "GET" {
        return plain(405, "read-only: GET only");
    }
    if req.path != "/" {
        return plain(404, "not found");
    }
    if !req.param("t").is_some_and(|t| same(t, token)) {
        return plain(403, "missing or wrong token: use the address rigswig printed");
    }
    let Some(data) = render(req.param("tool")) else { return plain(404, "no such harness") };
    let nonce = random_hex();
    let body = PAGE.replace("__NONCE__", &nonce).replace("__DATA__", &data);
    Response {
        status: 200,
        headers: vec![
            ("Content-Type", "text/html; charset=utf-8".into()),
            ("Cache-Control", "no-store".into()),
            (
                "Content-Security-Policy",
                format!(
                    "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; img-src data:; \
                     base-uri 'none'; form-action 'self'; frame-ancestors 'none'"
                ),
            ),
            ("X-Content-Type-Options", "nosniff".into()),
            ("Referrer-Policy", "no-referrer".into()),
            ("X-Frame-Options", "DENY".into()),
            ("Cross-Origin-Opener-Policy", "same-origin".into()),
            ("Cross-Origin-Resource-Policy", "same-origin".into()),
        ],
        body,
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        421 => "Misdirected Request",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    }
}

fn write(stream: &mut TcpStream, r: &Response) {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        r.status,
        reason(r.status),
        r.body.len()
    );
    for (k, v) in &r.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(r.body.as_bytes());
}

fn read_head(stream: &TcpStream) -> Result<String, u16> {
    let mut reader = BufReader::new(stream.take(MAX_REQUEST as u64 + 1));
    let mut head = String::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return Err(400),
            Ok(_) => {
                head.push_str(&line);
                if head.len() > MAX_REQUEST {
                    return Err(431);
                }
                if line == "\r\n" || line == "\n" {
                    return Ok(head);
                }
            }
            Err(_) => return Err(400),
        }
    }
}

/// Serve until `idle` passes with no request. `ready` is called with the page's address once listening.
pub fn serve(
    port: u16,
    idle: Duration,
    render: &dyn Fn(Option<&str>) -> Option<String>,
    ready: &dyn Fn(&str),
) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    let token = random_hex();
    ready(&format!("http://127.0.0.1:{port}/?t={token}"));
    let mut last = Instant::now();
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                last = Instant::now();
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let resp = match read_head(&stream) {
                    Ok(head) => match parse(&head.replace("\r\n", "\n").replace('\n', "\r\n")) {
                        Some(req) => handle(&req, &token, port, render),
                        None => plain(400, "bad request"),
                    },
                    Err(code) => plain(code, "bad request"),
                };
                write(&mut stream, &resp);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if last.elapsed() >= idle {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "abc123";
    const PORT: u16 = 4567;

    fn req(head: &str) -> Request {
        parse(head).expect("parses")
    }

    fn ok(_: Option<&str>) -> Option<String> {
        Some("{\"harness\":1}".into())
    }

    fn get(path: &str, extra: &str) -> Response {
        handle(&req(&format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\n{extra}\r\n")), TOKEN, PORT, &ok)
    }

    #[test]
    fn serves_the_page_with_a_strict_csp() {
        let r = get("/?t=abc123", "");
        assert_eq!(r.status, 200);
        let csp = &r.headers.iter().find(|(k, _)| *k == "Content-Security-Policy").unwrap().1;
        assert!(csp.starts_with("default-src 'none'; script-src 'nonce-"));
        assert!(r.body.contains("{\"harness\":1}"));
        assert!(!r.body.contains("__NONCE__") && !r.body.contains("__DATA__"));
    }

    #[test]
    fn refuses_wrong_token_host_origin_method_and_path() {
        assert_eq!(get("/", "").status, 403);
        assert_eq!(get("/?t=abc124", "").status, 403);
        assert_eq!(get("/?t=abc12", "").status, 403);
        assert_eq!(get("/?t=abc123", "Origin: http://evil.example\r\n").status, 403);
        assert_eq!(get("/?t=abc123", "Sec-Fetch-Site: cross-site\r\n").status, 403);
        assert_eq!(get("/?t=abc123", "Origin: http://127.0.0.1:4567\r\nSec-Fetch-Site: same-origin\r\n").status, 200);
        assert_eq!(get("/x?t=abc123", "").status, 404);
        let rebind = handle(&req("GET /?t=abc123 HTTP/1.1\r\nHost: attacker.example:4567\r\n\r\n"), TOKEN, PORT, &ok);
        assert_eq!(rebind.status, 421);
        let post = handle(&req("POST /?t=abc123 HTTP/1.1\r\nHost: 127.0.0.1:4567\r\n\r\n"), TOKEN, PORT, &ok);
        assert_eq!(post.status, 405);
        let none =
            handle(&req("GET /?t=abc123&tool=nope HTTP/1.1\r\nHost: 127.0.0.1:4567\r\n\r\n"), TOKEN, PORT, &|_| None);
        assert_eq!(none.status, 404);
    }

    #[test]
    fn embedded_json_cannot_close_its_script_element() {
        let s = embed_json(&serde_json::json!({ "name": "</script><script>alert(1)</script>&\u{2028}" }));
        assert!(!s.contains('<') && !s.contains('>') && !s.contains('&') && !s.contains('\u{2028}'));
        let back: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back["name"], "</script><script>alert(1)</script>&\u{2028}");
    }

    #[test]
    fn query_values_are_decoded() {
        let r = req("GET /?t=a%20b&tool=codex+x HTTP/1.1\r\nHost: h\r\n\r\n");
        assert_eq!(r.param("t"), Some("a b"));
        assert_eq!(r.param("tool"), Some("codex x"));
        assert!(parse("GARBAGE").is_none());
    }

    #[test]
    fn tokens_are_random_and_long() {
        let (a, b) = (random_hex(), random_hex());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}

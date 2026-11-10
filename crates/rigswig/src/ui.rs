//! `rigswig ui`: the same harness as a local, read-only page.
//!
//! One repo, chosen at start. Every request re-reads the setup and returns one HTML page with the harness JSON inside.
//! The page can run an audit (`/api/audit/*`, read-only, always on) and, only with `--allow-writes`, change settings
//! (`/api/plan`, `/api/apply`, `/api/undo`). The server:
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
/// kevinferron.com's two faces (SIL OFL 1.1, unmodified upstream bytes; assets/fonts/README.md), inlined as data:
/// fonts so the page makes no request.
const JOSEFIN: &[u8] = include_bytes!("../assets/fonts/JosefinSans-wght.ttf");
const PIXELIFY: &[u8] = include_bytes!("../assets/fonts/PixelifySans-wght.ttf");

/// Standard base64 (RFC 4648, with padding).
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// The page with its fonts filled in, built once.
fn page_template() -> &'static str {
    static PAGE_WITH_FONTS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PAGE_WITH_FONTS.get_or_init(|| {
        PAGE.replace("__FONT_JOSEFIN__", &base64(JOSEFIN)).replace("__FONT_PIXELIFY__", &base64(PIXELIFY))
    })
}
const MAX_REQUEST: usize = 8 * 1024;
/// Write-mode request bodies are one small JSON change.
const MAX_BODY: usize = 16 * 1024;

/// 128 random bits as hex from the operating system's cryptographic random source. Failure is returned to the caller:
/// an authentication token or write-mode temporary name must never fall back to a non-cryptographic generator.
pub fn random_hex() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
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
    pub body: String,
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
    Some(Request { method, path: path.to_string(), query, headers, body: String::new() })
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
    /// A second harness on the same repo, and how the two differ (`?vs=<tool>`).
    pub other: Option<&'a Harness>,
    pub comparison: Option<rigswig_core::compare::Comparison>,
    pub tools: Vec<ToolChoice>,
    pub version: &'static str,
    /// Started with `--allow-writes`: the page may offer changes.
    pub writes: bool,
    /// Started with `--allow-github`: the page may audit public GitHub repos.
    pub github: bool,
}

#[derive(serde::Serialize)]
pub struct ToolChoice {
    pub id: String,
    pub score: u8,
    pub evidence: Vec<String>,
}

/// How a request is turned into page data: `(tool, vs)` to embedded JSON, `None` for an unknown tool.
pub type Render<'a> = dyn Fn(Option<&str>, Option<&str>) -> Option<String> + 'a;

/// An API's actions: `(route, tool, body)` to a JSON answer, or a status and a message. Write mode's are present only
/// when the page was started with `--allow-writes`; the audit's always are.
pub type Act<'a> = dyn Fn(&str, Option<&str>, &str) -> Result<String, (u16, String)> + 'a;

fn json(status: u16, body: String) -> Response {
    Response {
        status,
        headers: vec![
            ("Content-Type", "application/json; charset=utf-8".into()),
            ("Cache-Control", "no-store".into()),
            ("X-Content-Type-Options", "nosniff".into()),
        ],
        body,
    }
}

/// `/api/*`: POST only, the token in a header (a cross-site form cannot set one), an exact Origin, a JSON body. The
/// audit routes are read-only and always on; the others need write mode. The page's GET gate (Host, Sec-Fetch-Site)
/// has already run.
fn api(req: &Request, token: &str, port: u16, act: Option<&Act>, jobs: Option<&Act>) -> Response {
    let refuse = |status: u16, msg: &str| json(status, serde_json::json!({ "error": msg }).to_string());
    let target = if req.path.starts_with("/api/audit/") { jobs } else { act };
    let Some(act) = target else {
        return if req.path.starts_with("/api/audit/") {
            refuse(404, "this page cannot run audits")
        } else {
            refuse(404, "write mode is off: start the page with rigswig ui --allow-writes")
        };
    };
    if req.method != "POST" {
        return refuse(405, "POST only");
    }
    let origin_ok = req
        .header("Origin")
        .is_some_and(|o| o == format!("http://127.0.0.1:{port}") || o == format!("http://localhost:{port}"));
    if !origin_ok {
        return refuse(403, "a write needs this page's Origin");
    }
    if !req.header("X-Rigswig-Token").is_some_and(|t| same(t, token)) {
        return refuse(403, "missing or wrong token");
    }
    if !req.header("Content-Type").is_some_and(|c| c.starts_with("application/json")) {
        return refuse(415, "JSON only");
    }
    match act(&req.path, req.param("tool").filter(|t| !t.is_empty()), &req.body) {
        Ok(body) => json(200, body),
        Err((status, msg)) => refuse(status, &msg),
    }
}

/// Answer one request. `render(tool, vs)` resolves the harness for a tool id (or the default when `None`), and a
/// second one to compare with when `vs` is given, and returns the page data as embedded JSON.
pub fn handle(
    req: &Request,
    token: &str,
    port: u16,
    render: &Render,
    act: Option<&Act>,
    jobs: Option<&Act>,
) -> Response {
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
    if req.path.starts_with("/api/") {
        return api(req, token, port, act, jobs);
    }
    if req.method != "GET" {
        return plain(405, "GET only");
    }
    if req.path != "/" {
        return plain(404, "not found");
    }
    if !req.param("t").is_some_and(|t| same(t, token)) {
        return plain(403, "missing or wrong token: use the address rigswig printed");
    }
    let Some(data) = render(req.param("tool"), req.param("vs").filter(|v| !v.is_empty())) else {
        return plain(404, "no such harness");
    };
    let nonce = match random_hex() {
        Ok(n) => n,
        Err(_) => return plain(500, "secure randomness is unavailable"),
    };
    let body = page_template().replace("__NONCE__", &nonce).replace("__DATA__", &data);
    Response {
        status: 200,
        headers: vec![
            ("Content-Type", "text/html; charset=utf-8".into()),
            ("Cache-Control", "no-store".into()),
            (
                "Content-Security-Policy",
                format!(
                    "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; img-src data:; font-src data:;{connect} \
                     base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
                    connect = if act.is_some() || jobs.is_some() { " connect-src 'self';" } else { "" }
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
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
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

/// The request head, and the body when there is one (`Content-Length`, at most `MAX_BODY`), from one buffered reader.
fn read_request(stream: &TcpStream) -> Result<(String, String), u16> {
    let mut reader = BufReader::new(stream.take((MAX_REQUEST + MAX_BODY) as u64 + 1));
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
                    break;
                }
            }
            Err(_) => return Err(400),
        }
    }
    let len = head
        .lines()
        .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-length")))
        .map(|(_, v)| v.trim().parse::<usize>().map_err(|_| 400u16))
        .transpose()?
        .unwrap_or(0);
    if len > MAX_BODY {
        return Err(413);
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).map_err(|_| 400u16)?;
    Ok((head, String::from_utf8(body).map_err(|_| 400u16)?))
}

/// Serve until `idle` passes with no request. `ready` is called with the page's address once listening.
pub fn serve(
    port: u16,
    idle: Duration,
    render: &Render,
    act: Option<&Act>,
    jobs: Option<&Act>,
    ready: &dyn Fn(&str),
) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    let token = random_hex()?;
    ready(&format!("http://127.0.0.1:{port}/?t={token}"));
    let mut last = Instant::now();
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                last = Instant::now();
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let resp = match read_request(&stream) {
                    Ok((head, body)) => match parse(&head.replace("\r\n", "\n").replace('\n', "\r\n")) {
                        Some(mut req) => {
                            req.body = body;
                            handle(&req, &token, port, render, act, jobs)
                        }
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

    fn ok(_: Option<&str>, _: Option<&str>) -> Option<String> {
        Some("{\"harness\":1}".into())
    }

    fn get(path: &str, extra: &str) -> Response {
        handle(
            &req(&format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\n{extra}\r\n")),
            TOKEN,
            PORT,
            &ok,
            None,
            None,
        )
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
        let rebind = handle(
            &req("GET /?t=abc123 HTTP/1.1\r\nHost: attacker.example:4567\r\n\r\n"),
            TOKEN,
            PORT,
            &ok,
            None,
            None,
        );
        assert_eq!(rebind.status, 421);
        let post =
            handle(&req("POST /?t=abc123 HTTP/1.1\r\nHost: 127.0.0.1:4567\r\n\r\n"), TOKEN, PORT, &ok, None, None);
        assert_eq!(post.status, 405);
        let none = handle(
            &req("GET /?t=abc123&tool=nope HTTP/1.1\r\nHost: 127.0.0.1:4567\r\n\r\n"),
            TOKEN,
            PORT,
            &|_, _| None,
            None,
            None,
        );
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

    fn post(path: &str, headers: &str, body: &str, act: Option<&Act>) -> Response {
        let head = format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\n{headers}\r\n");
        let mut r = req(&head);
        r.body = body.into();
        handle(&r, TOKEN, PORT, &ok, act, None)
    }

    fn post_jobs(path: &str, headers: &str, body: &str, act: Option<&Act>, jobs: Option<&Act>) -> Response {
        let head = format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\n{headers}\r\n");
        let mut r = req(&head);
        r.body = body.into();
        handle(&r, TOKEN, PORT, &ok, act, jobs)
    }

    #[test]
    fn audits_run_without_write_mode_behind_the_same_gate() {
        let echo = |route: &str, tool: Option<&str>, body: &str| -> Result<String, (u16, String)> {
            Ok(format!("{route}|{}|{body}", tool.unwrap_or("-")))
        };
        let jobs: Option<&Act> = Some(&echo);
        let good = "Origin: http://127.0.0.1:4567\r\nX-Rigswig-Token: abc123\r\nContent-Type: application/json\r\n";
        let r = post_jobs("/api/audit/start?tool=codex", good, "{}", None, jobs);
        assert_eq!(r.status, 200, "no write mode needed");
        assert_eq!(r.body, "/api/audit/start|codex|{}");
        assert_eq!(post_jobs("/api/plan", good, "{}", None, jobs).status, 404, "the audit does not open write mode");
        assert_eq!(post_jobs("/api/audit/start", good, "{}", None, None).status, 404);
        let no_token = "Origin: http://127.0.0.1:4567\r\nContent-Type: application/json\r\n";
        assert_eq!(post_jobs("/api/audit/start", no_token, "{}", None, jobs).status, 403);
        let foreign = "Origin: http://evil.example\r\nX-Rigswig-Token: abc123\r\nContent-Type: application/json\r\n";
        assert_eq!(post_jobs("/api/audit/poll", foreign, "{}", None, jobs).status, 403);
        let form = "Origin: http://127.0.0.1:4567\r\nX-Rigswig-Token: abc123\r\nContent-Type: text/plain\r\n";
        assert_eq!(post_jobs("/api/audit/start", form, "{}", None, jobs).status, 415);
        let page = handle(
            &req(&format!("GET /?t=abc123 HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\n\r\n")),
            TOKEN,
            PORT,
            &ok,
            None,
            jobs,
        );
        let csp = &page.headers.iter().find(|(k, _)| *k == "Content-Security-Policy").unwrap().1;
        assert!(csp.contains("connect-src 'self'"), "the page may poll its own server: {csp}");
    }

    #[test]
    fn writes_need_write_mode_post_origin_header_token_and_json() {
        let echo = |route: &str, tool: Option<&str>, body: &str| -> Result<String, (u16, String)> {
            Ok(format!("{route}|{}|{body}", tool.unwrap_or("-")))
        };
        let act: Option<&Act> = Some(&echo);
        let good = "Origin: http://127.0.0.1:4567\r\nX-Rigswig-Token: abc123\r\nContent-Type: application/json\r\n";
        let r = post("/api/plan?tool=codex", good, "{}", act);
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "/api/plan|codex|{}");

        assert_eq!(post("/api/plan", good, "{}", None).status, 404, "write mode off: the routes do not exist");
        let get = handle(
            &req(&format!("GET /api/plan HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\n{good}\r\n")),
            TOKEN,
            PORT,
            &ok,
            act,
            None,
        );
        assert_eq!(get.status, 405);
        let no_origin = "X-Rigswig-Token: abc123\r\nContent-Type: application/json\r\n";
        assert_eq!(post("/api/plan", no_origin, "{}", act).status, 403, "a write needs an Origin");
        let foreign = "Origin: http://evil.example\r\nX-Rigswig-Token: abc123\r\nContent-Type: application/json\r\n";
        assert_eq!(post("/api/plan", foreign, "{}", act).status, 403);
        let query_token = "Origin: http://127.0.0.1:4567\r\nContent-Type: application/json\r\n";
        assert_eq!(post("/api/plan?t=abc123", query_token, "{}", act).status, 403, "the token must be in the header");
        let form = "Origin: http://127.0.0.1:4567\r\nX-Rigswig-Token: abc123\r\nContent-Type: text/plain\r\n";
        assert_eq!(post("/api/plan", form, "{}", act).status, 415, "no simple (form-like) requests");
        let cross = format!("{good}Sec-Fetch-Site: cross-site\r\n");
        assert_eq!(post("/api/plan", &cross, "{}", act).status, 403);
        let failing =
            |_: &str, _: Option<&str>, _: &str| -> Result<String, (u16, String)> { Err((409, "nope".into())) };
        let r = post("/api/apply", good, "{}", Some(&failing));
        assert_eq!(r.status, 409);
        assert_eq!(r.body, "{\"error\":\"nope\"}");
    }

    #[test]
    fn base64_known_answers() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
    }

    #[test]
    fn tokens_are_random_and_long() {
        let (a, b) = (random_hex().expect("OS randomness"), random_hex().expect("OS randomness"));
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}

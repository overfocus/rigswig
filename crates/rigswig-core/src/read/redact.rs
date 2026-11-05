//! Turning configuration into labels that are safe to print: never arguments, env, headers, URL paths or inline code.

use sha2::{Digest, Sha256};

const INTERPRETERS: &[&str] =
    &["node", "bash", "sh", "zsh", "python", "python3", "npx", "tsx", "env", "bun", "deno", "uv", "uvx", "pwsh"];
const INLINE_FLAGS: &[&str] = &["-c", "-e", "-p", "--eval", "--print", "-Command"];

fn basename(t: &str) -> &str {
    t.rsplit(['/', '\\']).next().unwrap_or(t)
}

/// A token is shown (as its file name) only if it looks like a path or a bare word; anything else is `(inline)`.
pub fn safe_base(t: &str) -> String {
    let ok = !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric() || "_./${}~-\\".contains(c));
    if ok { basename(t).to_string() } else { "(inline)".into() }
}

/// Split a command line into words, keeping quoted runs together (no expansion).
fn words(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in cmd.chars() {
        match quote {
            Some(q) if c == q => {
                quote = None;
                cur.push(c);
            }
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                cur.push(c);
            }
            None if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            None => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A shell word with its quotes removed (`"$DIR"/x.sh` is one word, `$DIR/x.sh`), no expansion.
fn unquote(t: &str) -> String {
    t.chars().filter(|c| *c != '"' && *c != '\'').collect()
}

/// The script a hook or server command runs, by file name only: env assignments, interpreters and flags are skipped,
/// inline code is `(inline)`, and arguments are never returned.
pub fn script_of(command: &str) -> Option<String> {
    for raw in words(command) {
        let is_assign = raw.split_once('=').is_some_and(|(k, _)| {
            !k.is_empty()
                && k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        if is_assign {
            continue;
        }
        if INLINE_FLAGS.contains(&raw.as_str()) {
            return Some("(inline)".into());
        }
        let t = unquote(&raw);
        if t.starts_with('-') || INTERPRETERS.contains(&basename(&t)) {
            continue;
        }
        return Some(safe_base(&t));
    }
    None
}

/// `scheme://host` of a URL: no user info, port, path, query or fragment. `None` if it does not parse.
pub fn url_origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        format!("[{}]", v6.split(']').next()?)
    } else {
        host_port.split(':').next().unwrap_or("").to_string()
    };
    if host.is_empty() || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    Some(format!("{}://{}", scheme.to_ascii_lowercase(), host.to_ascii_lowercase()))
}

/// Remove terminal control characters (escape sequences start with one), keeping newlines and tabs out too.
pub fn printable(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// SHA-256 of `text`, lower-case hex.
pub fn sha256_hex(text: &str) -> String {
    let d = Sha256::digest(text.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_names_only() {
        assert_eq!(script_of("node \"$CLAUDE_PROJECT_DIR\"/hooks/guard.mjs --strict").as_deref(), Some("guard.mjs"));
        assert_eq!(script_of("TOKEN=abc123 bash ~/bin/check.sh secret-arg").as_deref(), Some("check.sh"));
        assert_eq!(script_of("python3 -c 'import os; os.system(\"x\")'").as_deref(), Some("(inline)"));
        assert_eq!(script_of("echo $(cat ~/.ssh/id_rsa)").as_deref(), Some("echo"));
        assert_eq!(script_of("curl|sh").as_deref(), Some("(inline)"));
        assert_eq!(script_of("   ").as_deref(), None);
    }

    #[test]
    fn origins_drop_everything_but_scheme_and_host() {
        assert_eq!(
            url_origin("https://user:pw@API.example.com:8443/v1/key=secret?x=1#f").as_deref(),
            Some("https://api.example.com")
        );
        assert_eq!(url_origin("http://[::1]:3000/mcp").as_deref(), Some("http://[::1]"));
        assert_eq!(url_origin("not a url"), None);
        assert_eq!(url_origin("https:///path"), None);
    }

    #[test]
    fn printable_strips_escapes() {
        assert_eq!(printable("ok\u{1b}]0;title\u{7}\u{1b}[31mred"), "ok]0;title[31mred");
    }

    #[test]
    fn sha256_known_answer() {
        assert_eq!(sha256_hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}

//! Reading JSON configuration through [`Fs`], with problems reported as warnings that never quote file content.

use std::path::Path;

use serde_json::{Map, Value};

use crate::fs::Fs;

/// The outcome of reading one JSON file.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonFile {
    Missing,
    /// It exists but is not valid JSON (or not readable). The parser's message can quote the file, so it is dropped.
    Malformed,
    Ok(Value),
}

impl JsonFile {
    pub fn value(&self) -> Option<&Value> {
        match self {
            Self::Ok(v) => Some(v),
            _ => None,
        }
    }

    /// The top-level object, or an empty one.
    pub fn object(&self) -> Map<String, Value> {
        self.value().and_then(Value::as_object).cloned().unwrap_or_default()
    }

    pub fn present(&self) -> bool {
        !matches!(self, Self::Missing)
    }
}

pub fn read_json(fs: &dyn Fs, path: &Path) -> JsonFile {
    if !fs.exists(path) {
        return JsonFile::Missing;
    }
    match fs.read_to_string(path) {
        Ok(text) => match serde_json::from_str(strip_jsonc(&text).as_ref()) {
            Ok(v) => JsonFile::Ok(v),
            Err(_) => JsonFile::Malformed,
        },
        Err(_) => JsonFile::Malformed,
    }
}

/// Settings files are JSON; tolerate a BOM only. (Comments would need a JSONC parser; not modelled.)
fn strip_jsonc(text: &str) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Borrowed(text.strip_prefix('\u{feff}').unwrap_or(text))
}

/// Line and block comments stripped outside string literals (Devin CLI's JSON-with-comments config format).
/// Backslash escapes and `//` inside strings are respected; newlines inside block comments are kept so the
/// stripped text stays line-aligned.
#[must_use]
pub fn jsonc(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' => match chars.peek() {
                Some('/') => {
                    chars.next();
                    for l in chars.by_ref() {
                        if l == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\n' {
                            out.push('\n');
                        }
                        if n == '*' && chars.peek() == Some(&'/') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => out.push(c),
            },
            other => out.push(other),
        }
    }
    out
}

/// Read a JSON-with-comments file (Devin CLI's config files).
pub fn read_jsonc(fs: &dyn Fs, path: &Path) -> JsonFile {
    if !fs.exists(path) {
        return JsonFile::Missing;
    }
    match fs.read_to_string(path) {
        Ok(text) => match serde_json::from_str(jsonc(&text).as_ref()) {
            Ok(v) => JsonFile::Ok(v),
            Err(_) => JsonFile::Malformed,
        },
        Err(_) => JsonFile::Malformed,
    }
}

/// `v[key]` as a non-empty string.
pub fn str_at<'a>(v: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// `v[key]` as an object.
pub fn obj_at<'a>(v: &'a Map<String, Value>, key: &str) -> Option<&'a Map<String, Value>> {
    v.get(key).and_then(Value::as_object)
}

/// `v[key]` as a list of strings (non-strings dropped).
pub fn strs_at(v: &Map<String, Value>, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default()
}

/// Read a TOML file into the same JSON value shape, so adapters handle both alike.
pub fn read_toml(fs: &dyn Fs, path: &Path) -> JsonFile {
    if !fs.exists(path) {
        return JsonFile::Missing;
    }
    match fs.read_to_string(path) {
        Ok(text) => match toml::from_str::<Value>(text.strip_prefix('\u{feff}').unwrap_or(&text)) {
            Ok(v) => JsonFile::Ok(v),
            Err(_) => JsonFile::Malformed,
        },
        Err(_) => JsonFile::Malformed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemFs;

    #[test]
    fn toml_reads_as_json_values() {
        let fs = MemFs::new()
            .file(
                "/c/config.toml",
                "model = \"m\"\n[mcp_servers.docs]\ncommand = \"npx\"\nargs = [\"-y\"]\nenabled = false\n",
            )
            .file("/c/bad.toml", "model = ");
        let v = read_toml(&fs, Path::new("/c/config.toml"));
        let o = v.object();
        assert_eq!(str_at(&o, "model"), Some("m"));
        assert_eq!(o["mcp_servers"]["docs"]["enabled"], Value::Bool(false));
        assert_eq!(read_toml(&fs, Path::new("/c/bad.toml")), JsonFile::Malformed);
        assert_eq!(read_toml(&fs, Path::new("/c/none.toml")), JsonFile::Missing);
    }

    #[test]
    fn jsonc_strips_comments_and_keeps_strings() {
        let doc = "{\n  // one\n  \"model\": \"m\", /* two\n  lines */ // trailing\n  \"url\": \"https://host/x\" // in the url it must survive? outside\n  /*\n\"url2\": \"https://real\" */\n, \"b\": true\n}";
        let cleaned = jsonc(doc);
        assert!(!cleaned.contains("one"));
        assert!(!cleaned.contains("t two"));
        let v: Value = serde_json::from_str(cleaned.as_str()).expect("valid after strip");
        assert_eq!(str_at(v.as_object().unwrap(), "url"), Some("https://host/x"), "the // inside a string stays");
        assert_eq!(v["b"], Value::Bool(true));
        // Escaped quotes keep the string open.
        let q = jsonc("{\"a\": \"say \\\"ok //not-comment\\\"\" }");
        assert_eq!(
            serde_json::from_str::<Value>(q.as_str()).unwrap()["a"],
            Value::String("say \"ok //not-comment\"".into())
        );
    }

    #[test]
    fn jsonc_reads_devin_style_config() {
        let fs = MemFs::new().file(
            "/c/config.json",
            "// user config\n{\n  // Agent behavior\n  \"agent\": { \"model\": \"swe-1-6-fast\" },\n  /* multi\n     line */\n  \"permissions\": { \"allow\": [\"Exec(npm run)\"] }\n}",
        );
        let v = read_jsonc(&fs, Path::new("/c/config.json"));
        let o = v.object();
        assert_eq!(str_at(obj_at(&o, "agent").unwrap(), "model"), Some("swe-1-6-fast"));
        assert_eq!(o["permissions"]["allow"][0], Value::String("Exec(npm run)".into()));
        assert_eq!(read_jsonc(&fs, Path::new("/c/none.json")), JsonFile::Missing);
        assert_eq!(read_jsonc(&fs, Path::new("/c/broken")), JsonFile::Missing);
    }
}

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

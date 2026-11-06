//! The subset of YAML that `config.yaml` files use, as JSON values (like [`crate::read::json`]).
//!
//! Supported: nested maps at any depth, plain scalars (quoted, numbers, booleans, `null`), inline lists
//! (`[a, b]`), block lists (`key:` then `- item` lines of scalars), and `|` block scalars. Whole-line comments
//! and ` #` comments outside quotes are stripped. Anything else — anchors, tags, flow mappings, lists of maps —
//! is dropped and reported in `notes` (feature and line number, never the content): nothing is guessed at.
//!
//! One parser for every adapter that reads a `config.yaml`.

use std::path::Path;

use serde_json::{Map, Value};

use crate::fs::Fs;

/// A parsed YAML file: the value when the whole file parsed, and what was not supported.
#[derive(Debug, Clone, PartialEq)]
pub struct YamlFile {
    pub value: Option<Value>,
    /// `line N: <feature>`; the content after the feature name is never quoted.
    pub notes: Vec<String>,
}

impl YamlFile {
    /// The top-level object, or an empty one.
    pub fn object(&self) -> Map<String, Value> {
        self.value.as_ref().and_then(Value::as_object).cloned().unwrap_or_default()
    }
}

/// Read a YAML file through [`Fs`]. A missing file is `value: None` with no notes.
pub fn read_yaml(fs: &dyn Fs, path: &Path) -> YamlFile {
    if !fs.exists(path) {
        return YamlFile { value: None, notes: Vec::new() };
    }
    match fs.read_to_string(path) {
        Ok(text) => parse(text.strip_prefix('\u{feff}').unwrap_or(&text)),
        Err(_) => YamlFile { value: None, notes: vec!["line 1: the file could not be read".into()] },
    }
}

struct Line {
    indent: usize,
    text: String,
    number: usize,
}

/// Parse the whole text. The top level must be a mapping; a scalar or list file is reported, not converted.
pub fn parse(text: &str) -> YamlFile {
    let mut notes: Vec<String> = Vec::new();
    let lines: Vec<Line> = text
        .split_inclusive('\n')
        .enumerate()
        .filter_map(|(n, raw)| {
            let line = strip_comment(raw.trim_end_matches(['\n', '\r']));
            if line.trim().is_empty() {
                return None;
            }
            if line.starts_with('\t') {
                notes.push(format!("line {}: tab indentation", n + 1));
                return None;
            }
            Some(Line { indent: indent_of(line), text: line.trim_end().to_string(), number: n + 1 })
        })
        .collect();

    let first = lines.first();
    if first.is_none() {
        return YamlFile { value: None, notes: vec!["line 1: the file is empty or only comments".into()] };
    }
    let indent = lines[0].indent;
    let mut i = 0;
    let top = block(&lines, &mut i, indent, &mut notes);
    if i < lines.len() {
        notes.push(format!("line {}: over-indented", lines[i].number));
    }
    let value = match top {
        Value::Object(_) => Some(top),
        Value::Null => None,
        _ => {
            notes.insert(0, "line 1: top level is not a mapping".into());
            None
        }
    };
    notes.sort();
    YamlFile { value, notes }
}

/// One block of lines at exactly `indent`: a mapping or a list. Leaves `i` on the first line that does not
/// belong to the block.
fn block(lines: &[Line], i: &mut usize, indent: usize, notes: &mut Vec<String>) -> Value {
    if *i >= lines.len() || lines[*i].indent != indent {
        return Value::Null;
    }
    let content = |l: &Line| l.text[l.indent..].to_string();
    let is_list = content(&lines[*i]).starts_with("- ");
    if is_list {
        let mut out = Vec::new();
        while *i < lines.len() && lines[*i].indent == indent {
            let entry = content(&lines[*i]).strip_prefix("- ").unwrap_or_default().trim().to_string();
            let this = *i;
            *i += 1;
            if is_list_of_map(&entry, lines, *i, indent) {
                notes.push(format!("line {}: a list of maps", lines[this].number));
                *i = skip_deeper(lines, *i, indent);
                continue;
            }
            if entry.is_empty() {
                notes.push(format!("line {}: empty list entry", lines[this].number));
            }
            out.push(scalar(lines[this].number, &entry, notes));
        }
        return Value::Array(out);
    }
    let mut map = Map::new();
    while *i < lines.len() && lines[*i].indent == indent {
        let line = &lines[*i];
        let entry = content(line);
        let this_key = key_of(&entry, line.number, notes);
        *i += 1;
        let Some((key, rest)) = this_key else { continue };
        let value = match rest {
            Some(rest) => value_text(line.number, &rest, lines, i, indent, notes),
            None => {
                if *i < lines.len() && lines[*i].indent > indent {
                    let child = lines[*i].indent;
                    block(lines, i, child, notes)
                } else {
                    Value::Null
                }
            }
        };
        map.insert(key, value);
    }
    Value::Object(map)
}

/// Does the list entry at `entry` (whose next line is at `next`) start a list of maps?
fn is_list_of_map(entry: &str, lines: &[Line], next: usize, indent: usize) -> bool {
    if entry.contains(": ") || entry.ends_with(':') {
        return true;
    }
    entry.is_empty() && next < lines.len() && lines[next].indent > indent
}

/// Skip every line deeper than `indent` (the body of a dropped entry).
fn skip_deeper(lines: &[Line], from: usize, indent: usize) -> usize {
    let mut i = from;
    while i < lines.len() && lines[i].indent > indent {
        i += 1;
    }
    i
}

/// A `key: value` or `key:` line: the key, and `Some` when the rest of the line holds the value. A line that is
/// not a key is reported and dropped (`None`).
fn key_of(line: &str, number: usize, notes: &mut Vec<String>) -> Option<(String, Option<String>)> {
    let (raw_key, rest) = line.split_once(':')?;
    if raw_key.trim().is_empty() {
        notes.push(format!("line {number}: no key before the colon"));
        return None;
    }
    let after = rest.trim().is_empty();
    if !after && !(rest.starts_with(' ') || rest.starts_with('\t')) {
        notes.push(format!("line {number}: no space after the colon"));
        return None;
    }
    let value = if after { None } else { Some(rest.trim().to_string()) };
    Some((unquote(raw_key.trim()), value))
}

/// The text after `key:`: a block scalar, or a scalar or inline list.
fn value_text(
    number: usize,
    rest: &str,
    lines: &[Line],
    i: &mut usize,
    indent: usize,
    notes: &mut Vec<String>,
) -> Value {
    let trimmed = rest.trim_end();
    if trimmed == "|" {
        let start = *i;
        let mut base = usize::MAX;
        while *i < lines.len() && lines[*i].indent > indent {
            base = base.min(lines[*i].indent);
            *i += 1;
        }
        notes.push(format!("line {number}: block scalar, counted as its own lines"));
        if base == usize::MAX {
            return Value::Null;
        }
        let body: Vec<String> = lines[start..*i].iter().map(|l| l.text[base..].to_string()).collect();
        return Value::String(body.join("\n"));
    }
    if trimmed.starts_with('{') {
        if trimmed == "{}" {
            return Value::Object(Map::new());
        }
        notes.push(format!("line {number}: a flow mapping"));
        return Value::Null;
    }
    if trimmed == "[]" {
        return Value::Array(Vec::new());
    }
    scalar_or_inline(number, trimmed, notes)
}

fn scalar(number: usize, text: &str, notes: &mut Vec<String>) -> Value {
    let t = text.trim();
    if t.starts_with('&') || t.starts_with('*') {
        notes.push(format!("line {number}: an anchor or alias"));
        return Value::Null;
    }
    if t.starts_with("!!") {
        notes.push(format!("line {number}: a tag"));
        return Value::Null;
    }
    if t.starts_with('[') && !t.ends_with(']') {
        notes.push(format!("line {number}: a flow list"));
        return Value::Null;
    }
    match t {
        "" => Value::Null,
        "true" | "True" => Value::Bool(true),
        "false" | "False" => Value::Bool(false),
        "null" | "~" | "Null" => Value::Null,
        other => {
            if let Ok(n) = other.parse::<i64>() {
                return Value::Number(n.into());
            }
            let unquoted = unquote(other);
            if unquoted != *other { Value::String(unquoted) } else { Value::String(other.to_string()) }
        }
    }
}

fn scalar_or_inline(number: usize, text: &str, notes: &mut Vec<String>) -> Value {
    let t = text.trim();
    if t.len() >= 2 && t.starts_with('[') && t.ends_with(']') {
        let inner = &t[1..t.len() - 1];
        return Value::Array(split_items(inner).into_iter().map(|part| scalar(number, &part, notes)).collect());
    }
    scalar(number, t, notes)
}

/// The items of an inline list, unquoted ones kept whole, quoted runs kept together.
fn split_items(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in inner.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c == ',' => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
                continue;
            }
            None => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// The comment ` #...` (or a whole leading `#`) removed, but not inside quotes.
fn strip_comment(line: &str) -> &str {
    let s = line.trim_end();
    if s.trim_start().starts_with('#') {
        return "";
    }
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => {
                if c == '\'' || c == '"' {
                    quote = Some(c);
                } else if c == '#' {
                    let before = s[..i].chars().next_back();
                    if before.is_none_or(|b| b == ' ' || b == '\t') {
                        return s[..i].trim_end();
                    }
                }
            }
        }
    }
    s
}

/// Strip outer quotes, honouring the escapes the settings files use.
fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() >= 2 && b[0] == b'\'' && *b.last().unwrap_or(&b' ') == b'\'' {
        return s[1..s.len() - 1].replace("''", "'");
    }
    if b.len() >= 2 && b[0] == b'"' && *b.last().unwrap_or(&b' ') == b'"' {
        let inner = &s[1..s.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut it = inner.chars();
        while let Some(c) = it.next() {
            if c == '\\' {
                match it.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(o) => out.push(o),
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        return out;
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(s: &str) -> Map<String, Value> {
        parse(s).object()
    }

    #[test]
    fn nested_maps_scalars_and_lists() {
        let m = map(
            "model:\n  default: \"org/model\"\n  provider: nous\napprovals:\n  mode: smart\ncount: 4096\nraw: 'it''s'\nflag: true\nnone: null\nlist: [a, 'b c']\nblock:\n  - x\n  - 'y'\n",
        );
        assert_eq!(m["model"]["default"], "org/model");
        assert_eq!(m["model"]["provider"], "nous");
        assert_eq!(m["approvals"]["mode"], "smart");
        assert_eq!(m["count"], Value::Number(4096.into()));
        assert_eq!(m["raw"], "it's");
        assert_eq!(m["flag"], Value::Bool(true));
        assert_eq!(m["none"], Value::Null);
        assert_eq!(m["list"], Value::Array(vec![Value::String("a".into()), Value::String("b c".into())]));
        assert_eq!(m["block"], Value::Array(vec![Value::String("x".into()), Value::String("y".into())]));
    }

    #[test]
    fn deep_nesting_under_mcp_servers() {
        let m = map("mcp_servers:\n  time:\n    command: uvx\n    args:\n      - mcp-server-time\n      - '-y'\n");
        assert_eq!(m["mcp_servers"]["time"]["command"], "uvx");
        assert_eq!(m["mcp_servers"]["time"]["args"][1], "-y");
    }

    #[test]
    fn comments_and_quotes() {
        let m = map("# top\nmodel: default # trailing\nname: \"has # hash\"\nurl: 'http://x/#frag'\n");
        assert_eq!(m["model"], "default");
        assert_eq!(m["name"], "has # hash");
        assert_eq!(m["url"], "http://x/#frag");
    }

    #[test]
    fn empty_sentinel_model() {
        let m = map("model: \"\"\n");
        assert_eq!(m["model"], Value::String("".into()));
        assert_eq!(map("model:\n")["model"], Value::Null);
    }

    #[test]
    fn unsupported_constructions_are_reported_not_guessed() {
        let f = parse("a: &anchor 1\nb: *anchor\nc: {x: 1}\nd:\n  - k: v\n");
        let m = f.object();
        assert_eq!(m["a"], Value::Null, "the dropped entry is not guessed at");
        assert_eq!(m["b"], Value::Null);
        assert_eq!(m["c"], Value::Null);
        assert_eq!(m["d"], Value::Array(vec![]), "only the map list entry is dropped");
        for wanted in ["anchor", "alias", "flow mapping", "list of maps"] {
            assert!(f.notes.iter().any(|n| n.contains(wanted)), "notes: {:?}", f.notes);
        }
    }

    #[test]
    fn top_level_must_be_a_mapping() {
        let f = parse("- a\n- b\n");
        assert_eq!(f.value, None);
        assert_eq!(f.notes[0], "line 1: top level is not a mapping");
    }

    #[test]
    fn block_scalar_keeps_joined_lines() {
        let f = parse("text: |\n  one\n  two\nnext: 1\n");
        assert_eq!(f.object()["text"], Value::String("one\ntwo".into()));
        assert_eq!(f.object()["next"], Value::Number(1.into()));
    }

    #[test]
    fn missing_file_is_none_without_notes() {
        let f = parse("");
        assert_eq!(f.value, None);
        assert!(f.notes.iter().any(|n| n.contains("empty")));
    }
}

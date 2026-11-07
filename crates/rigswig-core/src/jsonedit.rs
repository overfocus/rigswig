//! Changing one member of a JSON settings file in place, so the rest of the file keeps its own layout: the planner
//! edits the text a person wrote rather than serialising the whole object again. Only what changes is touched: a
//! member's value is replaced, a member is added after the last one with the separators the object already uses, or
//! a member is removed with its comma. Every edit returns `None` when the text is not the shape it expects, and the
//! caller checks that the result parses to the object it meant, so the worst case is the old full rewrite, never a
//! wrong file.

use serde_json::Value;

/// A parsed value with byte spans into the text it came from.
struct Node {
    start: usize,
    end: usize,
    kind: Kind,
}

enum Kind {
    Object(Vec<Member>),
    Array,
    Scalar,
}

struct Member {
    key: String,
    key_start: usize,
    key_end: usize,
    value: Node,
}

const MAX_DEPTH: usize = 64;

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn string(&mut self) -> Option<()> {
        self.i += 1;
        while self.i < self.s.len() {
            match self.s[self.i] {
                b'\\' => self.i += 2,
                b'"' => {
                    self.i += 1;
                    return Some(());
                }
                _ => self.i += 1,
            }
        }
        None
    }

    fn value(&mut self, depth: usize) -> Option<Node> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.ws();
        let start = self.i;
        let kind = match *self.s.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut members = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                } else {
                    loop {
                        self.ws();
                        if self.s.get(self.i) != Some(&b'"') {
                            return None;
                        }
                        let key_start = self.i;
                        self.string()?;
                        let key_end = self.i;
                        let key: String = serde_json::from_slice(&self.s[key_start..key_end]).ok()?;
                        self.ws();
                        if self.s.get(self.i) != Some(&b':') {
                            return None;
                        }
                        self.i += 1;
                        let value = self.value(depth + 1)?;
                        members.push(Member { key, key_start, key_end, value });
                        self.ws();
                        match self.s.get(self.i)? {
                            b',' => self.i += 1,
                            b'}' => {
                                self.i += 1;
                                break;
                            }
                            _ => return None,
                        }
                    }
                }
                Kind::Object(members)
            }
            b'[' => {
                self.i += 1;
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                } else {
                    loop {
                        self.value(depth + 1)?;
                        self.ws();
                        match self.s.get(self.i)? {
                            b',' => self.i += 1,
                            b']' => {
                                self.i += 1;
                                break;
                            }
                            _ => return None,
                        }
                    }
                }
                Kind::Array
            }
            b'"' => {
                self.string()?;
                Kind::Scalar
            }
            _ => {
                while self.i < self.s.len()
                    && !matches!(self.s[self.i], b',' | b']' | b'}' | b' ' | b'\t' | b'\n' | b'\r')
                {
                    self.i += 1;
                }
                if self.i == start {
                    return None;
                }
                Kind::Scalar
            }
        };
        Some(Node { start, end: self.i, kind })
    }
}

/// The top-level object of `text` (after an optional byte-order mark), with nothing but white space after it.
fn root(text: &str) -> Option<Node> {
    let mut p = Parser { s: text.as_bytes(), i: if text.starts_with('\u{feff}') { 3 } else { 0 } };
    let node = p.value(0)?;
    p.ws();
    (p.i == p.s.len() && matches!(node.kind, Kind::Object(_))).then_some(node)
}

fn members(node: &Node) -> &[Member] {
    match &node.kind {
        Kind::Object(m) => m,
        _ => &[],
    }
}

/// The white space at the start of the line `at` is on.
fn line_indent(text: &str, at: usize) -> &str {
    let line = text[..at].rfind('\n').map_or(0, |n| n + 1);
    let rest = &text[line..];
    &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()]
}

/// How an object lays out its members, read from the object itself.
struct Layout {
    /// Members on their own lines (read from the first one), so a new object value is laid out that way too.
    multiline: bool,
    /// The text between the opening brace and the first key, used before every member added.
    lead: String,
    /// The text between a key and its value (`": "` in most files).
    colon: String,
    /// One level of indentation, for a value rendered over several lines.
    unit: String,
    /// The indentation of the object's members.
    indent: String,
}

fn layout(text: &str, obj: &Node) -> Layout {
    let ms = members(obj);
    let multiline = text[obj.start..obj.end].contains('\n');
    let parent = line_indent(text, obj.start).to_string();
    match ms.first() {
        Some(m) => {
            let indent = line_indent(text, m.key_start).to_string();
            let unit = indent.strip_prefix(parent.as_str()).filter(|u| !u.is_empty()).unwrap_or("  ").to_string();
            Layout {
                multiline: text[obj.start + 1..m.key_start].contains('\n'),
                lead: text[obj.start + 1..m.key_start].to_string(),
                colon: text[m.key_end..m.value.start].to_string(),
                unit,
                indent,
            }
        }
        None => Layout {
            multiline,
            lead: if multiline { format!("\n{parent}  ") } else { " ".into() },
            colon: ": ".into(),
            unit: "  ".into(),
            indent: format!("{parent}  "),
        },
    }
}

/// A new value as text: on one line, or for an object in a multi-line file, one member a line.
fn render(v: &Value, multiline: bool, indent: &str, unit: &str) -> String {
    match v {
        Value::Object(o) if !o.is_empty() => {
            let parts: Vec<String> = o
                .iter()
                .map(|(k, x)| {
                    format!("{}: {}", Value::String(k.clone()), render(x, multiline, &format!("{indent}{unit}"), unit))
                })
                .collect();
            if multiline {
                format!("{{\n{indent}{unit}{}\n{indent}}}", parts.join(&format!(",\n{indent}{unit}")))
            } else {
                format!("{{ {} }}", parts.join(", "))
            }
        }
        Value::Array(a) if !a.is_empty() => {
            let parts: Vec<String> = a.iter().map(|x| render(x, false, indent, unit)).collect();
            format!("[{}]", parts.join(", "))
        }
        _ => v.to_string(),
    }
}

/// An array as text, laid out like the one it replaces: on one line if that one was, else one element a line.
fn render_array_like(text: &str, old: &Node, v: &Value, unit: &str) -> String {
    let Value::Array(a) = v else { return v.to_string() };
    if a.is_empty() || !text[old.start..old.end].contains('\n') {
        return render(v, false, "", unit);
    }
    let indent = line_indent(text, old.start);
    let parts: Vec<String> = a.iter().map(|x| x.to_string()).collect();
    format!("[\n{indent}{unit}{}\n{indent}]", parts.join(&format!(",\n{indent}{unit}")))
}

fn splice(text: &str, start: usize, end: usize, with: &str) -> String {
    format!("{}{}{}", &text[..start], with, &text[end..])
}

/// Sets (`Some`) or removes (`None`) a member of the object `obj`; `value` is laid out to fit.
fn set_in(text: &str, obj: &Node, key: &str, value: Option<&Value>) -> Option<String> {
    let ms = members(obj);
    let lay = layout(text, obj);
    let at = ms.iter().position(|m| m.key == key);
    // A key written twice is ambiguous to edit by hand; leave it to the full rewrite.
    if ms.iter().filter(|m| m.key == key).count() > 1 {
        return None;
    }
    match (at, value) {
        (Some(i), Some(v)) => {
            let old = &ms[i].value;
            let new = match (&old.kind, v) {
                (Kind::Array, Value::Array(_)) => render_array_like(text, old, v, &lay.unit),
                _ => render(v, lay.multiline, &lay.indent, &lay.unit),
            };
            Some(splice(text, old.start, old.end, &new))
        }
        (None, Some(v)) => {
            let member = format!(
                "{}{}{}",
                Value::String(key.into()),
                lay.colon,
                render(v, lay.multiline, &lay.indent, &lay.unit)
            );
            match ms.last() {
                Some(last) => Some(splice(text, last.value.end, last.value.end, &format!(",{}{member}", lay.lead))),
                None => {
                    let close = if lay.multiline { format!("\n{}", line_indent(text, obj.start)) } else { " ".into() };
                    Some(splice(text, obj.start, obj.end, &format!("{{{}{member}{close}}}", lay.lead)))
                }
            }
        }
        (Some(i), None) => {
            if ms.len() == 1 {
                Some(splice(text, obj.start, obj.end, "{}"))
            } else if i + 1 < ms.len() {
                Some(splice(text, ms[i].key_start, ms[i + 1].key_start, ""))
            } else {
                Some(splice(text, ms[i - 1].value.end, ms[i].value.end, ""))
            }
        }
        (None, None) => Some(text.to_string()),
    }
}

/// `text` with the top-level member `key` set to `value`, or removed when `value` is `None`.
pub(crate) fn set_top(text: &str, key: &str, value: Option<&Value>) -> Option<String> {
    let r = root(text)?;
    set_in(text, &r, key, value)
}

/// `text` with `outer[key]` set to `value`, adding `outer` as an object when it is missing.
pub(crate) fn set_nested(text: &str, outer: &str, key: &str, value: &Value) -> Option<String> {
    let r = root(text)?;
    let ms = members(&r);
    if ms.iter().filter(|m| m.key == outer).count() > 1 {
        return None;
    }
    match ms.iter().find(|m| m.key == outer) {
        Some(m) if matches!(m.value.kind, Kind::Object(_)) => set_in(text, &m.value, key, Some(value)),
        Some(_) => None,
        None => {
            let mut o = serde_json::Map::new();
            o.insert(key.into(), value.clone());
            set_in(text, &r, outer, Some(&Value::Object(o)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_replaced_and_nothing_else_moves() {
        let t = "{ \"model\":\"big\",  \"enabledPlugins\": {\"a@m\": true,\"b@m\":true} }\n";
        assert_eq!(
            set_nested(t, "enabledPlugins", "a@m", &Value::Bool(false)).unwrap(),
            "{ \"model\":\"big\",  \"enabledPlugins\": {\"a@m\": false,\"b@m\":true} }\n"
        );
    }

    #[test]
    fn a_member_is_added_with_the_separators_the_object_uses() {
        let multi = "{\n    \"model\": \"big\"\n}\n";
        assert_eq!(
            set_nested(multi, "enabledPlugins", "a@m", &Value::Bool(false)).unwrap(),
            "{\n    \"model\": \"big\",\n    \"enabledPlugins\": {\n        \"a@m\": false\n    }\n}\n"
        );
        let inline = "{\"model\":\"big\"}";
        assert_eq!(
            set_nested(inline, "enabledPlugins", "a@m", &Value::Bool(false)).unwrap(),
            "{\"model\":\"big\",\"enabledPlugins\":{ \"a@m\": false }}"
        );
        let nested = "{\n  \"enabledPlugins\": {\n    \"a@m\": true\n  }\n}\n";
        assert_eq!(
            set_nested(nested, "enabledPlugins", "b@m", &Value::Bool(false)).unwrap(),
            "{\n  \"enabledPlugins\": {\n    \"a@m\": true,\n    \"b@m\": false\n  }\n}\n"
        );
    }

    #[test]
    fn members_that_share_a_line_get_a_new_object_on_that_line_too() {
        // The object spans lines (its array does), but its members start on the brace's line.
        let t = "{ \"permissions\": { \"allow\": [\n  \"x\"\n] } }\n";
        assert_eq!(
            set_nested(t, "enabledPlugins", "a@m", &Value::Bool(false)).unwrap(),
            "{ \"permissions\": { \"allow\": [\n  \"x\"\n] }, \"enabledPlugins\": { \"a@m\": false } }\n"
        );
    }

    #[test]
    fn a_member_is_removed_with_its_comma() {
        let t = "{\n  \"a\": [\"x\"],\n  \"b\": 1,\n  \"c\": 2\n}\n";
        assert_eq!(set_top(t, "a", None).unwrap(), "{\n  \"b\": 1,\n  \"c\": 2\n}\n");
        assert_eq!(set_top(t, "c", None).unwrap(), "{\n  \"a\": [\"x\"],\n  \"b\": 1\n}\n");
        assert_eq!(set_top("{\"a\": 1}", "a", None).unwrap(), "{}");
    }

    #[test]
    fn an_array_keeps_its_one_line_or_many_lines() {
        let one = "{ \"allow\": [\"x\", \"y\"] }";
        assert_eq!(set_top(one, "allow", Some(&serde_json::json!(["x"]))).unwrap(), "{ \"allow\": [\"x\"] }");
        let many = "{\n  \"allow\": [\n    \"x\"\n  ]\n}";
        assert_eq!(
            set_top(many, "allow", Some(&serde_json::json!(["x", "y"]))).unwrap(),
            "{\n  \"allow\": [\n    \"x\",\n    \"y\"\n  ]\n}"
        );
    }

    #[test]
    fn what_it_cannot_edit_safely_it_leaves_to_the_caller() {
        assert!(set_top("[1]", "a", None).is_none(), "not an object");
        assert!(set_top("{\"a\": 1} x", "a", None).is_none(), "trailing text");
        assert!(set_top("{\"a\": 1, \"a\": 2}", "a", Some(&Value::Bool(true))).is_none(), "a key written twice");
        assert!(set_nested("{\"p\": 1}", "p", "k", &Value::Bool(true)).is_none(), "not an object to set into");
        assert!(set_top("{\"a\": \"unterminated}", "a", None).is_none());
    }

    #[test]
    fn strings_with_braces_and_escapes_do_not_confuse_it() {
        let t = "{\"cmd\": \"echo \\\"}{\\\" ,\", \"p\": {\"k\": true}}";
        assert_eq!(
            set_nested(t, "p", "k", &Value::Bool(false)).unwrap(),
            "{\"cmd\": \"echo \\\"}{\\\" ,\", \"p\": {\"k\": false}}"
        );
    }
}

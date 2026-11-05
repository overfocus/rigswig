//! The YAML front matter of agent, skill, command and rule files: the subset those files use.
//!
//! Supported: `key: value` (plain, `'single'` or `"double"` quoted), `key: [a, b]`, a `key:` followed by `- item` lines,
//! block scalars (`|` keeps newlines, `>` folds them; blank lines inside are kept), and plain values continued on more
//! indented lines. Comment lines are skipped. A nested map is not supported: its key is skipped and reported in
//! `unsupported`, never guessed at. One parser for every adapter.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FmValue {
    Str(String),
    List(Vec<String>),
}

impl FmValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            Self::List(_) => None,
        }
    }

    /// A list as written, or a comma-separated string split into one (`tools: Read, Grep`).
    pub fn as_list(&self) -> Vec<String> {
        match self {
            Self::List(v) => v.clone(),
            Self::Str(s) => s.split(',').map(|x| unquote(x.trim())).filter(|x| !x.is_empty()).collect(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrontMatter {
    pub fields: BTreeMap<String, FmValue>,
    /// Keys whose values used YAML this reader does not support.
    pub unsupported: Vec<String>,
    /// Byte offset where the body starts (after the closing `---` line); 0 when there is no front matter.
    pub body_start: usize,
}

impl FrontMatter {
    pub fn str(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(FmValue::as_str).filter(|s| !s.is_empty())
    }
}

fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() >= 2 && b[0] == b'\'' && b[b.len() - 1] == b'\'' {
        return s[1..s.len() - 1].replace("''", "'");
    }
    if b.len() >= 2 && b[0] == b'"' && b[b.len() - 1] == b'"' {
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

/// Strip a trailing ` # comment` from a plain (unquoted) value.
fn strip_comment(v: &str) -> &str {
    if v.starts_with('\'') || v.starts_with('"') {
        return v;
    }
    match v.find(" #") {
        Some(i) => v[..i].trim_end(),
        None => v,
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn key_of(line: &str) -> Option<(&str, &str)> {
    let (k, rest) = line.split_once(':')?;
    let ok = !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !ok || !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
        return None;
    }
    Some((k, rest.trim()))
}

/// Parse the front matter at the start of `text` (`---` on the first line, closed by a `---` line).
pub fn parse(text: &str) -> FrontMatter {
    let mut fm = FrontMatter::default();
    let text_nobom = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bom = text.len() - text_nobom.len();
    let mut lines = text_nobom.split_inclusive('\n');
    let first = lines.next().unwrap_or("");
    if first.trim_end() != "---" {
        return fm;
    }
    let mut offset = bom + first.len();
    let mut block: Vec<&str> = Vec::new();
    let mut closed = false;
    for l in lines {
        offset += l.len();
        if l.trim_end() == "---" {
            closed = true;
            break;
        }
        block.push(l.trim_end_matches(['\n', '\r']));
    }
    if !closed {
        return fm;
    }
    fm.body_start = offset;

    let mut i = 0;
    while i < block.len() {
        let line = block[i];
        i += 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') || indent_of(line) > 0 {
            continue;
        }
        let Some((key, rest)) = key_of(line) else { continue };
        let rest = strip_comment(rest);

        // The more-indented lines that belong to this key (blank lines included while more follow).
        let mut j = i;
        while j < block.len() && (block[j].trim().is_empty() || indent_of(block[j]) > 0 || block[j].starts_with("- ")) {
            j += 1;
        }
        while j > i && block[j - 1].trim().is_empty() {
            j -= 1;
        }
        let more = &block[i..j];
        i = j;

        let value = if let Some(style) = rest.chars().next().filter(|c| *c == '|' || *c == '>') {
            if !rest[1..].chars().all(|c| c == '-' || c == '+' || c.is_ascii_digit()) {
                fm.unsupported.push(key.into());
                continue;
            }
            let ind = more.iter().filter(|l| !l.trim().is_empty()).map(|l| indent_of(l)).min().unwrap_or(0);
            let body: Vec<&str> = more.iter().map(|l| if l.len() >= ind { &l[ind..] } else { "" }).collect();
            let s = if style == '|' {
                body.join("\n")
            } else {
                // Folded: lines join with a space; a blank line becomes a newline.
                let mut out = String::new();
                for (n, l) in body.iter().enumerate() {
                    if l.is_empty() {
                        out.push('\n');
                    } else {
                        if n > 0 && !out.is_empty() && !out.ends_with('\n') {
                            out.push(' ');
                        }
                        out.push_str(l);
                    }
                }
                out
            };
            FmValue::Str(s)
        } else if rest.starts_with('[') {
            let inner = rest.trim_start_matches('[').trim_end_matches(']');
            FmValue::List(inner.split(',').map(|x| unquote(x.trim())).filter(|x| !x.is_empty()).collect())
        } else if rest.is_empty()
            && !more.is_empty()
            && more.iter().all(|l| l.trim().is_empty() || l.trim_start().starts_with("- ") || l.trim() == "-")
        {
            FmValue::List(
                more.iter()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| unquote(strip_comment(l.trim_start().trim_start_matches('-').trim())))
                    .collect(),
            )
        } else if more.iter().any(|l| key_of(l.trim_start()).is_some() && !l.trim_start().starts_with("- "))
            && rest.is_empty()
        {
            fm.unsupported.push(key.into());
            continue;
        } else if !more.is_empty() {
            // A plain scalar continued on indented lines: folded with spaces.
            let mut parts = vec![rest.to_string()];
            parts.extend(more.iter().map(|l| l.trim().to_string()));
            FmValue::Str(unquote(&parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")))
        } else {
            FmValue::Str(unquote(rest))
        };
        fm.fields.insert(key.to_string(), value);
    }
    fm
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(fm: &FrontMatter, k: &str) -> String {
        fm.str(k).unwrap_or_default().to_string()
    }

    #[test]
    fn plain_quoted_and_comments() {
        let fm = parse(
            "---\nname: reviewer\ndescription: \"Says \\\"no\\\"\"\nmodel: 'it''s' \n# a comment\ncolor: red # trailing\n---\nbody",
        );
        assert_eq!(s(&fm, "name"), "reviewer");
        assert_eq!(s(&fm, "description"), "Says \"no\"");
        assert_eq!(s(&fm, "model"), "it's");
        assert_eq!(s(&fm, "color"), "red");
        assert_eq!(
            &"---\nname: reviewer\ndescription: \"Says \\\"no\\\"\"\nmodel: 'it''s' \n# a comment\ncolor: red # trailing\n---\nbody"
                [fm.body_start..],
            "body"
        );
    }

    #[test]
    fn lists_in_every_form() {
        let fm = parse("---\ntools: [Read, 'Grep']\nallowed:\n  - Bash\n  - Edit # why\nplain: Read, Write\n---\n");
        assert_eq!(fm.fields["tools"].as_list(), vec!["Read", "Grep"]);
        assert_eq!(fm.fields["allowed"].as_list(), vec!["Bash", "Edit"]);
        assert_eq!(fm.fields["plain"].as_list(), vec!["Read", "Write"]);
    }

    #[test]
    fn block_scalars_keep_blank_lines() {
        let fm = parse("---\ndescription: |\n  First paragraph.\n\n  Second paragraph.\nname: x\n---\n");
        assert_eq!(s(&fm, "description"), "First paragraph.\n\nSecond paragraph.");
        assert_eq!(s(&fm, "name"), "x");
        let folded = parse("---\ndescription: >-\n  one\n  two\n\n  three\n---\n");
        assert_eq!(s(&folded, "description"), "one two\nthree");
    }

    #[test]
    fn continued_plain_value_folds() {
        let fm = parse("---\ndescription: Use when\n  asked to review.\n---\n");
        assert_eq!(s(&fm, "description"), "Use when asked to review.");
    }

    #[test]
    fn nested_maps_are_reported_not_guessed() {
        let fm = parse("---\nhooks:\n  PreToolUse: x\nname: a\n---\n");
        assert_eq!(fm.unsupported, vec!["hooks"]);
        assert_eq!(s(&fm, "name"), "a");
        assert!(!fm.fields.contains_key("hooks"));
    }

    #[test]
    fn no_or_unclosed_front_matter() {
        assert_eq!(parse("# title\nname: x"), FrontMatter::default());
        assert_eq!(parse("---\nname: x\n"), FrontMatter::default());
        let crlf = parse("\u{feff}---\r\nname: x\r\n---\r\nbody");
        assert_eq!(s(&crlf, "name"), "x");
    }
}

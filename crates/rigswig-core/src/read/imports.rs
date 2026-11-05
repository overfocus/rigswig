//! `@path` imports in instruction files (Claude Code's syntax; other tools may adopt it).

/// The `@path` tokens in `text`, skipping fenced code blocks and inline code spans, in order of appearance.
pub fn imports_of(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        // Blank out inline code spans.
        let mut plain = String::with_capacity(line.len());
        let mut in_code = false;
        for c in line.chars() {
            if c == '`' {
                in_code = !in_code;
                plain.push(' ');
            } else {
                plain.push(if in_code { ' ' } else { c });
            }
        }
        let mut prev_space = true;
        let chars: Vec<char> = plain.chars().collect();
        let mut k = 0;
        while k < chars.len() {
            let c = chars[k];
            if c == '@' && prev_space {
                let start = k + 1;
                let mut end = start;
                while end < chars.len() && !chars[end].is_whitespace() {
                    end += 1;
                }
                let raw: String = chars[start..end].iter().collect();
                let tok = raw.trim_end_matches(['.', ',', ';', ':', ')', ']']);
                if looks_like_path(tok) {
                    out.push(tok.to_string());
                }
                k = end;
                prev_space = false;
                continue;
            }
            prev_space = c.is_whitespace();
            k += 1;
        }
    }
    out
}

/// `./x`, `../x`, `~/x`, `/x`, `dir/x`, or `name.ext`: not an @-mention of a person.
fn looks_like_path(t: &str) -> bool {
    if t.is_empty() {
        return false;
    }
    if t.starts_with("./") || t.starts_with("../") || t.starts_with("~/") || t.starts_with('/') {
        return true;
    }
    let word = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if let Some((first, _)) = t.split_once('/') {
        return word(first);
    }
    match t.rsplit_once('.') {
        Some((stem, ext)) => {
            word(stem.trim_start_matches('.')) && !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric())
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_paths_not_mentions_or_code() {
        let text = "See @AGENTS.md and @docs/guide.md, also @./x.md.\nPing @someone or a@b.c\n`@not/this.md`\n```\n@nor/this.md\n```\n@~/notes.md @../up.md";
        assert_eq!(imports_of(text), vec!["AGENTS.md", "docs/guide.md", "./x.md", "~/notes.md", "../up.md"]);
    }
}

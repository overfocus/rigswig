//! What-if: a harness with some items taken out, and its totals recomputed.
//!
//! A pattern names what to take out:
//! - a package id (`formatter@market`): everything the plugin contributes;
//! - `layer:<id>` (`layer:user`): everything from that layer;
//! - `<kind>:<name>`, where the name may use `*` (`agent:*`, `skill:deploy`);
//! - an exact item id.
//!
//! Items the removed ones overrode are not brought back: that would need the adapter's own order, so the what-if says
//! so in `coverage.notModelled` when it matters.

use crate::model::{Harness, LoadWhen, Warning, estimate_tokens};

const KINDS: &[&str] = &["instruction", "agent", "skill", "command", "hook", "mcp", "permission"];

/// `*` matches any run of characters.
pub fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(k) => rest = &rest[k + part.len()..],
                None => return false,
            }
        }
    }
    true
}

fn matches(h: &Harness, pattern: &str, i: &crate::model::Item) -> bool {
    if i.id == pattern {
        return true;
    }
    if let Some(layer) = pattern.strip_prefix("layer:") {
        return i.origin.layer == layer;
    }
    if h.packages.iter().any(|p| p.id == pattern) {
        return i.origin.package.as_deref() == Some(pattern);
    }
    if let Some((kind, name)) = pattern.split_once(':') {
        if KINDS.contains(&kind) {
            return i.detail.kind() == kind && glob(name, &i.name);
        }
    }
    false
}

/// `h` without the items any pattern names. Returns the new harness and, for each pattern, how many effective items it
/// took out (0 means it matched nothing that was in effect).
pub fn without(h: &Harness, patterns: &[String]) -> (Harness, Vec<(String, usize)>) {
    let mut out = h.clone();
    let mut counts = Vec::new();
    let mut removed_ids = Vec::new();
    for p in patterns {
        let mut n = 0;
        for i in out.items.iter_mut() {
            if i.effective && matches(h, p, i) {
                i.effective = false;
                i.load.when = LoadWhen::Never;
                removed_ids.push(i.id.clone());
                n += 1;
            }
        }
        counts.push((p.clone(), n));
    }
    let total: usize = counts.iter().map(|(_, n)| n).sum();
    if total > 0 {
        let chars: u64 = out
            .items
            .iter()
            .filter(|i| i.effective && i.load.when == LoadWhen::Startup)
            .map(|i| i.load.chars.value)
            .sum();
        out.context.chars.value = chars;
        out.context.tokens = estimate_tokens(chars);
        out.warnings.push(Warning {
            code: "what-if".into(),
            message: format!(
                "what-if: {total} item{} taken out ({})",
                if total == 1 { "" } else { "s" },
                patterns.join(", ")
            ),
            item: None,
            file: None,
        });
        if out.items.iter().any(|i| i.shadowed_by.as_ref().is_some_and(|s| removed_ids.contains(s))) {
            out.coverage.not_modelled.push("what-if: items the removed ones overrode are not brought back".into());
        }
    }
    (out, counts)
}

/// `40000`, `40k`, `1.5k`, `1m` as a number of tokens.
pub fn parse_tokens(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let (num, mult) = match s.strip_suffix('k') {
        Some(n) => (n, 1_000.0),
        None => match s.strip_suffix('m') {
            Some(n) => (n, 1_000_000.0),
            None => (s.as_str(), 1.0),
        },
    };
    let v: f64 = num.parse().ok()?;
    (v.is_finite() && v >= 0.0).then(|| (v * mult).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_and_token_sizes() {
        assert!(glob("deploy*", "deploy-prod"));
        assert!(glob("*", "anything"));
        assert!(!glob("deploy", "deploy-prod"));
        assert_eq!(parse_tokens("40k"), Some(40_000));
        assert_eq!(parse_tokens("1.5K"), Some(1_500));
        assert_eq!(parse_tokens("1m"), Some(1_000_000));
        assert_eq!(parse_tokens("1200"), Some(1_200));
        assert_eq!(parse_tokens("lots"), None);
        assert_eq!(parse_tokens("-3"), None);
    }
}

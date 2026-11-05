//! The text form of a harness, for the terminal. Everything printed went through the model's redaction and is
//! stripped of control characters again here.

use std::collections::BTreeMap;
use std::fmt::Write;

use rigswig_core::model::{Harness, ItemDetail, LoadWhen, McpState};
use rigswig_core::read::redact::printable;

const KINDS: &[(&str, &str)] = &[
    ("instruction", "Instructions"),
    ("agent", "Agents"),
    ("skill", "Skills"),
    ("command", "Commands"),
    ("hook", "Hooks"),
    ("mcp", "MCP servers"),
    ("permission", "Permission rules"),
];

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn text(h: &Harness) -> String {
    let mut o = String::new();
    let p = |s: &str| printable(s);
    let _ = writeln!(o, "{} in {}", p(&h.tool.id), p(&h.target.repo));

    // Settings an adapter may or may not report: shown when present, never assumed.
    let set_by = |key: &str| {
        h.settings.get(key).and_then(|d| match (&d.value, &d.source) {
            (serde_json::Value::String(v), Some(src)) => Some(format!("{} (set by {})", p(v), p(src))),
            _ => None,
        })
    };
    let model = set_by("model").unwrap_or_else(|| "the tool's default (nothing sets it)".into());
    let _ = writeln!(o, "  Model     {model}");
    if let Some(provider) = set_by("provider") {
        let _ = writeln!(o, "  Provider  {provider}");
    }
    if let Some(trust) = h.settings.get("projectTrust").and_then(|d| d.value.as_str()) {
        let _ = writeln!(o, "  Project   {} (project config loads only when trusted)", p(trust));
    }

    let c = &h.context;
    let _ = writeln!(
        o,
        "  Context   ~{} tokens before you type ({}..{}, estimated from {} characters)",
        thousands(c.tokens.value),
        thousands(c.tokens.low),
        thousands(c.tokens.high),
        thousands(c.chars.value)
    );
    for n in &c.not_measured {
        let _ = writeln!(o, "            + not measured: {}", p(&n.what));
    }

    // The biggest startup costs.
    let mut big: Vec<_> =
        h.items.iter().filter(|i| i.effective && i.load.when == LoadWhen::Startup && i.load.chars.value > 0).collect();
    big.sort_by(|a, b| b.load.chars.value.cmp(&a.load.chars.value).then(a.id.cmp(&b.id)));
    if !big.is_empty() {
        let _ = writeln!(o, "\n  Largest at startup");
        for i in big.iter().take(5) {
            let share = if c.chars.value > 0 { i.load.chars.value as f64 * 100.0 / c.chars.value as f64 } else { 0.0 };
            let _ =
                writeln!(o, "    {:>5.1}%  {:<11} {}  ({})", share, i.detail.kind(), p(&i.name), p(&i.origin.layer));
        }
    }

    let _ = writeln!(o, "\n  {:<18} {:>8} {:>8}   by layer (in effect)", "", "in effect", "found");
    for (kind, title) in KINDS {
        let all: Vec<_> = h.items.iter().filter(|i| i.detail.kind() == *kind).collect();
        if all.is_empty() {
            continue;
        }
        let on: Vec<_> = all.iter().filter(|i| i.effective).collect();
        let mut by: BTreeMap<u32, (String, usize)> = BTreeMap::new();
        for i in &on {
            let rank = h.layers.iter().find(|l| l.id == i.origin.layer).map_or(u32::MAX, |l| l.rank);
            let label =
                if i.origin.layer.starts_with("plugin:") { "plugins".to_string() } else { i.origin.layer.clone() };
            let key = if label == "plugins" { 1000 } else { rank };
            by.entry(key).or_insert((label, 0)).1 += 1;
        }
        let layers: Vec<String> = by.values().map(|(l, n)| format!("{} {n}", p(l))).collect();
        let _ = writeln!(o, "  {:<18} {:>8} {:>8}   {}", title, on.len(), all.len(), layers.join(", "));
    }
    let pending =
        h.items.iter().filter(|i| matches!(&i.detail, ItemDetail::Mcp(m) if m.state == McpState::Pending)).count();
    let shadowed = h.items.iter().filter(|i| i.shadowed_by.is_some()).count();
    if shadowed > 0 {
        let _ = writeln!(o, "  {shadowed} overridden by a same-named item at a higher layer");
    }
    if pending > 0 {
        let _ = writeln!(o, "  {pending} MCP server(s) waiting for approval");
    }

    let packages: Vec<_> = h.packages.iter().filter(|p| p.enabled).collect();
    if !h.packages.is_empty() {
        let _ = writeln!(o, "\n  Plugins   {} on, {} off", packages.len(), h.packages.len() - packages.len());
    }

    if !h.warnings.is_empty() {
        let _ = writeln!(o, "\n  Warnings");
        for w in &h.warnings {
            let file = w.file.as_deref().map(|f| format!(" ({})", p(f))).unwrap_or_default();
            let _ = writeln!(o, "    ! {}{file}", p(&w.message));
        }
    }
    let _ = writeln!(o, "\n  Not modelled: {} things (see --json coverage.notModelled)", h.coverage.not_modelled.len());
    o
}

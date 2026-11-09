//! The text form of a harness, for the terminal. Everything printed went through the model's redaction and is
//! stripped of control characters again here.

use std::collections::BTreeMap;
use std::fmt::Write;

use rigswig_core::model::{Harness, ItemDetail, LoadWhen, McpState, SessionScope};
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
    if let Some(s) = &h.tool.session {
        let scope = match s.scope {
            SessionScope::Repo => "this repo",
            SessionScope::Machine => "this machine",
        };
        let ev = s.evidence.iter().map(|e| p(e)).collect::<Vec<_>>().join(", ");
        let _ = writeln!(o, "  Session   active on {scope} ({ev}) — this reading follows the live session");
    }
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

/// The text form of a comparison.
pub fn compare_text(c: &rigswig_core::compare::Comparison) -> String {
    let mut o = String::new();
    let p = |s: &str| printable(s);
    let label = |s: &rigswig_core::compare::Side| format!("{} in {}", p(&s.tool), p(&s.repo));
    let (la, lb) = (label(&c.a), label(&c.b));
    let _ = writeln!(o, "A  {la}\nB  {lb}\n");
    let delta = c.b.tokens as i64 - c.a.tokens as i64;
    let _ = writeln!(
        o,
        "  Context   A ~{}  B ~{}  tokens before you type  ({})",
        thousands(c.a.tokens),
        thousands(c.b.tokens),
        match delta {
            0 => "the same".to_string(),
            d if d > 0 => format!("B has {} more", thousands(d.unsigned_abs())),
            d => format!("B has {} fewer", thousands(d.unsigned_abs())),
        }
    );

    let _ = writeln!(o, "\n  {:<18} {:>7} {:>7} {:>7} {:>7}", "", "only A", "only B", "changed", "same");
    for (kind, title) in KINDS {
        if let Some(k) = c.kinds.get(*kind) {
            let _ = writeln!(o, "  {:<18} {:>7} {:>7} {:>7} {:>7}", title, k.only_a, k.only_b, k.changed, k.same);
        }
    }

    let list = |o: &mut String, title: &str, items: &[rigswig_core::compare::Entry]| {
        if items.is_empty() {
            return;
        }
        let _ = writeln!(o, "\n  {title} ({})", items.len());
        for e in items.iter().take(10) {
            let size = if e.chars > 0 { format!("~{} tokens", thousands(e.chars.div_ceil(4))) } else { String::new() };
            let _ = writeln!(o, "    {:<11} {}  ({})  {size}", e.kind, p(&e.name), p(&e.layer));
        }
        if items.len() > 10 {
            let _ = writeln!(o, "    ... and {} more (--json for all)", items.len() - 10);
        }
    };
    list(&mut o, "Only in A", &c.only_a);
    list(&mut o, "Only in B", &c.only_b);
    if !c.changed.is_empty() {
        let _ = writeln!(o, "\n  Changed ({})", c.changed.len());
        for x in c.changed.iter().take(10) {
            let _ = writeln!(
                o,
                "    {:<11} {}  A ~{} / B ~{} tokens",
                x.kind,
                p(&x.name),
                thousands(x.a.chars.div_ceil(4)),
                thousands(x.b.chars.div_ceil(4))
            );
        }
    }
    if !c.settings.is_empty() {
        let _ = writeln!(o, "\n  Settings that differ");
        let v = |x: &serde_json::Value| match x {
            serde_json::Value::Null => "(not set)".to_string(),
            serde_json::Value::String(s) => p(s),
            other => p(&other.to_string()),
        };
        for s in &c.settings {
            let _ = writeln!(o, "    {:<24} A {}  B {}", p(&s.name), v(&s.a), v(&s.b));
        }
    }
    o
}

/// The text form of an audit.
pub fn audit_text(a: &rigswig_core::audit::Audit) -> String {
    use rigswig_core::audit::Severity;
    let mut o = String::new();
    let p = |s: &str| printable(s);
    let _ = writeln!(
        o,
        "rigswig audit: {} in {}  (~{} tokens before you type)",
        p(&a.tool),
        p(&a.repo),
        thousands(a.tokens)
    );
    if let Some(u) = &a.usage {
        let _ = writeln!(
            o,
            "  with usage from {} session{} in the logs{} (metadata only; `rigswig usage` for the counts)",
            u.sessions,
            if u.sessions == 1 { "" } else { "s" },
            u.since.as_deref().map_or_else(String::new, |s| format!(" since {}", p(s)))
        );
    }
    if a.findings.is_empty() {
        let _ = writeln!(o, "\n  No findings.");
    }
    let total: u64 = a.findings.iter().filter_map(|f| f.saving.as_ref()).map(|s| s.tokens).sum();
    for (n, f) in a.findings.iter().enumerate() {
        let sev = match f.severity {
            Severity::High => "HIGH",
            Severity::Medium => "MED ",
            Severity::Low => "LOW ",
        };
        let _ = writeln!(o, "\n  {:>2}. {sev}  {}", n + 1, p(&f.title));
        let _ = writeln!(o, "            [{}]", p(&f.code));
        for e in &f.evidence {
            let _ = writeln!(o, "            - {}", p(e));
        }
        if let Some(s) = &f.saving {
            let _ = writeln!(
                o,
                "            saves ~{} tokens a session (check with: rigswig status --without {})",
                thousands(s.tokens),
                p(&if s.without.len() <= 3 {
                    s.without.join(",")
                } else {
                    format!("<{} items, see --json>", s.without.len())
                })
            );
        }
        let _ = writeln!(o, "            fix: {}", p(&f.proposal));
    }
    if total > 0 {
        let _ = writeln!(
            o,
            "\n  Together the findings with a saving take ~{} tokens out of each session (estimates).",
            thousands(total)
        );
    }
    for note in &a.notes {
        let _ = writeln!(o, "  note: {}", p(note));
    }
    let _ = writeln!(o, "  Findings come from files only: \"nothing references it\" never means \"unused\".");
    o
}

/// The text form of fleet mode.
pub fn fleet_text(f: &rigswig_core::audit::Fleet) -> String {
    use rigswig_core::audit::Recommendation;
    let mut o = String::new();
    let p = |s: &str| printable(s);
    let _ = writeln!(o, "rigswig audit: {} repos\n", f.repos.len());
    for r in &f.repos {
        let _ = writeln!(
            o,
            "  {:<28} {:<12} ~{:>6} tokens  {} findings",
            p(&r.repo),
            p(&r.tool),
            thousands(r.tokens),
            r.findings
        );
    }
    for (title, which) in [
        ("Archive (nothing in any repo references it)", Recommendation::Archive),
        ("Move to project scope", Recommendation::MoveToProject),
        ("No signal to judge by", Recommendation::Unknown),
        ("Keep at user scope", Recommendation::Keep),
    ] {
        let rows: Vec<_> = f.items.iter().filter(|i| i.recommendation == which).collect();
        if rows.is_empty() {
            continue;
        }
        let sum: u64 = rows.iter().map(|i| i.tokens).sum();
        let _ = writeln!(o, "\n  {title}: {} (~{} tokens a session)", rows.len(), thousands(sum));
        let limit = if which == Recommendation::Keep { 8 } else { 25 };
        for i in rows.iter().take(limit) {
            let r#where = if which == Recommendation::MoveToProject {
                format!("  -> {}", p(&i.needed_in.join(", ")))
            } else {
                String::new()
            };
            let _ = writeln!(
                o,
                "    {:<8} {:<40} ~{:>5}  {}{}",
                p(&i.kind),
                p(&i.name.chars().take(40).collect::<String>()),
                thousands(i.tokens),
                p(&i.recommendation.sentence(i)),
                r#where
            );
        }
        if rows.len() > limit {
            let _ = writeln!(o, "    ... and {} more (--json for all)", rows.len() - limit);
        }
    }
    let _ = writeln!(o);
    for n in &f.notes {
        let _ = writeln!(o, "  note: {}", p(n));
    }
    o
}

/// `rigswig usage` as text.
pub fn usage_text(u: &rigswig_core::usage::Usage) -> String {
    let mut o = String::new();
    let p = |s: &str| printable(s);
    let day = |s: &str| p(s.get(..10).unwrap_or(s));
    let _ = writeln!(o, "rigswig usage: {} in {}", p(&u.tool), p(&u.repo));
    let window = match (&u.since, &u.first, &u.last) {
        (Some(s), _, Some(l)) => format!("since {}, last {}", day(s), day(l)),
        (None, Some(f), Some(l)) => format!("{} to {}", day(f), day(l)),
        (Some(s), _, None) => format!("since {}", day(s)),
        _ => "every log there is".into(),
    };
    let _ = writeln!(
        o,
        "  {} session{} ({window}), from {} log file{}",
        u.sessions,
        if u.sessions == 1 { "" } else { "s" },
        u.files,
        if u.files == 1 { "" } else { "s" }
    );
    if u.sessions == 0 {
        let _ = writeln!(o, "\n  No sessions in the window.");
    } else {
        let t = &u.tokens;
        let _ = writeln!(
            o,
            "\n  Tokens   input {}  output {}  cache read {}  cache write {}",
            thousands(t.input),
            thousands(t.output),
            thousands(t.cache_read),
            thousands(t.cache_write)
        );
        if !u.models.is_empty() {
            let mut models: Vec<_> = u.models.iter().collect();
            models.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            let list: Vec<String> = models.iter().map(|(m, n)| format!("{} ({})", p(m), thousands(**n))).collect();
            let _ = writeln!(o, "  Models   {} responses: {}", thousands(u.models.values().sum()), list.join(", "));
        }
        for (kind, title) in [("agent", "Agents"), ("skill", "Skills"), ("mcp", "MCP servers")] {
            let rows: Vec<_> = u.invoked.iter().filter(|i| i.kind == kind).collect();
            let _ = writeln!(o, "\n  {title} invoked");
            if rows.is_empty() {
                let _ = writeln!(o, "    none");
            }
            for i in rows {
                let _ = writeln!(
                    o,
                    "    {:<40} {:>6} call{}  in {} session{}",
                    p(&i.name),
                    thousands(i.calls),
                    if i.calls == 1 { " " } else { "s" },
                    i.sessions,
                    if i.sessions == 1 { "" } else { "s" }
                );
            }
        }
    }
    if u.files_unread > 0 {
        let _ = writeln!(
            o,
            "\n  {} log file{} could not be read, so {} not counted",
            u.files_unread,
            if u.files_unread == 1 { "" } else { "s" },
            if u.files_unread == 1 { "it is" } else { "they are" }
        );
    }
    if u.skipped_lines > 0 {
        let _ = writeln!(
            o,
            "\n  {} line{} skipped: not in a shape this reads, or with no timestamp to place in the window",
            thousands(u.skipped_lines),
            if u.skipped_lines == 1 { " was" } else { "s were" }
        );
    }
    let _ = writeln!(o, "\n  Not counted:");
    for n in &u.not_counted {
        let _ = writeln!(o, "    - {}", p(n));
    }
    let _ = writeln!(
        o,
        "\n  `rigswig audit --usage` turns these counts into findings: what loads in every session and is never invoked."
    );
    o
}

//! `rigswig audit`: ranked findings about a harness, each with its evidence, an estimated saving where one can be
//! computed (with [`crate::whatif::without`]) and a proposed change. Everything is derived from files, offline.
//!
//! By default findings never say an item is "unused": without session logs rigswig cannot see usage, and it reads
//! them only when a run asks (`audit --usage`). They say what is true of the files: "nothing in the repo references
//! this". With [`Options::usage`] they also say what the logs show: an item invoked in the window counts as needed
//! here, and `not-invoked` names what loads but no session called — still never "unused", since a skill typed as
//! `/name` is not counted ([`crate::usage`]).
//!
//! The audit reads through the same injected, read-only file system as the adapters (instruction files, the repo's
//! files for references and signals, and the user's optional signal table), and prints nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::adapter::Ctx;
use crate::fs::EntryKind;
use crate::model::{Harness, Item, ItemDetail, LayerKind, LoadWhen, PermissionEffect};
use crate::read::redact::printable;
use crate::usage::{MIN_SESSIONS, Usage};
use crate::whatif::{glob, without};

/// Claude Code's documented aim for one instruction file: under 200 lines
/// (<https://code.claude.com/docs/en/memory.md>, "Write effective instructions").
pub const CLAUDE_MD_LINES: usize = 200;

/// The audit of one harness in one repo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Audit {
    pub tool: String,
    pub repo: String,
    /// The startup estimate the savings are measured against.
    pub tokens: u64,
    /// Most severe first, then by saving.
    pub findings: Vec<Finding>,
    /// Every user-scope agent, skill, command and plugin, and whether this repo's files say it is needed here. Fleet
    /// mode is built from these.
    pub user_scope: Vec<UserScoped>,
    /// What the audit could not judge.
    pub notes: Vec<String>,
    /// The session-log counts the findings used, when the run asked for them (`--usage`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// A stable code: `heavy-user-scope`, `plugin-fit`, `instruction-budget`, `repeated-text`, `dead-reference`,
    /// `permission-one-off`, `permission-covered`, `permission-broad`, `not-invoked` (only with `--usage`),
    /// `warning:<code>`.
    pub code: String,
    pub severity: Severity,
    pub title: String,
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saving: Option<Saving>,
    pub proposal: String,
    /// The items it is about.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    High,
    Medium,
    Low,
}

/// Estimated tokens from startup files that the proposed change would save: an estimate, computed by taking `without`
/// out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Saving {
    pub tokens: u64,
    /// The `--without` patterns that reproduce it.
    pub without: Vec<String>,
}

/// A user-scope item and what this repo says about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserScoped {
    /// An item id, or a package id for a plugin.
    pub id: String,
    /// `agent`, `skill`, `command` or `plugin`.
    pub kind: String,
    pub name: String,
    /// Tokens it adds to every session (for a plugin, everything it contributes at startup).
    pub tokens: u64,
    /// True when the repo references it by name (or matches the plugin's signals), false when nothing does, absent
    /// when the audit cannot tell (a plugin with no signal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needed: Option<bool>,
}

/// A plugin and the files that show a repo needs it. A plugin whose name matches `plugin` (a glob, `*` for any run)
/// fits a repo with any of `needs`: a path (`Cargo.toml`), a folder (`supabase/`) or a file pattern (`*.tf`, looked
/// for at the top and one folder down).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub plugin: Vec<String>,
    pub needs: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Signals that add to (and, for the same plugin pattern, replace) the built-in table.
    pub signals: Vec<Signal>,
    /// Session-log counts for this repo ([`crate::adapter::Adapter::usage`]), when the run asked for them.
    pub usage: Option<Usage>,
}

/// The built-in plugin signals: language servers by their project files, platforms by their config.
pub fn builtin_signals() -> Vec<Signal> {
    let s = |plugin: &[&str], needs: &[&str]| Signal {
        plugin: plugin.iter().map(|x| (*x).into()).collect(),
        needs: needs.iter().map(|x| (*x).into()).collect(),
    };
    vec![
        s(&["rust-analyzer*", "rust-*", "*-rust", "*-rust-*"], &["Cargo.toml", "*.rs"]),
        s(&["typescript*", "*-typescript*", "ts-lsp*", "vtsls*"], &["tsconfig.json", "jsconfig.json", "package.json"]),
        s(
            &["pyright*", "python*", "*-python*", "pylsp*", "ruff*"],
            &["pyproject.toml", "setup.py", "requirements.txt", "*.py"],
        ),
        s(&["gopls*", "go-*", "*-go"], &["go.mod", "*.go"]),
        s(&["clangd*", "*-cpp*", "c-lsp*"], &["CMakeLists.txt", "compile_commands.json", "*.c", "*.cpp", "*.h"]),
        s(&["swift*", "sourcekit*", "*-swift*"], &["Package.swift", "*.xcodeproj", "*.swift"]),
        s(&["jdtls*", "java-*", "*-java*"], &["pom.xml", "build.gradle", "build.gradle.kts", "*.java"]),
        s(&["kotlin*", "*-kotlin*"], &["build.gradle.kts", "*.kt"]),
        s(&["csharp*", "omnisharp*", "*-csharp*", "dotnet*"], &["*.csproj", "*.sln", "*.cs"]),
        s(&["php*", "intelephense*", "*-php*"], &["composer.json", "*.php"]),
        s(&["lua*", "*-lua*"], &["*.lua"]),
        s(&["ruby*", "solargraph*", "*-ruby*"], &["Gemfile", "*.rb"]),
        s(&["vercel*"], &["vercel.json", ".vercel/"]),
        s(&["supabase*"], &["supabase/"]),
        s(&["firebase*"], &["firebase.json", ".firebaserc"]),
        s(&["netlify*"], &["netlify.toml", ".netlify/"]),
        s(&["cloudflare*", "wrangler*"], &["wrangler.toml", "wrangler.json", "wrangler.jsonc"]),
        s(&["terraform*", "*-terraform*"], &["*.tf"]),
        s(&["prisma*"], &["prisma/", "schema.prisma"]),
    ]
}

/// The user's own signal table, `~/.rigswig/signals.json`: `{ "signals": [ { "plugin": [...], "needs": [...] } ] }`.
/// Returns the signals and, when the file is there but unreadable, a note.
pub fn user_signals(ctx: &Ctx) -> (Vec<Signal>, Option<String>) {
    #[derive(Deserialize)]
    struct File {
        signals: Vec<Signal>,
    }
    let path = ctx.home.join(".rigswig/signals.json");
    match ctx.fs.read_to_string(&path) {
        Err(_) => (Vec::new(), None),
        Ok(text) => match serde_json::from_str::<File>(&text) {
            Ok(f) => (f.signals, None),
            Err(_) => (Vec::new(), Some("~/.rigswig/signals.json is not a valid signal table, ignored".into())),
        },
    }
}

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "vendor",
    "coverage",
    "__pycache__",
    "venv",
    "Pods",
    "DerivedData",
];
/// Dot folders whose files can reference agents, skills and commands.
const DOT_DIRS: &[&str] = &[".claude", ".codex", ".agents", ".github", ".cursor"];
const TEXT_EXTS: &[&str] = &[
    "md", "mdx", "txt", "json", "jsonc", "toml", "yaml", "yml", "js", "mjs", "cjs", "ts", "tsx", "jsx", "sh", "py",
    "rs", "go", "rb",
];
const MAX_FILES: usize = 5_000;
const MAX_FILE_BYTES: usize = 256 * 1024;
const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;

/// The repo's text, lower-cased, for "does anything reference this name": a bounded walk that skips build output and
/// dependencies. Returns the text and whether the walk stopped at a bound.
fn corpus(ctx: &Ctx, tick: &mut dyn FnMut(usize)) -> (String, bool) {
    struct Walk<'a, 't> {
        ctx: &'a Ctx<'a>,
        tick: &'t mut dyn FnMut(usize),
        files: usize,
        bytes: usize,
        cut: bool,
        out: String,
    }
    fn go(w: &mut Walk, dir: &Path, depth: usize) {
        if depth > 10 {
            return;
        }
        for e in w.ctx.fs.read_dir(dir).unwrap_or_default() {
            if w.files >= MAX_FILES || w.bytes >= MAX_TOTAL_BYTES {
                w.cut = true;
                return;
            }
            let p = dir.join(&e.name);
            match e.kind {
                EntryKind::Dir => {
                    let hidden = e.name.starts_with('.') && !DOT_DIRS.contains(&e.name.as_str());
                    if !hidden && !SKIP_DIRS.contains(&e.name.as_str()) {
                        go(w, &p, depth + 1);
                    }
                }
                EntryKind::File => {
                    let ext = e.name.rsplit_once('.').map(|(_, x)| x).unwrap_or("");
                    if !TEXT_EXTS.contains(&ext) {
                        continue;
                    }
                    let Ok(text) = w.ctx.fs.read_to_string(&p) else { continue };
                    if text.len() > MAX_FILE_BYTES {
                        continue;
                    }
                    w.files += 1;
                    w.bytes += text.len();
                    if w.files % 100 == 0 {
                        (w.tick)(w.files);
                    }
                    w.out.push_str(&text.to_lowercase());
                    w.out.push('\n');
                }
                EntryKind::Symlink => {}
            }
        }
    }
    let mut w = Walk { ctx, tick, files: 0, bytes: 0, cut: false, out: String::new() };
    go(&mut w, &ctx.repo, 0);
    (w.tick)(w.files);
    (w.out, w.cut)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// `needle` (lower case) appears in `hay` with no word character on either side.
fn mentions(hay: &str, needle: &str) -> bool {
    if needle.chars().count() < 3 {
        return false;
    }
    let mut from = 0;
    while let Some(k) = hay[from..].find(needle) {
        let at = from + k;
        let end = at + needle.len();
        let before = hay[..at].chars().next_back();
        let after = hay[end..].chars().next();
        if !before.is_some_and(is_word) && !after.is_some_and(is_word) {
            return true;
        }
        from = at + needle.chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// The names an item can be referenced by: its name, the name as a slug, and its file's stem (for a skill, its
/// folder).
fn names_of(i: &Item) -> Vec<String> {
    let mut out = vec![i.name.to_lowercase()];
    let slug: String = i.name.to_lowercase().split_whitespace().collect::<Vec<_>>().join("-");
    out.push(slug);
    if let Some(f) = &i.origin.file {
        let p = Path::new(f);
        let stem = if p.file_name().is_some_and(|n| n == "SKILL.md") {
            p.parent().and_then(Path::file_name)
        } else {
            p.file_stem()
        };
        if let Some(s) = stem {
            out.push(s.to_string_lossy().to_lowercase());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// A shown path (`~/...`, repo-relative or absolute) back to a real one.
fn real_path(ctx: &Ctx, shown: &str) -> PathBuf {
    if let Some(rest) = shown.strip_prefix("~/") {
        ctx.home.join(rest)
    } else if Path::new(shown).has_root() {
        PathBuf::from(shown)
    } else if shown == "." {
        ctx.repo.clone()
    } else {
        ctx.repo.join(shown)
    }
}

fn tokens(chars: u64) -> u64 {
    chars.div_ceil(4)
}

fn layer_kind(h: &Harness, layer: &str) -> Option<LayerKind> {
    h.layers.iter().find(|l| l.id == layer).map(|l| l.kind)
}

fn startup(i: &Item) -> bool {
    i.effective && i.load.when == LoadWhen::Startup
}

fn saving(h: &Harness, patterns: Vec<String>) -> Option<Saving> {
    if patterns.is_empty() {
        return None;
    }
    let (w, _) = without(h, &patterns);
    let t = h.context.tokens.value.saturating_sub(w.context.tokens.value);
    (t > 0).then_some(Saving { tokens: t, without: patterns })
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn short(s: &str, max: usize) -> String {
    let s = printable(s);
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max { s } else { format!("{}...", s.chars().take(max).collect::<String>()) }
}

/// Audit `h`, which `ctx` resolved.
pub fn audit(ctx: &Ctx, h: &Harness, opts: &Options) -> Audit {
    audit_with(ctx, h, opts, &mut |_| {})
}

/// Where an audit has got to: what it is doing, and how far through it is (0 to 1).
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub stage: String,
    pub fraction: f64,
}

/// [`audit`], telling `progress` what it is doing as it goes. Reading the repo's text is most of the time; its share
/// of the bar grows with each hundred files read, towards (never past) the next stage.
pub fn audit_with(ctx: &Ctx, h: &Harness, opts: &Options, progress: &mut dyn FnMut(Progress)) -> Audit {
    let mut say = |stage: String, fraction: f64| progress(Progress { stage, fraction });
    let mut findings = Vec::new();
    let mut notes = Vec::new();
    say("Reading the repo's files".into(), 0.0);
    let (text, cut) = corpus(ctx, &mut |files| {
        let f = 0.7 * (1.0 - (-(files as f64) / 1500.0).exp());
        say(format!("Reading the repo's files ({files} read)"), f);
    });
    say("Checking user-scope agents, skills, commands and plugins".into(), 0.72);
    if cut {
        notes.push(format!(
            "the repo is larger than the reference search reads ({MAX_FILES} files or {} MB); names only beyond it count as unreferenced",
            MAX_TOTAL_BYTES / (1024 * 1024)
        ));
    }

    let user_scope = heavy_user_scope(ctx, h, &text, opts, &mut findings, &mut notes);
    if let Some(u) = &opts.usage {
        not_invoked(h, u, &mut findings, &mut notes);
    }
    say("Checking instruction files".into(), 0.85);
    instruction_files(ctx, h, &mut findings, &mut notes);
    if h.tool.id == "claude-code" {
        agents_md_not_imported(ctx, h, &mut findings);
    }
    say("Checking permission rules".into(), 0.94);
    permissions(h, &mut findings);
    warnings(h, &mut findings);
    say("Ranking findings".into(), 0.99);

    findings.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then(b.saving.as_ref().map_or(0, |s| s.tokens).cmp(&a.saving.as_ref().map_or(0, |s| s.tokens)))
            .then(a.code.cmp(&b.code))
            .then(a.title.cmp(&b.title))
    });
    Audit {
        tool: h.tool.id.clone(),
        repo: h.target.repo.clone(),
        tokens: h.context.tokens.value,
        findings,
        user_scope,
        notes,
        usage: opts.usage.clone(),
    }
}

/// User-scope agents, skills and commands nothing in the repo references, and user-scope plugins the repo's files do
/// not call for.
fn heavy_user_scope(
    ctx: &Ctx,
    h: &Harness,
    text: &str,
    opts: &Options,
    findings: &mut Vec<Finding>,
    notes: &mut Vec<String>,
) -> Vec<UserScoped> {
    let mut rows = Vec::new();
    let user = |i: &Item| layer_kind(h, &i.origin.layer) == Some(LayerKind::User) && i.origin.package.is_none();
    for (kind, title, home) in [
        ("agent", "agents", ".claude/agents/"),
        ("skill", "skills", ".claude/skills/"),
        ("command", "commands", "commands/"),
    ] {
        let all: Vec<&Item> = h.items.iter().filter(|i| startup(i) && i.detail.kind() == kind && user(i)).collect();
        if all.is_empty() {
            continue;
        }
        let mut unref = Vec::new();
        let mut referenced = Vec::new();
        for i in &all {
            let needed =
                names_of(i).iter().any(|n| mentions(text, n)) || opts.usage.as_ref().is_some_and(|u| invoked(u, i));
            rows.push(UserScoped {
                id: i.id.clone(),
                kind: kind.into(),
                name: i.name.clone(),
                tokens: tokens(i.load.chars.value),
                needed: Some(needed),
            });
            if needed { referenced.push(*i) } else { unref.push(*i) }
        }
        let unref_tokens: u64 = unref.iter().map(|i| tokens(i.load.chars.value)).sum();
        if unref.len() < 3 || unref_tokens < 500 {
            continue;
        }
        let all_tokens: u64 = all.iter().map(|i| tokens(i.load.chars.value)).sum();
        unref.sort_by(|a, b| b.load.chars.value.cmp(&a.load.chars.value).then(a.name.cmp(&b.name)));
        let mut evidence: Vec<String> = unref
            .iter()
            .take(8)
            .map(|i| format!("{} (~{} tokens)", short(&i.name, 60), tokens(i.load.chars.value)))
            .collect();
        if unref.len() > 8 {
            evidence.push(format!("... and {} more", unref.len() - 8));
        }
        if !referenced.is_empty() {
            let names: Vec<String> = referenced.iter().take(8).map(|i| short(&i.name, 40)).collect();
            let how = if opts.usage.is_some() { "referenced or invoked here" } else { "referenced here" };
            evidence.push(format!("{how}: {}", names.join(", ")));
        }
        if let Some(u) = opts.usage.as_ref().filter(|u| u.sessions > 0) {
            evidence.push(format!("and none of the others was invoked in {}", window(u)));
        }
        let ids: Vec<String> = unref.iter().map(|i| i.id.clone()).collect();
        let s = saving(h, ids.clone());
        findings.push(Finding {
            code: "heavy-user-scope".into(),
            severity: if s.as_ref().is_some_and(|s| s.tokens >= 2_000) { Severity::High } else { Severity::Medium },
            title: format!(
                "{} load in every session (~{all_tokens} tokens); nothing in the repo references {} (~{unref_tokens} tokens)",
                plural(all.len(), &format!("user-scope {}", &title[..title.len() - 1]), &format!("user-scope {title}")),
                if unref.len() == all.len() { "any of them".to_string() } else { format!("{} of them", unref.len()) }
            ),
            evidence,
            saving: s,
            proposal: format!(
                "Archive the ones you no longer use, and move the rest into the {home} folder of the repos that use them. `rigswig audit --repos` shows which repos reference each."
            ),
            items: ids,
            file: None,
        });
    }

    // Plugins switched on at user scope, against the repo's signals.
    let mut table = builtin_signals();
    for s in &opts.signals {
        table.retain(|b| b.plugin != s.plugin);
    }
    table.splice(0..0, opts.signals.iter().cloned());
    let mut unsignalled = 0;
    for p in h.packages.iter().filter(|p| p.enabled) {
        let decided_user = p.decided_by.as_deref().is_some_and(|d| layer_kind(h, d) == Some(LayerKind::User));
        if !decided_user {
            continue;
        }
        let contributed: Vec<&Item> = h.items.iter().filter(|i| p.contributes.contains(&i.id)).collect();
        let cost: u64 = contributed.iter().filter(|i| startup(i)).map(|i| tokens(i.load.chars.value)).sum();
        let name = p.id.split('@').next().unwrap_or(&p.id).to_lowercase();
        let signal = table.iter().find(|s| s.plugin.iter().any(|g| glob(&g.to_lowercase(), &name)));
        let needed = signal.map(|s| s.needs.iter().any(|n| has(ctx, n)));
        rows.push(UserScoped { id: p.id.clone(), kind: "plugin".into(), name: p.id.clone(), tokens: cost, needed });
        let Some(sig) = signal else {
            unsignalled += 1;
            continue;
        };
        if needed == Some(true) {
            continue;
        }
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for i in &contributed {
            *counts.entry(i.detail.kind()).or_default() += 1;
        }
        let adds: Vec<String> =
            counts.iter().map(|(k, n)| format!("{n} {k}{}", if *n == 1 { "" } else { "s" })).collect();
        let mut evidence = vec![format!("the repo has none of: {}", sig.needs.join(", "))];
        if !adds.is_empty() {
            evidence.push(format!("it adds {}", adds.join(", ")));
        }
        let s = saving(h, vec![p.id.clone()]);
        findings.push(Finding {
            code: "plugin-fit".into(),
            severity: if s.as_ref().is_some_and(|s| s.tokens >= 2_000) || counts.contains_key("hook") {
                Severity::High
            } else {
                Severity::Medium
            },
            title: format!("plugin {} is on for every repo, and this repo has none of its files", printable(&p.id)),
            evidence,
            saving: s,
            proposal: format!(
                "Turn it off in user settings and on in the project settings of the repos that need it (\"enabledPlugins\": {{\"{}\": true}} in .claude/settings.json).",
                printable(&p.id)
            ),
            items: vec![p.id.clone()],
            file: None,
        });
    }
    if unsignalled > 0 {
        notes.push(format!(
            "{} on at user scope with no signal to judge {} by; add one to ~/.rigswig/signals.json",
            plural(unsignalled, "plugin is", "plugins are"),
            if unsignalled == 1 { "it" } else { "them" }
        ));
    }
    rows
}

/// Whether the session logs show `i` invoked: an agent by its type, a skill by its name, an MCP server by any of its
/// tools. A plugin's `plugin:name` also counts when called by its bare `name` (sessions do call them that way): the
/// claim this guards is "never invoked", so a match that might be another item's errs towards invoked.
fn invoked(u: &Usage, i: &Item) -> bool {
    match i.detail.kind() {
        "mcp" => u.mcp_calls(&i.name) > 0 || u.find("mcp", &i.name).is_some(),
        k @ ("agent" | "skill") => {
            u.find(k, &i.name).is_some() || i.name.rsplit_once(':').is_some_and(|(_, bare)| u.find(k, bare).is_some())
        }
        _ => false,
    }
}

/// "23 sessions since 2026-10-08", for evidence.
fn window(u: &Usage) -> String {
    let n = plural(u.sessions as usize, "session", "sessions");
    match u.since.as_deref().or(u.first.as_deref()) {
        Some(d) => format!("{n} since {}", d.get(..10).unwrap_or(d)),
        None => n,
    }
}

/// Agents, skills and MCP servers that load in every session and that no session in the window invoked. Items an
/// earlier finding already names are left to it.
fn not_invoked(h: &Harness, u: &Usage, findings: &mut Vec<Finding>, notes: &mut Vec<String>) {
    if u.sessions < MIN_SESSIONS {
        notes.push(format!(
            "usage: {} in the logs, fewer than {MIN_SESSIONS}; too few to say what is never invoked",
            plural(u.sessions as usize, "session", "sessions")
        ));
        return;
    }
    let named: BTreeSet<String> = findings.iter().flat_map(|f| f.items.iter().cloned()).collect();
    for (kind, title) in [("agent", "agents"), ("skill", "skills"), ("mcp", "MCP servers")] {
        let mut idle: Vec<&Item> = h
            .items
            .iter()
            .filter(|i| i.effective && i.detail.kind() == kind && !named.contains(&i.id))
            .filter(|i| kind == "mcp" || i.load.when == LoadWhen::Startup)
            .filter(|i| !invoked(u, i))
            .collect();
        if idle.is_empty() {
            continue;
        }
        idle.sort_by(|a, b| b.load.chars.value.cmp(&a.load.chars.value).then(a.name.cmp(&b.name)));
        let cost: u64 = idle.iter().map(|i| tokens(i.load.chars.value)).sum();
        let mut evidence: Vec<String> = idle
            .iter()
            .take(8)
            .map(|i| {
                let size = if i.load.chars.value > 0 {
                    format!("~{} tokens", tokens(i.load.chars.value))
                } else {
                    "tool listing not measured".into()
                };
                format!("{} ({size}, {})", short(&i.name, 60), i.origin.layer)
            })
            .collect();
        if idle.len() > 8 {
            evidence.push(format!("... and {} more", idle.len() - 8));
        }
        evidence.push(format!("invoked in none of {} in this repo", window(u)));
        if kind == "skill" {
            evidence.push("a skill you only ever type as /name is not counted: check these before archiving".into());
        }
        let ids: Vec<String> = idle.iter().map(|i| i.id.clone()).collect();
        let s = saving(h, ids.clone());
        // A skill typed as /name is not counted, so a skill finding is never High.
        let big = s.as_ref().is_some_and(|s| s.tokens >= 2_000);
        let severity = match (big, kind) {
            (true, "skill") => Severity::Medium,
            (true, _) => Severity::High,
            (false, "skill") => Severity::Low,
            (false, _) => Severity::Medium,
        };
        findings.push(Finding {
            code: "not-invoked".into(),
            severity,
            title: format!(
                "{} {} in every session and none was invoked in {}{}",
                plural(idle.len(), &title[..title.len() - 1], title),
                if idle.len() == 1 { "loads" } else { "load" },
                window(u),
                if cost > 0 { format!(" (~{cost} tokens)") } else { String::new() }
            ),
            evidence,
            saving: s,
            proposal: match kind {
                "mcp" => "Turn off the servers this repo does not call (`rigswig ui --allow-writes` can, for project servers), or move them to the repos that do.".into(),
                _ => "Archive the ones you no longer use, or move them to the scope of the repos that invoke them. `rigswig usage` lists what each session here did invoke.".into(),
            },
            items: ids,
            file: None,
        });
    }
}

/// Whether the repo has `need`: a path, a folder (`x/`), or a file pattern (`*.tf`) at the top or one folder down.
fn has(ctx: &Ctx, need: &str) -> bool {
    if !need.contains('*') {
        let p = ctx.repo.join(need.trim_end_matches('/'));
        return if need.ends_with('/') {
            ctx.fs.kind(&p).is_ok_and(|k| k != EntryKind::File)
        } else {
            ctx.fs.exists(&p)
        };
    }
    let entries = ctx.fs.read_dir(&ctx.repo).unwrap_or_default();
    if entries.iter().any(|e| e.kind != EntryKind::Dir && glob(need, &e.name)) {
        return true;
    }
    entries
        .iter()
        .filter(|e| e.kind == EntryKind::Dir && !e.name.starts_with('.') && !SKIP_DIRS.contains(&e.name.as_str()))
        .any(|d| {
            ctx.fs
                .read_dir(&ctx.repo.join(&d.name))
                .unwrap_or_default()
                .iter()
                .any(|e| e.kind != EntryKind::Dir && glob(need, &e.name))
        })
}

struct Doc {
    item: Item,
    path: PathBuf,
    text: String,
}

fn is_memory_index(i: &Item) -> bool {
    i.name.ends_with("/memory/MEMORY.md")
}

/// Budgets, repeated text and dead references in the instruction files that load at startup.
fn instruction_files(ctx: &Ctx, h: &Harness, findings: &mut Vec<Finding>, notes: &mut Vec<String>) {
    let repo_real = ctx.fs.canonicalize(&ctx.repo).unwrap_or_else(|_| ctx.repo.clone());
    let docs: Vec<Doc> = h
        .items
        .iter()
        .filter(|i| startup(i) && matches!(i.detail, ItemDetail::Instruction(_)))
        .filter_map(|i| {
            let shown = i.origin.file.as_deref()?;
            let path = real_path(ctx, shown);
            // A file the repo holds (shown repo-relative) that is a link out of the repo: the harness loads it, so its
            // size counts, but an audit never quotes text from outside the repo it was pointed at. An untrusted clone
            // could link its CLAUDE.md to any file on this machine.
            let in_repo = !shown.starts_with("~/") && !Path::new(shown).has_root();
            if in_repo && !ctx.fs.canonicalize(&path).is_ok_and(|r| r.starts_with(&repo_real)) {
                notes.push(format!(
                    "{} links to a file outside the repo; it is counted but its text is not quoted or checked",
                    printable(shown)
                ));
                return None;
            }
            let text = ctx.fs.read_to_string(&path).ok()?;
            Some(Doc { item: i.clone(), path, text })
        })
        .collect();

    if h.tool.id == "claude-code" {
        for d in docs.iter().filter(|d| !is_memory_index(&d.item)) {
            budget(d, findings);
        }
    }
    if h.tool.id == "dsh" {
        // dsh renders each instruction chain under a per-chain byte budget (`maxBytes`, 65,536 bytes per
        // chain in the dsh-base default; docs/adapters/dsh.md, from deepseek-harness's agent-instructions
        // README): broader files are dropped first, then the most specific file is truncated at render
        // time. In multilingual text a byte figure and a chars/4 token estimate drift apart, so the finding
        // names where the cut lands on the file's own bytes.
        for d in docs.iter().filter(|d| d.item.detail.kind() == "instruction") {
            chain_budget(d, findings);
        }
    }
    repeated(&docs, h, findings);
    for d in &docs {
        let kind = layer_kind(h, &d.item.origin.layer);
        if matches!(kind, Some(LayerKind::Project | LayerKind::Local)) && d.path.starts_with(&ctx.repo) {
            dead_references(ctx, d, findings);
        }
    }
}

/// Where bytes cost more than characters: multilingual text (one CJK character is 3 bytes in UTF-8) makes a
/// byte budget fill sooner than a chars/4 token estimate shows.
fn divergence(chars: u64, bytes: u64) -> Option<String> {
    let per_char = bytes as f64 / chars.max(1) as f64;
    (per_char >= 1.2).then(|| {
        format!(
            "multilingual text at {per_char:.1} bytes a character: a byte budget fills about {per_char:.1}x sooner than this chars/4 estimate shows"
        )
    })
}

/// dsh's chain budget (`maxBytes`, 65,536 bytes per chain in the dsh-base default - docs/adapters/dsh.md, from
/// deepseek-harness's agent-instructions README): broader files are dropped first, then the most specific file
/// is truncated at render time. The audit reports where that cut lands on this file's actual bytes - the line,
/// and the file's own instructions at the boundary - so the byte figure means something a person can see.
fn chain_budget(d: &Doc, findings: &mut Vec<Finding>) {
    const CHAIN_BYTES: u64 = 65_536;
    const AROUND: usize = 60;
    let lines = d.text.lines().count();
    if lines <= CLAUDE_MD_LINES {
        return;
    }
    let chars = d.text.chars().count() as u64;
    let bytes = d.text.len() as u64;
    if bytes <= CHAIN_BYTES {
        return;
    }
    let cut = floor_char_boundary(&d.text, CHAIN_BYTES as usize);
    let line = d.text[..cut].matches('\n').count() + 1;
    let squeeze = |s: String| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let before: String = d.text[..cut].chars().rev().take(AROUND).collect();
    let before: String = before.chars().rev().collect();
    let after: String = d.text[cut..].chars().take(AROUND).collect();
    let mut evidence = vec![format!(
        "{lines} lines, {chars} characters, {bytes} bytes (~{} tokens; the estimate counts characters, the budget counts bytes)",
        tokens(chars),
    )];
    if let Some(note) = divergence(chars, bytes) {
        evidence.push(note);
    }
    evidence.push(
        "the chain's maxBytes budget (65,536 bytes per chain in dsh-base) lands mid-file: everything from the cut never renders".to_string(),
    );
    evidence.push(format!(
        "the cut lands on line {line}, at byte {CHAIN_BYTES}: \u{2026}{}\u{27e8}cut\u{27e9}{}\u{2026}",
        squeeze(before),
        squeeze(after),
    ));
    findings.push(Finding {
        code: "instruction-budget".into(),
        severity: Severity::High,
        title: format!(
            "{} is {lines} lines ({bytes} bytes); dsh's chain budget (maxBytes 65,536) cuts it at line {line}",
            printable(&d.item.name),
        ),
        evidence,
        saving: None,
        proposal: "Split the rules per directory so every file renders whole inside the chain budget. rigswig never rewrites instruction files.".into(),
        items: vec![d.item.id.clone()],
        file: d.item.origin.file.clone(),
    });
}

/// The last UTF-8 character boundary at or before `at`: a budget can cut inside a character, a quote may not.
fn floor_char_boundary(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Markdown sections (by `#` to `###` headings) with their sizes, largest first.
fn sections(text: &str) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = Vec::new();
    let mut cur = ("(before the first heading)".to_string(), 0u64);
    let mut fence = false;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            fence = !fence;
        }
        let t = line.trim_end();
        let level = t.chars().take_while(|c| *c == '#').count();
        if !fence && (1..=3).contains(&level) && t[level..].starts_with(' ') {
            if cur.1 > 0 {
                out.push(cur);
            }
            cur = (t[level..].trim().to_string(), 0);
        }
        cur.1 += line.chars().count() as u64;
    }
    if cur.1 > 0 {
        out.push(cur);
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.1));
    out
}

/// A CLAUDE.md that mentions the AGENTS.md beside it without importing it. With a CLAUDE.md present, Claude Code reads
/// AGENTS.md only through an `@AGENTS.md` import, so a markdown link (`[AGENTS.md](AGENTS.md)`) or a "see AGENTS.md"
/// loads nothing, and the rules in it never reach the model. One finding for the files that load at startup, one for
/// those in subfolders (on demand).
fn agents_md_not_imported(ctx: &Ctx, h: &Harness, findings: &mut Vec<Finding>) {
    let repo_real = ctx.fs.canonicalize(&ctx.repo).unwrap_or_else(|_| ctx.repo.clone());
    let inside = |p: &Path| ctx.fs.canonicalize(p).is_ok_and(|r| r.starts_with(&repo_real));
    let loaded: BTreeSet<&str> =
        h.items.iter().filter(|i| i.effective).filter_map(|i| i.origin.file.as_deref()).collect();
    // (startup?, the CLAUDE.md, its line, the AGENTS.md, its tokens, its item id)
    let mut hits: Vec<(bool, String, String, String, u64, String)> = Vec::new();
    for i in h.items.iter().filter(|i| {
        i.effective
            && matches!(i.detail, ItemDetail::Instruction(_))
            && matches!(i.load.when, LoadWhen::Startup | LoadWhen::OnMatch)
    }) {
        let Some(shown) = i.origin.file.as_deref() else { continue };
        let base = Path::new(shown).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !(base == "CLAUDE.md" || base == "CLAUDE.local.md") || shown.starts_with("~/") || Path::new(shown).has_root()
        {
            continue;
        }
        let dir = Path::new(shown).parent().map(|d| d.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        // `.claude/CLAUDE.md` sits beside the repo's own AGENTS.md, at the top.
        let dir = if dir == ".claude" { String::new() } else { dir };
        let agents = if dir.is_empty() { "AGENTS.md".to_string() } else { format!("{dir}/AGENTS.md") };
        let (claude_path, agents_path) = (real_path(ctx, shown), real_path(ctx, &agents));
        if loaded.contains(agents.as_str()) || !inside(&claude_path) || !inside(&agents_path) {
            continue;
        }
        let (Ok(text), Ok(agents_text)) = (ctx.fs.read_to_string(&claude_path), ctx.fs.read_to_string(&agents_path))
        else {
            continue;
        };
        let Some(line) = text.lines().find(|l| l.contains("AGENTS.md")) else { continue };
        hits.push((
            i.load.when == LoadWhen::Startup,
            shown.to_string(),
            line.trim().to_string(),
            agents,
            tokens(agents_text.chars().count() as u64),
            i.id.clone(),
        ));
    }
    for startup in [true, false] {
        let group: Vec<&(bool, String, String, String, u64, String)> = hits.iter().filter(|x| x.0 == startup).collect();
        if group.is_empty() {
            continue;
        }
        let total: u64 = group.iter().map(|x| x.4).sum();
        let mut evidence: Vec<String> = group
            .iter()
            .take(8)
            .map(|(_, c, line, a, t, _)| {
                format!("{} says \"{}\"; {} (~{t} tokens) never loads", printable(c), short(line, 70), printable(a))
            })
            .collect();
        if group.len() > 8 {
            evidence.push(format!("... and {} more", group.len() - 8));
        }
        let title = if startup {
            format!(
                "{} mentions AGENTS.md without importing it, so Claude Code never reads it (~{total} tokens of rules)",
                group.iter().map(|x| printable(&x.1)).collect::<Vec<_>>().join(" and ")
            )
        } else {
            format!(
                "{} in subfolders mention their AGENTS.md without importing it (~{total} tokens of rules never load)",
                plural(group.len(), "CLAUDE.md", "CLAUDE.md files")
            )
        };
        findings.push(Finding {
            code: "agents-md-not-imported".into(),
            severity: Severity::Medium,
            title,
            evidence,
            saving: None,
            proposal: "Put @AGENTS.md on a line of its own in each CLAUDE.md: with a CLAUDE.md present, Claude Code reads AGENTS.md only through an import. If the mention is meant for people, leave it.".into(),
            items: group.iter().map(|x| x.5.clone()).collect(),
            file: group.first().map(|x| x.1.clone()),
        });
    }
}

fn budget(d: &Doc, findings: &mut Vec<Finding>) {
    let lines = d.text.lines().count();
    if lines <= CLAUDE_MD_LINES {
        return;
    }
    let chars = d.text.chars().count() as u64;
    let bytes = d.text.len() as u64;
    let mut evidence = vec![format!("{lines} lines, {chars} characters, {bytes} bytes (~{} tokens)", tokens(chars))];
    if let Some(note) = divergence(chars, bytes) {
        evidence.push(note);
    }
    for (title, size) in sections(&d.text).iter().take(5) {
        evidence.push(format!(
            "{:>4.1}%  {} (~{} tokens)",
            *size as f64 * 100.0 / chars.max(1) as f64,
            short(title, 60),
            tokens(*size)
        ));
    }
    findings.push(Finding {
        code: "instruction-budget".into(),
        severity: if tokens(chars) >= 4_000 { Severity::High } else { Severity::Medium },
        title: format!("{} is {lines} lines; Claude Code's aim is under {CLAUDE_MD_LINES}", printable(&d.item.name)),
        evidence,
        saving: None,
        proposal: "Move the rules that apply to one area into that area's docs (or path-scoped .claude/rules/) and keep an index here. rigswig never rewrites instruction files.".into(),
        items: vec![d.item.id.clone()],
        file: d.item.origin.file.clone(),
    });
}

/// Sentences of eight words or more, normalised (lower case, markdown marks and extra space removed).
fn sentences(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim_start_matches(|c: char| c.is_whitespace() || "-*>#|".contains(c));
        for s in line.split_inclusive(['.', '!', '?']) {
            let norm: String = s
                .chars()
                .filter(|c| !"*_`".contains(*c))
                .collect::<String>()
                .to_lowercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let norm = norm.trim_end_matches(['.', '!', '?', ':', ';', ',']).to_string();
            if norm.split(' ').count() >= 8 {
                out.insert(norm);
            }
        }
    }
    out
}

fn repeated(docs: &[Doc], h: &Harness, findings: &mut Vec<Finding>) {
    let sets: Vec<BTreeSet<String>> = docs.iter().map(|d| sentences(&d.text)).collect();
    for a in 0..docs.len() {
        for b in a + 1..docs.len() {
            let shared: Vec<&String> = sets[a].intersection(&sets[b]).collect();
            let chars: u64 = shared.iter().map(|s| s.chars().count() as u64).sum();
            if shared.len() < 2 && chars < 200 {
                continue;
            }
            let (da, db) = (&docs[a], &docs[b]);
            let layer = |d: &Doc| {
                layer_kind(h, &d.item.origin.layer).map_or("?", |k| match k {
                    LayerKind::User => "user",
                    LayerKind::Project => "project",
                    LayerKind::Local => "local",
                    LayerKind::Managed => "managed",
                    _ => "other",
                })
            };
            let mut evidence: Vec<String> = shared.iter().take(3).map(|s| format!("\"{}\"", short(s, 90))).collect();
            if shared.len() > 3 {
                evidence.push(format!("... and {} more", shared.len() - 3));
            }
            findings.push(Finding {
                code: "repeated-text".into(),
                severity: if tokens(chars) >= 500 { Severity::Medium } else { Severity::Low },
                title: format!(
                    "{} in {} ({}) also in {} ({}), ~{} tokens loaded twice",
                    plural(shared.len(), "sentence", "sentences"),
                    printable(&db.item.name),
                    layer(db),
                    printable(&da.item.name),
                    layer(da),
                    tokens(chars)
                ),
                evidence,
                saving: None,
                proposal: "Keep each rule in one layer: rules for every repo in the user file, rules for this repo in the project file.".into(),
                items: vec![da.item.id.clone(), db.item.id.clone()],
                file: db.item.origin.file.clone(),
            });
        }
    }
}

/// Root `.gitignore` lines, as simple patterns (no negation): paths a reference may name without them existing.
fn ignored_patterns(ctx: &Ctx) -> Vec<String> {
    let text = ctx.fs.read_to_string(&ctx.repo.join(".gitignore")).unwrap_or_default();
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
        .map(|l| l.trim_matches('/').to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn is_ignored(patterns: &[String], rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    (1..=parts.len()).any(|n| {
        let prefix = parts[..n].join("/");
        let name = parts[n - 1];
        patterns.iter().any(|p| {
            let p = p.replace("**", "*");
            if p.contains('/') { glob(&p, &prefix) } else { glob(&p, name) }
        })
    })
}

/// A backticked span that looks like a repo path: it has a `/`, no spaces, globs, placeholders or URL, and does not
/// start at the home folder or the root.
fn path_like(span: &str) -> Option<String> {
    let s = span.trim().trim_end_matches([',', '.', ';', ')']);
    let s = s.strip_prefix("./").unwrap_or(s);
    // `file.ts:12` points at a line.
    let s = match s.rsplit_once(':') {
        Some((p, line)) if !line.is_empty() && line.chars().all(|c| c.is_ascii_digit()) => p,
        _ => s,
    };
    let bad = s.is_empty()
        || !s.contains('/')
        || s.contains("://")
        || s.contains("..")
        || s.starts_with(['/', '~', '-', '@'])
        || s.chars().any(|c| c.is_whitespace() || "*?<>{}[]$|=\\\"'(),`".contains(c));
    // `my_parser.cr`, `your-plugin/`, `example.ts`: a name to fill in, not a file that should exist.
    let last = s.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_lowercase();
    let placeholder = ["my_", "my-", "your_", "your-", "yourplugin", "example", "foo", "xxx", "placeholder"]
        .iter()
        .any(|p| last.starts_with(p));
    (!bad && !placeholder).then(|| s.to_string())
}

/// Inline code spans outside fenced blocks.
fn code_spans(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        let mut parts = line.split('`');
        parts.next();
        while let (Some(code), Some(_)) = (parts.next(), parts.next()) {
            out.push(code.to_string());
        }
    }
    out
}

/// Script names after `npm run`, `pnpm run`, `yarn run` and `bun run`, anywhere in the text.
fn npm_scripts(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for runner in ["npm run ", "pnpm run ", "yarn run ", "bun run "] {
        let mut from = 0;
        while let Some(k) = text[from..].find(runner) {
            let at = from + k;
            let ok_before = text[..at].chars().next_back().is_none_or(|c| !is_word(c));
            let rest = &text[at + runner.len()..];
            let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || ":_.-".contains(*c)).collect();
            let name = name.trim_end_matches(['.', ':']).to_string();
            if ok_before && !name.is_empty() && !name.starts_with('-') {
                out.insert(name);
            }
            from = at + runner.len();
        }
    }
    out
}

/// `make <target>` inside code (spans and fenced blocks), not in prose ("make sure").
fn make_targets(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut fence = false;
    let mut code: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence {
            code.push(line.to_string());
        }
    }
    code.extend(code_spans(text));
    for c in code {
        for part in c.split(['&', ';', '|']) {
            if let Some(rest) = part.trim().strip_prefix("make ") {
                if let Some(t) = rest.split_whitespace().find(|w| !w.starts_with('-') && !w.contains('=')) {
                    if t.chars().all(|c| c.is_ascii_alphanumeric() || "_.-/".contains(c)) {
                        out.insert(t.to_string());
                    }
                }
            }
        }
    }
    out
}

/// The folders beside the repo (its siblings), lower case: a line that names one is about another repo.
fn sibling_names(ctx: &Ctx) -> Vec<String> {
    let own = ctx.repo.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let Some(parent) = ctx.repo.parent() else { return Vec::new() };
    ctx.fs
        .read_dir(parent)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.kind == EntryKind::Dir && !e.name.starts_with('.'))
        .map(|e| e.name.to_lowercase())
        .filter(|n| *n != own && n.chars().count() >= 3)
        .collect()
}

/// `text` with the lines about another repo blanked (fence markers kept): a line that, with the one before it, names
/// a sibling folder or a `../` path. Its paths and scripts belong to that repo.
fn local_text(ctx: &Ctx, text: &str) -> String {
    let siblings = sibling_names(ctx);
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        let window = format!("{} {}", if i > 0 { lines[i - 1] } else { "" }, line).to_lowercase();
        let elsewhere = window.contains("../")
            || ["that repo", "other repo", "another repo", "sibling repo"].iter().any(|p| window.contains(p))
            || siblings.iter().any(|s| mentions(&window, s));
        // "Create `src/parsers/my_parser.rs`", "probably a small script `x.mjs`": a file to make, not one that should
        // already exist.
        let lower = line.to_lowercase();
        let to_make = [
            "create",
            "creates",
            "creating",
            "generate",
            "generates",
            "scaffold",
            "new file",
            "add",
            "adds",
            "adding",
            "new",
            "would",
            "probably",
            "proposed",
            "planned",
            "todo",
        ]
        .iter()
        .any(|w| mentions(&lower, w));
        if !(elsewhere || to_make) || line.trim_start().starts_with("```") {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

fn dead_references(ctx: &Ctx, d: &Doc, findings: &mut Vec<Finding>) {
    let ignored = ignored_patterns(ctx);
    let text = local_text(ctx, &d.text);
    let dir = d.path.parent().map(Path::to_path_buf).unwrap_or_else(|| ctx.repo.clone());
    let mut dead: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();
    for span in code_spans(&text) {
        let Some(rel) = path_like(&span) else { continue };
        if !seen.insert(rel.clone()) {
            continue;
        }
        let first = rel.split('/').next().unwrap_or("");
        let bases = [&ctx.repo, &dir];
        // Only judge paths whose first folder is here: `owner/repo` and other paths elsewhere are not repo paths.
        let Some(base) = bases.iter().find(|b| ctx.fs.exists(&b.join(first))) else { continue };
        if is_ignored(&ignored, &rel) {
            continue;
        }
        let found = bases.iter().any(|b| ctx.fs.exists(&b.join(rel.trim_end_matches('/'))));
        if !found {
            let shown = if *base == &ctx.repo { rel } else { format!("{rel} (beside the file)") };
            dead.push(format!("`{}`: no such path", short(&shown, 80)));
        }
    }

    let pkg = ctx.fs.read_to_string(&ctx.repo.join("package.json")).ok();
    if let Some(scripts) = pkg
        .as_deref()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
        .and_then(|v| v.get("scripts").and_then(|s| s.as_object()).cloned())
    {
        for name in npm_scripts(&text) {
            if !scripts.contains_key(&name) {
                dead.push(format!("`npm run {}`: no such script in package.json", short(&name, 60)));
            }
        }
    }
    if let Ok(makefile) = ctx.fs.read_to_string(&ctx.repo.join("Makefile")) {
        let targets: BTreeSet<String> = makefile
            .lines()
            .filter(|l| !l.starts_with(['\t', ' ', '#', '.']))
            .filter_map(|l| l.split_once(':').filter(|(_, r)| !r.starts_with('=')).map(|(t, _)| t))
            .flat_map(|t| t.split_whitespace().map(String::from))
            .collect();
        for t in make_targets(&text) {
            if !targets.contains(&t) {
                dead.push(format!("`make {}`: no such target in the Makefile", short(&t, 60)));
            }
        }
    }
    if dead.is_empty() {
        return;
    }
    let n = dead.len();
    let mut evidence: Vec<String> = dead.into_iter().take(10).collect();
    if n > 10 {
        evidence.push(format!("... and {} more", n - 10));
    }
    findings.push(Finding {
        code: "dead-reference".into(),
        severity: Severity::Medium,
        title: format!(
            "{} names {} that the repo does not have",
            printable(&d.item.name),
            plural(n, "path or script", "paths or scripts")
        ),
        evidence,
        saving: None,
        proposal: "Fix or remove these references: an agent that follows one spends turns looking for it.".into(),
        items: vec![d.item.id.clone()],
        file: d.item.origin.file.clone(),
    });
}

/// Commands whose broad allow (with a wildcard) lets an agent delete, kill, run any code, publish or reach the network
/// without asking.
const RISKY: &[&str] = &[
    "rm",
    "kill",
    "pkill",
    "killall",
    "sudo",
    "chmod",
    "chown",
    "dd",
    "node",
    "python",
    "python3",
    "ruby",
    "perl",
    "bash",
    "sh",
    "zsh",
    "eval",
    "curl",
    "wget",
    "ssh",
    "scp",
    "npx",
    "bunx",
    "deno",
    "docker",
    "git push",
    "git reset",
    "git clean",
    "gh api",
];
/// Paths that hold credentials.
const SECRET_PATHS: &[&str] = &[
    ".ssh",
    ".aws",
    ".gnupg",
    ".config/gh",
    ".config/gcloud",
    ".netrc",
    ".npmrc",
    ".docker/config",
    ".kube",
    "credentials",
    ".env",
    "keychain",
    ".pypirc",
    ".git-credentials",
];

struct Rule<'a> {
    item: &'a Item,
    tool: &'a str,
    pattern: Option<&'a str>,
}

/// Whether `broad` allows everything `narrow` does (same tool; Claude Code's `prefix:*` form and `*` globs).
fn covers(broad: &Rule, narrow: &Rule) -> bool {
    if broad.tool != narrow.tool || broad.item.id == narrow.item.id {
        return false;
    }
    let Some(n) = narrow.pattern else { return false };
    match broad.pattern {
        None => true,
        Some(b) if b == n => false,
        Some(b) => match b.strip_suffix(":*") {
            Some(prefix) => n.starts_with(prefix) && n != b,
            None => b.contains('*') && glob(&b.replace("**", "*"), n),
        },
    }
}

fn risky(r: &Rule) -> Option<String> {
    match (r.tool, r.pattern) {
        ("Bash", None) => Some("every shell command".into()),
        ("Bash", Some(p)) => {
            let p = p.trim();
            if p == "*" || p == ":*" {
                return Some("every shell command".into());
            }
            if !p.contains('*') {
                return None;
            }
            let head = p.trim_end_matches(":*").trim_end_matches('*').trim();
            RISKY.iter().find(|cmd| head == **cmd).map(|cmd| format!("any `{cmd}` command"))
        }
        ("Read" | "Edit" | "Write", Some(p)) => {
            SECRET_PATHS.iter().find(|s| p.contains(*s)).map(|s| format!("{} of a credentials path ({s})", r.tool))
        }
        _ => None,
    }
}

fn one_off(r: &Rule) -> bool {
    r.tool == "Bash"
        && r.pattern.is_some_and(|p| {
            !p.contains('*')
                && (p.contains("://")
                    || p.chars().count() >= 60
                    || p.contains(['"', '\''])
                    || p.contains(" && ")
                    || p.contains(" | "))
        })
}

fn permissions(h: &Harness, findings: &mut Vec<Finding>) {
    let rules: Vec<Rule> = h
        .items
        .iter()
        .filter(|i| i.effective)
        .filter_map(|i| match &i.detail {
            ItemDetail::Permission(p) if p.effect == PermissionEffect::Allow => {
                Some(Rule { item: i, tool: p.tool.as_str(), pattern: p.pattern.as_deref() })
            }
            _ => None,
        })
        .collect();
    let mut files: BTreeMap<String, Vec<&Rule>> = BTreeMap::new();
    for r in &rules {
        files.entry(r.item.origin.file.clone().unwrap_or_else(|| r.item.origin.layer.clone())).or_default().push(r);
    }
    for (file, rs) in &files {
        let rule_text = |r: &Rule| short(r.item.name.strip_prefix("allow ").unwrap_or(&r.item.name), 80);
        let mut push = |code: &str, severity: Severity, title: String, list: Vec<(String, String)>, proposal: &str| {
            if list.is_empty() {
                return;
            }
            let n = list.len();
            let mut evidence: Vec<String> = list.iter().take(6).map(|(t, _)| t.clone()).collect();
            if n > 6 {
                evidence.push(format!("... and {} more", n - 6));
            }
            findings.push(Finding {
                code: code.into(),
                severity,
                title,
                evidence,
                saving: None,
                proposal: proposal.into(),
                items: list.into_iter().map(|(_, id)| id).collect(),
                file: Some(file.clone()),
            });
        };

        let broad: Vec<(String, String)> = rs
            .iter()
            .filter_map(|r| risky(r).map(|why| (format!("{} ({why})", rule_text(r)), r.item.id.clone())))
            .collect();
        let nb = broad.len();
        push(
            "permission-broad",
            Severity::High,
            format!(
                "{} in {} {} without asking",
                plural(nb, "allow rule", "allow rules"),
                printable(file),
                if nb == 1 { "lets an agent do something risky" } else { "let an agent do risky things" }
            ),
            broad,
            "Drop these, or narrow each to the exact commands or paths you approve; Claude Code asks for the rest.",
        );

        let offs: Vec<(String, String)> =
            rs.iter().filter(|r| one_off(r)).map(|r| (rule_text(r), r.item.id.clone())).collect();
        let no = offs.len();
        push(
            "permission-one-off",
            if no >= 20 { Severity::Medium } else { Severity::Low },
            format!(
                "{} in {} {} one command, not a pattern",
                plural(no, "allow rule", "allow rules"),
                printable(file),
                if no == 1 { "is" } else { "are each" }
            ),
            offs,
            "These pile up from \"always allow\" answers. Drop them; keep a pattern only for commands you run often.",
        );

        let covered: Vec<(String, String)> = rs
            .iter()
            .filter_map(|r| {
                let by = rules.iter().find(|b| covers(b, r))?;
                let place = by.item.origin.file.as_deref().unwrap_or(&by.item.origin.layer);
                Some((
                    format!("{} (covered by {} in {})", rule_text(r), rule_text(by), printable(place)),
                    r.item.id.clone(),
                ))
            })
            .collect();
        let nc = covered.len();
        push(
            "permission-covered",
            Severity::Low,
            format!(
                "{} in {} {} already covered by broader ones",
                plural(nc, "allow rule", "allow rules"),
                printable(file),
                if nc == 1 { "is" } else { "are" }
            ),
            covered,
            "Drop them: the broader rule already allows the same commands.",
        );
    }
}

/// The harness's own warnings, as findings.
fn warnings(h: &Harness, findings: &mut Vec<Finding>) {
    for w in h.warnings.iter().filter(|w| w.code != "what-if") {
        let base = w.code.strip_prefix(crate::model::INCOMPLETE_WARNING_PREFIX).unwrap_or(&w.code);
        let (severity, proposal) = match base {
            "agent-all-tools" => (
                Severity::Medium,
                "Give each agent a `tools` list in its front matter, so a review or research agent cannot edit or run commands.",
            ),
            "plugin-missing" => (Severity::Medium, "Reinstall the plugin, or remove it from enabledPlugins."),
            "mcp-pending" => (Severity::Low, "Approve or reject the server, so each session starts the same way."),
            "malformed-json" | "malformed-toml" => {
                (Severity::High, "Fix the file: the harness skips it too, so its settings are not in effect.")
            }
            _ => (Severity::Low, "See `rigswig status --json` for the warning in context."),
        };
        let items: Vec<String> = match base {
            "agent-all-tools" => h
                .items
                .iter()
                .filter(|i| {
                    i.effective && matches!(&i.detail, ItemDetail::Agent(a) if a.tools == crate::model::ToolAccess::All)
                })
                .map(|i| i.id.clone())
                .collect(),
            _ => w.item.iter().cloned().collect(),
        };
        let evidence: Vec<String> = if base == "agent-all-tools" {
            h.items
                .iter()
                .filter(|i| items.contains(&i.id))
                .take(8)
                .map(|i| format!("{} ({})", short(&i.name, 60), printable(&i.origin.layer)))
                .collect()
        } else {
            Vec::new()
        };
        findings.push(Finding {
            code: format!("warning:{}", w.code),
            severity,
            title: printable(&w.message),
            evidence,
            saving: None,
            proposal: proposal.into(),
            items,
            file: w.file.clone(),
        });
    }
}

/// Fleet mode: the same user-scope items seen from many repos.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fleet {
    pub repos: Vec<FleetRepo>,
    /// User-scope items, the costliest per session first.
    pub items: Vec<FleetItem>,
    /// What moving every item the fleet does not need saves, per repo: estimated tokens from startup files.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetRepo {
    pub repo: String,
    pub tool: String,
    pub tokens: u64,
    pub findings: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetItem {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub tokens: u64,
    /// The repos whose files say they need it.
    pub needed_in: Vec<String>,
    /// How many repos were judged (a repo that cannot tell is left out).
    pub judged: usize,
    pub recommendation: Recommendation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Recommendation {
    /// Needed in most repos: user scope is right.
    Keep,
    /// Needed in a few: project scope in those.
    MoveToProject,
    /// Nothing references it anywhere.
    Archive,
    /// No repo could judge it.
    Unknown,
}

impl Recommendation {
    pub fn sentence(&self, item: &FleetItem) -> String {
        let n = item.needed_in.len();
        match self {
            Self::Keep => format!("needed in {n} of {}: keep at user scope", item.judged),
            Self::MoveToProject => format!(
                "needed in {n} of {}: move to project scope in {}",
                item.judged,
                if n == 1 { "that one".to_string() } else { format!("those {n}") }
            ),
            // A plugin is judged by the repo's files (its signals), not by name.
            Self::Archive if item.kind == "plugin" => {
                format!("none of {} repos has the files it is for: turn it off at user scope", item.judged)
            }
            Self::Archive => {
                format!("nothing in {} repos references it: archive it, unless you call it by hand", item.judged)
            }
            Self::Unknown => "no signal to judge it by".into(),
        }
    }
}

/// Combine the audits of many repos (one harness, one home folder).
pub fn fleet(audits: &[Audit]) -> Fleet {
    let mut by: BTreeMap<String, FleetItem> = BTreeMap::new();
    for a in audits {
        for u in &a.user_scope {
            let e = by.entry(u.id.clone()).or_insert_with(|| FleetItem {
                id: u.id.clone(),
                kind: u.kind.clone(),
                name: u.name.clone(),
                tokens: u.tokens,
                needed_in: Vec::new(),
                judged: 0,
                recommendation: Recommendation::Unknown,
            });
            if let Some(needed) = u.needed {
                e.judged += 1;
                if needed {
                    e.needed_in.push(a.repo.clone());
                }
            }
        }
    }
    let mut items: Vec<FleetItem> = by
        .into_values()
        .map(|mut i| {
            let n = i.needed_in.len();
            i.recommendation = if i.judged == 0 {
                Recommendation::Unknown
            } else if n == 0 {
                Recommendation::Archive
            } else if n * 2 < i.judged {
                Recommendation::MoveToProject
            } else {
                Recommendation::Keep
            };
            i
        })
        .collect();
    items.sort_by(|a, b| {
        let rank = |r: Recommendation| match r {
            Recommendation::Archive => 0,
            Recommendation::MoveToProject => 1,
            Recommendation::Unknown => 2,
            Recommendation::Keep => 3,
        };
        rank(a.recommendation).cmp(&rank(b.recommendation)).then(b.tokens.cmp(&a.tokens)).then(a.id.cmp(&b.id))
    });
    let movable: u64 = items
        .iter()
        .filter(|i| matches!(i.recommendation, Recommendation::Archive | Recommendation::MoveToProject))
        .map(|i| i.tokens)
        .sum();
    let mut notes = Vec::new();
    if movable > 0 {
        notes.push(format!(
            "archiving or moving what this suggests takes up to ~{movable} tokens out of every session in a repo that needs none of it"
        ));
    }
    notes.push(
        "\"needed\" means the repo's files reference it by name, or match a plugin's signals; usage is never read"
            .into(),
    );
    Fleet {
        repos: audits
            .iter()
            .map(|a| FleetRepo {
                repo: a.repo.clone(),
                tool: a.tool.clone(),
                tokens: a.tokens,
                findings: a.findings.len(),
            })
            .collect(),
        items,
        notes,
    }
}

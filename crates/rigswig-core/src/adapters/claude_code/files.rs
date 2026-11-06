//! Instruction files (CLAUDE.md, AGENTS.md, rules, @imports), agents, skills and commands.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::Paths;
use super::plugins::Plugin;
use crate::adapters::build::{Build, calc, char_len};
use crate::fs::is_within;
use crate::model::*;
use crate::read::frontmatter::{self, FrontMatter};
use crate::read::imports::imports_of;
use crate::read::redact::sha256_hex;

/// Claude Code follows @imports at most four hops.
const MAX_IMPORT_HOPS: usize = 4;
/// The skill listing cuts description + when_to_use at this many characters.
const SKILL_LISTING_CAP: u64 = 1536;
/// A file larger than this is counted by its length but not parsed for imports.
const MAX_PARSE: usize = 1_000_000;

struct Instr<'p> {
    p: &'p Paths,
    seen: BTreeSet<PathBuf>,
    roots: Vec<PathBuf>,
}

fn add_instruction(b: &mut Build, st: &mut Instr, file: &Path, layer: &str, hops: usize, imported_by: Option<String>) {
    let Ok(real) = b.fs().canonicalize(file) else { return };
    if !st.seen.insert(real.clone()) {
        return;
    }
    let Ok(text) = b.fs().read_to_string(file) else { return };
    let name = b.display(file);
    let (when, applies_to) = rule_scope(&text, file);
    let mut it = b.item(
        layer,
        &name,
        ItemDetail::Instruction(InstructionAttrs { imported_by: imported_by.clone(), applies_to }),
        when,
        calc(char_len(&text)),
    );
    it.origin.file = Some(name.clone());
    it.load.content_hash = Some(sha256_hex(&text));
    let idx = b.push(it);
    let id = b.items[idx].id.clone();
    if hops >= MAX_IMPORT_HOPS || text.len() > MAX_PARSE {
        return;
    }
    let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    for imp in imports_of(&text) {
        let target = match imp.strip_prefix("~/") {
            Some(rest) => b.ctx.home.join(rest),
            None if imp.starts_with('/') => PathBuf::from(&imp),
            None => dir.join(&imp),
        };
        let Ok(t) = b.fs().canonicalize(&target) else { continue };
        if b.fs().read_to_string(&t).is_err() {
            continue;
        }
        if !st.roots.iter().any(|r| is_within(&t, r)) {
            b.warn("import-outside", "an @import outside the repo and the config folder was not followed", Some(file));
            continue;
        }
        add_instruction(b, st, &target, layer, hops + 1, Some(id.clone()));
    }
}

/// A rule file's `paths` front matter makes it load on match; everything else loads at startup.
fn rule_scope(text: &str, file: &Path) -> (LoadWhen, Vec<String>) {
    let in_rules = file.components().any(|c| c.as_os_str() == "rules");
    if !in_rules {
        return (LoadWhen::Startup, Vec::new());
    }
    let fm = frontmatter::parse(text);
    match fm.fields.get("paths") {
        Some(v) if !v.as_list().is_empty() => (LoadWhen::OnMatch, v.as_list()),
        _ => (LoadWhen::Startup, Vec::new()),
    }
}

/// Folders the subfolder search never enters, besides dot folders: dependencies and build output.
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
/// The subfolder search stops after this many folders, and says so.
const MAX_SUBFOLDERS: usize = 20_000;

/// Every real folder below the repo, with its repo-relative path (`apps/web`), nearest first. Dot folders,
/// dependencies and build output are skipped, and links are never followed (a link is not a folder here).
pub(crate) fn subfolders(b: &mut Build, repo: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::from([(repo.to_path_buf(), String::new(), 0usize)]);
    while let Some((dir, rel, depth)) = queue.pop_front() {
        if depth >= 12 {
            continue;
        }
        for e in b.fs().read_dir(&dir).unwrap_or_default() {
            if e.kind != crate::fs::EntryKind::Dir || e.name.starts_with('.') || SKIP_DIRS.contains(&e.name.as_str()) {
                continue;
            }
            if out.len() >= MAX_SUBFOLDERS {
                b.note_not_modelled(format!(
                    "subfolders past the first {MAX_SUBFOLDERS}: their CLAUDE.md files and skills are not listed"
                ));
                return out;
            }
            let r = if rel.is_empty() { e.name.clone() } else { format!("{rel}/{}", e.name) };
            out.push((dir.join(&e.name), r.clone()));
            queue.push_back((dir.join(&e.name), r, depth + 1));
        }
    }
    out
}

/// Items added since `from` load when Claude reads, writes or edits a file under `rel`, not at startup.
fn on_demand(b: &mut Build, from: usize, rel: &str) {
    for it in &mut b.items[from..] {
        if it.effective {
            it.load.when = LoadWhen::OnMatch;
        }
        match &mut it.detail {
            ItemDetail::Instruction(a) if a.applies_to.is_empty() => a.applies_to = vec![format!("{rel}/**")],
            _ => {}
        }
    }
}

pub(crate) fn instructions(b: &mut Build, p: &Paths, subs: &[(PathBuf, String)]) {
    let roots: Vec<PathBuf> =
        [&p.repo, &p.cfg].iter().map(|r| b.fs().canonicalize(r).unwrap_or_else(|_| (*r).clone())).collect();
    let mut st = Instr { p, seen: BTreeSet::new(), roots };

    add_instruction(b, &mut st, &p.system.join("CLAUDE.md"), "managed", 0, None);
    add_instruction(b, &mut st, &p.cfg.join("CLAUDE.md"), "user", 0, None);
    for (f, _) in b.walk(&p.cfg.join("rules"), ".md") {
        add_instruction(b, &mut st, &f, "user", 0, None);
    }

    // The working directory and every folder above it, root first; CLAUDE.local.md after CLAUDE.md.
    let mut chain: Vec<PathBuf> = st.p.repo.ancestors().map(Path::to_path_buf).collect();
    chain.reverse();
    let any_claude =
        chain.iter().any(|d| b.fs().exists(&d.join("CLAUDE.md")) || b.fs().exists(&d.join("CLAUDE.local.md")))
            || b.fs().exists(&p.repo.join(".claude/CLAUDE.md"));
    for d in &chain {
        add_instruction(b, &mut st, &d.join("CLAUDE.md"), "project", 0, None);
        add_instruction(b, &mut st, &d.join("CLAUDE.local.md"), "local", 0, None);
    }
    add_instruction(b, &mut st, &p.repo.join(".claude/CLAUDE.md"), "project", 0, None);
    if !any_claude {
        for d in &chain {
            add_instruction(b, &mut st, &d.join("AGENTS.md"), "project", 0, None);
        }
    }
    for (f, _) in b.walk(&p.repo.join(".claude/rules"), ".md") {
        add_instruction(b, &mut st, &f, "project", 0, None);
    }

    // CLAUDE.md and CLAUDE.local.md in subfolders load on demand: once Claude reads, writes or edits a file there.
    // What they import loads with them.
    for (d, rel) in subs {
        let from = b.items.len();
        add_instruction(b, &mut st, &d.join("CLAUDE.md"), "project", 0, None);
        add_instruction(b, &mut st, &d.join("CLAUDE.local.md"), "local", 0, None);
        on_demand(b, from, rel);
    }
    b.note_not_modelled("AGENTS.md and .claude/rules in subfolders (the docs do not say when they load)");
}

fn read_fm(b: &Build, file: &Path) -> Option<(String, FrontMatter)> {
    let text = b.fs().read_to_string(file).ok()?;
    let fm = frontmatter::parse(&text);
    Some((text, fm))
}

fn stem(file: &Path) -> String {
    file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn agent(b: &mut Build, file: &Path, layer: &str, name: String) -> Option<usize> {
    let (text, fm) = read_fm(b, file)?;
    let desc = fm.str("description").unwrap_or("");
    let tools = fm.fields.get("tools").map(|v| v.as_list()).filter(|v| !v.is_empty());
    let tool_chars = tools.as_ref().map_or(0, |t| char_len(&t.join(", ")));
    let chars = char_len(&name) + char_len(desc) + tool_chars;
    for key in ["hooks", "mcpServers"] {
        if fm.unsupported.iter().any(|k| k == key) || fm.fields.contains_key(key) {
            b.warn(
                "agent-frontmatter",
                format!("agent {name} declares {key} in its front matter (not modelled)"),
                Some(file),
            );
        }
    }
    let access = tools.map_or(ToolAccess::All, ToolAccess::Only);
    let mut it = b.item(
        layer,
        &name,
        ItemDetail::Agent(AgentAttrs { tools: access, model: fm.str("model").map(String::from) }),
        LoadWhen::Startup,
        calc(chars),
    );
    it.origin.file = Some(b.display(file));
    it.load.content_hash = Some(sha256_hex(&text));
    Some(b.push(it))
}

fn skill(b: &mut Build, dir: &Path, layer: &str, name: String) -> Option<usize> {
    let file = dir.join("SKILL.md");
    let (text, fm) = read_fm(b, &file)?;
    let listed = char_len(fm.str("description").unwrap_or("")) + char_len(fm.str("when_to_use").unwrap_or(""));
    let chars = char_len(&name) + listed.min(SKILL_LISTING_CAP);
    let body = char_len(&text[fm.body_start..]);
    let mut it =
        b.item(layer, &name, ItemDetail::Skill(SkillAttrs { body_chars: Some(body) }), LoadWhen::Startup, calc(chars));
    it.origin.file = Some(b.display(&file));
    it.load.content_hash = Some(sha256_hex(&text));
    Some(b.push(it))
}

fn command(b: &mut Build, file: &Path, layer: &str, name: String, inline_desc: Option<&str>) -> Option<usize> {
    let (text, desc) = match inline_desc {
        Some(d) => (String::new(), d.to_string()),
        None => {
            let (text, fm) = read_fm(b, file)?;
            let d = fm.str("description").unwrap_or("").to_string();
            (text, d)
        }
    };
    let chars = char_len(&name) + char_len(&desc).min(SKILL_LISTING_CAP);
    let mut it = b.item(layer, &name, ItemDetail::Command(CommandAttrs {}), LoadWhen::Startup, calc(chars));
    if inline_desc.is_none() {
        it.origin.file = Some(b.display(file));
        it.load.content_hash = Some(sha256_hex(&text));
    }
    Some(b.push(it))
}

fn skill_name(b: &Build, dir: &Path) -> String {
    read_fm(b, &dir.join("SKILL.md")).and_then(|(_, fm)| fm.str("name").map(String::from)).unwrap_or_else(|| stem(dir))
}

fn skills_in(
    b: &mut Build,
    root: &Path,
    layer: &str,
    ns: Option<&str>,
    keep: &dyn Fn(&Build, &Path) -> bool,
) -> Vec<usize> {
    let mut out = Vec::new();
    for d in b.dirs_in(root) {
        if !b.fs().exists(&d.join("SKILL.md")) || !keep(b, &d) || !keep(b, &d.join("SKILL.md")) {
            continue;
        }
        let n = skill_name(b, &d);
        let name = ns.map_or(n.clone(), |ns| format!("{ns}:{n}"));
        out.extend(skill(b, &d, layer, name));
    }
    out
}

pub(crate) fn agents_skills_commands(b: &mut Build, p: &Paths, plugins: &[Plugin], subs: &[(PathBuf, String)]) {
    let all = |_: &Build, _: &Path| true;
    let enterprise = p.system.join(".claude");
    b.note_not_modelled("the enterprise agents and skills folders are assumed to be .claude/agents and .claude/skills in the system folder");

    // Agents: managed > project > user > plugin (plugins are namespaced and never clash).
    // Each folder is scanned recursively; a subfolder does not change an agent's name, which comes from `name`. Project
    // agents come from every .claude/agents between the working directory and the git root, the closest winning.
    let mut agents = Vec::new();
    let root = git_root(b, &p.repo);
    let project_dirs: Vec<PathBuf> =
        p.repo.ancestors().take_while(|d| d.starts_with(&root)).map(|d| d.join(".claude/agents")).collect();
    let mut roots = vec![(enterprise.join("agents"), "managed", 0usize)];
    roots.extend(project_dirs.into_iter().enumerate().map(|(n, d)| (d, "project", n)));
    roots.push((p.cfg.join("agents"), "user", 0));
    for (dir, layer, distance) in roots {
        for (f, _) in b.walk(&dir, ".md") {
            let name = read_fm(b, &f).and_then(|(_, fm)| fm.str("name").map(String::from)).unwrap_or_else(|| stem(&f));
            if distance > 0 {
                // A farther folder's agent with a name a closer one has is overridden by it: `push` already shadows
                // it by the first one in, the closer; the warning it adds about two files "in one place" is dropped.
                let id = format!("{}:agent:project:{}", super::ID, crate::read::redact::printable(&name));
                if b.items.iter().any(|i| i.id == id) {
                    let msg = format!(
                        "two agents named {:?} in one place; the second is ignored",
                        crate::read::redact::printable(&name)
                    );
                    let before = b.warnings.iter().filter(|w| w.message == msg).count();
                    agent(b, &f, layer, name);
                    if b.warnings.iter().filter(|w| w.message == msg).count() > before {
                        b.warnings.retain(|w| w.message != msg);
                    }
                    continue;
                }
            }
            agents.extend(agent(b, &f, layer, name));
        }
    }
    let order = |l: &str| match l {
        "managed" => 0,
        "project" => 1,
        "user" => 2,
        _ => 3,
    };
    b.resolve_clashes(&agents, |i| i.name.clone(), |i| order(&i.origin.layer));

    // Skills: managed > user > project. Commands are skills; a skill beats a command of the same name.
    let skill_order = |l: &str| match l {
        "managed" => 0,
        "user" => 1,
        "project" => 2,
        _ => 3,
    };
    let mut skills = Vec::new();
    for (root, layer) in [
        (enterprise.join("skills"), "managed"),
        (p.cfg.join("skills"), "user"),
        (p.repo.join(".claude/skills"), "project"),
    ] {
        skills.extend(skills_in(b, &root, layer, None, &all));
    }
    // Skills in a subfolder's .claude/skills load the first time Claude reads or edits a file there, named after the
    // folder (`apps/web:deploy`), and never clash with the root's.
    for (d, rel) in subs {
        let from = b.items.len();
        skills_in(b, &d.join(".claude/skills"), "project", Some(rel), &all);
        on_demand(b, from, rel);
    }
    b.note_not_modelled(
        "skills nested deeper than skills/<name>/SKILL.md, command subfolders, and .claude/agents below the working directory",
    );
    let mut commands = Vec::new();
    for (root, layer) in [(p.cfg.join("commands"), "user"), (p.repo.join(".claude/commands"), "project")] {
        for f in b.files_in(&root, ".md") {
            commands.extend(command(b, &f, layer, stem(&f), None));
        }
    }
    b.note_not_modelled("which of a user and a project command with the same name wins (the skills order is assumed)");

    for pl in plugins {
        plugin_files(b, pl, &mut skills, &mut commands);
    }

    b.resolve_clashes(&skills, |i| i.name.clone(), |i| skill_order(&i.origin.layer));
    b.resolve_clashes(&commands, |i| i.name.clone(), |i| skill_order(&i.origin.layer));
    // A skill beats a command of the same name.
    let skill_names: std::collections::BTreeMap<String, String> = skills
        .iter()
        .filter(|&&i| b.items[i].effective)
        .map(|&i| (b.items[i].name.clone(), b.items[i].id.clone()))
        .collect();
    for &i in &commands {
        if b.items[i].effective {
            if let Some(winner) = skill_names.get(&b.items[i].name) {
                let w = winner.clone();
                Build::shadow(&mut b.items[i], &w);
            }
        }
    }

    let all_tools = b
        .items
        .iter()
        .filter(|i| i.effective && matches!(&i.detail, ItemDetail::Agent(a) if a.tools == ToolAccess::All))
        .count();
    if all_tools > 0 {
        b.warn(
            "agent-all-tools",
            format!(
                "{all_tools} agent{} name no tools, so each can use every tool",
                if all_tools == 1 { "" } else { "s" }
            ),
            None,
        );
    }
}

/// A plugin's agents, skills and commands, every path confined to the plugin folder.
fn plugin_files(b: &mut Build, pl: &Plugin, skills: &mut Vec<usize>, commands: &mut Vec<usize>) {
    let keep = |b: &Build, p: &Path| pl.confine(b, p.strip_prefix(&pl.dir).unwrap_or(p)).is_some();
    let warn_escape = |b: &mut Build| {
        b.warn("plugin-path-escape", format!("plugin {}: an entry leaves the plugin folder, ignored", pl.id), None)
    };

    // Agents: `agents` in the manifest replaces agents/. Subfolders add to the name.
    let agent_roots: Vec<PathBuf> = match pl.manifest_list("agents") {
        None => vec![pl.dir.join("agents")],
        Some(list) => list.iter().filter_map(Value::as_str).filter_map(|r| pl.path(b, r, "agents")).collect(),
    };
    for root in agent_roots {
        let files: Vec<(PathBuf, String)> = if root.extension().is_some_and(|e| e == "md") {
            vec![(root.clone(), root.file_name().unwrap_or_default().to_string_lossy().into_owned())]
        } else {
            b.walk(&root, ".md")
        };
        for (f, rel) in files {
            if !keep(b, &f) {
                warn_escape(b);
                continue;
            }
            let base = read_fm(b, &f).and_then(|(_, fm)| fm.str("name").map(String::from)).unwrap_or_else(|| stem(&f));
            let folders: Vec<&str> = rel.split('/').collect();
            let mut parts = vec![pl.ns.as_str()];
            parts.extend(&folders[..folders.len().saturating_sub(1)]);
            let name = format!("{}:{base}", parts.join(":"));
            agent(b, &f, &pl.layer, name);
        }
    }

    // Skills: skills/ plus whatever the manifest adds.
    skills.extend(skills_in(b, &pl.dir.join("skills"), &pl.layer, Some(&pl.ns), &keep));
    for r in pl.manifest_list("skills").unwrap_or_default().iter().filter_map(Value::as_str) {
        let Some(d) = pl.path(b, r, "skills") else { continue };
        if b.fs().exists(&d.join("SKILL.md")) {
            if keep(b, &d.join("SKILL.md")) {
                let n = format!("{}:{}", pl.ns, skill_name(b, &d));
                skills.extend(skill(b, &d, &pl.layer, n));
            }
        } else {
            skills.extend(skills_in(b, &d, &pl.layer, Some(&pl.ns), &keep));
        }
    }

    // Commands: `commands` in the manifest replaces commands/: a path, a list, or a map of name to source/content.
    match pl.manifest.get("commands") {
        None | Some(Value::Null) => {
            for f in b.files_in(&pl.dir.join("commands"), ".md") {
                if keep(b, &f) {
                    commands.extend(command(b, &f, &pl.layer, format!("{}:{}", pl.ns, stem(&f)), None));
                } else {
                    warn_escape(b);
                }
            }
        }
        Some(Value::Object(map)) => {
            for (key, c) in map {
                let Some(c) = c.as_object() else { continue };
                let name = format!("{}:{key}", pl.ns);
                if let Some(src) = c.get("source").and_then(Value::as_str) {
                    let Some(f) = pl.path(b, src, "commands") else { continue };
                    match c.get("description").and_then(Value::as_str) {
                        Some(d) => commands.extend(command(b, &f, &pl.layer, name, Some(d))),
                        None => commands.extend(command(b, &f, &pl.layer, name, None)),
                    }
                } else {
                    let d = c.get("description").and_then(Value::as_str).unwrap_or("");
                    commands.extend(command(b, &pl.dir, &pl.layer, name, Some(d)));
                }
            }
        }
        Some(_) => {
            for r in pl.manifest_list("commands").unwrap_or_default().iter().filter_map(Value::as_str) {
                let Some(p) = pl.path(b, r, "commands") else { continue };
                if b.fs().read_dir(&p).is_ok() {
                    for f in b.files_in(&p, ".md") {
                        if keep(b, &f) {
                            commands.extend(command(b, &f, &pl.layer, format!("{}:{}", pl.ns, stem(&f)), None));
                        }
                    }
                } else if p.extension().is_some_and(|e| e == "md") {
                    commands.extend(command(b, &p, &pl.layer, format!("{}:{}", pl.ns, stem(&p)), None));
                }
            }
        }
    }
}

/// Auto memory loads the first 200 lines of `MEMORY.md`, or its first 25 KB, whichever comes first.
const MEMORY_LINES: usize = 200;
const MEMORY_BYTES: usize = 25 * 1024;

/// The repo's git root: the main checkout for a worktree (whose `.git` is a file pointing into the main `.git`), the
/// folder holding `.git` otherwise, and the repo folder itself outside git.
fn git_root(b: &Build, repo: &Path) -> PathBuf {
    for d in repo.ancestors() {
        let g = d.join(".git");
        match b.fs().kind(&g) {
            Ok(crate::fs::EntryKind::Dir) => return d.to_path_buf(),
            Ok(_) => {
                let text = b.fs().read_to_string(&g).unwrap_or_default();
                let Some(target) = text.lines().find_map(|l| l.strip_prefix("gitdir:")).map(str::trim) else {
                    return d.to_path_buf();
                };
                let target = if Path::new(target).has_root() { PathBuf::from(target) } else { d.join(target) };
                // `<main>/.git/worktrees/<name>`: the main checkout is the folder above `.git`.
                let mut cur = target.as_path();
                while let Some(parent) = cur.parent() {
                    if cur.file_name().is_some_and(|n| n == "worktrees")
                        && parent.file_name().is_some_and(|n| n == ".git")
                    {
                        return parent.parent().map_or_else(|| d.to_path_buf(), Path::to_path_buf);
                    }
                    cur = parent;
                }
                return d.to_path_buf();
            }
            Err(_) => {}
        }
    }
    repo.to_path_buf()
}

/// The folder name Claude Code gives a project under `<config>/projects/`: every character that is not a letter or a
/// digit becomes `-` (inferred from the folders it writes; the docs do not state the rule).
fn project_dir_name(root: &Path) -> String {
    root.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// The auto-memory index, `MEMORY.md`, which loads at startup (topic files beside it load only when read).
pub(crate) fn memory_index(b: &mut Build, p: &Paths) {
    b.note_not_modelled(
        "auto memory: the project folder name is inferred (non-alphanumerics become -); trust rules for a repo's autoMemoryDirectory",
    );
    let off_env = b.ctx.var("CLAUDE_CODE_DISABLE_AUTO_MEMORY").is_some_and(|v| v != "0" && v != "false");
    let off_setting = super::settings::resolved(b, "autoMemoryEnabled") == Some(&Value::Bool(false));
    if off_env || off_setting {
        return;
    }
    let dir = match super::settings::resolved(b, "autoMemoryDirectory").and_then(Value::as_str) {
        Some(d) if d.starts_with("~/") => b.ctx.home.join(&d[2..]),
        Some(d) if Path::new(d).has_root() => PathBuf::from(d),
        _ => {
            let name = match b.ctx.var("CLAUDE_CODE_PROJECT_DIR_NAME") {
                Some(n) if !n.contains(['/', '\\']) && n != ".." => n.to_string(),
                _ => project_dir_name(&git_root(b, &p.repo)),
            };
            p.cfg.join("projects").join(name).join("memory")
        }
    };
    let file = dir.join("MEMORY.md");
    let Ok(text) = b.fs().read_to_string(&file) else { return };
    let mut loaded: String = text.split_inclusive('\n').take(MEMORY_LINES).collect();
    if loaded.len() > MEMORY_BYTES {
        let mut cut = MEMORY_BYTES;
        while !loaded.is_char_boundary(cut) {
            cut -= 1;
        }
        loaded.truncate(cut);
    }
    let name = b.display(&file);
    let mut it = b.item(
        "user",
        &name,
        ItemDetail::Instruction(InstructionAttrs::default()),
        LoadWhen::Startup,
        calc(char_len(&loaded)),
    );
    it.origin.file = Some(name);
    it.load.content_hash = Some(sha256_hex(&loaded));
    b.push(it);
}

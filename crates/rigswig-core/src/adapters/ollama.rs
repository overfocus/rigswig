//! The Ollama adapter. Every rule it models is in `docs/adapters/ollama.md`, with the docs it comes from.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapter::{Adapter, Ctx, Detection};
use crate::adapters::build::{Build, calc, char_len};
use crate::fs::Fs;
use crate::model::*;
use crate::read::json::read_json;
use crate::read::redact::{printable, sha256_hex};

pub const ID: &str = "ollama";
pub const ADAPTER_VERSION: &str = "0.1.0";
/// The Ollama release whose documents this adapter follows (read 2026-11-06).
const TOOL_VERSION: &str = "0.40.13";
const SYSTEM_MEDIA_TYPE: &str = "application/vnd.ollama.image.system";

pub struct Ollama;

/// The paths, resolved once: the model store and the Ollama home.
struct Paths {
    models: PathBuf,
    ollama_home: PathBuf,
}

impl Paths {
    fn new(ctx: &Ctx) -> Self {
        let home = ctx.home.clone();
        let ollama_home = home.join(".ollama");
        let models = match ctx.var("OLLAMA_MODELS") {
            Some(d) => expand(&home, d),
            None => match ctx.os {
                // The documented Linux default is the systemd service's store; a manually run server keeps
                // models in ~/.ollama/models, which is used when the systemd store is not there (inferred).
                Os::Linux => {
                    let system = PathBuf::from("/usr/share/ollama/.ollama/models");
                    if ctx.fs.exists(&system) { system } else { ollama_home.join("models") }
                }
                _ => ollama_home.join("models"),
            },
        };
        Self { models, ollama_home }
    }
}

/// `~/x` becomes `<home>/x`; a bare relative value is taken against the home folder (inferred).
fn expand(home: &Path, value: &str) -> PathBuf {
    let v = value.trim();
    match v.strip_prefix("~/").or_else(|| v.strip_prefix("~\\")) {
        Some(rest) => home.join(rest),
        None if v == "~" => home.to_path_buf(),
        None => {
            let p = PathBuf::from(v);
            if p.is_absolute() { p } else { home.join(p) }
        }
    }
}

/// `somehost:11434`, user-free: anything before an `@` (credentials) is dropped, so only the host shows.
fn host_only(value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() || v.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let host = v.rsplit_once('@').map_or(v, |(_, h)| h);
    Some(printable(host))
}

struct Run<'a, 'b> {
    b: &'b mut Build<'a>,
    p: Paths,
}

impl Adapter for Ollama {
    fn id(&self) -> &'static str {
        ID
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &[
            "OLLAMA_MODELS",
            "OLLAMA_HOST",
            "OLLAMA_CONTEXT_LENGTH",
            "OLLAMA_KEEP_ALIVE",
            "OLLAMA_MAX_LOADED_MODELS",
            "OLLAMA_NUM_PARALLEL",
            "OLLAMA_NO_CLOUD",
        ]
    }

    fn detect(&self, ctx: &Ctx) -> Detection {
        let p = Paths::new(ctx);
        let checks = [
            (ctx.fs.exists(&p.models), "model store", 100),
            (ctx.fs.exists(&p.ollama_home), "Ollama home", 50),
            (ctx.fs.exists(&p.ollama_home.join("server.json")), "server.json", 30),
            (ctx.fs.exists(&p.ollama_home.join("launch")), "launch integrations", 20),
            (ctx.fs.exists(&p.ollama_home.join("config.json")), "config.json", 10),
            (!ctx.env.is_empty(), "OLLAMA_* variables", 40),
        ];
        let mut score = 0;
        let mut evidence = Vec::new();
        for (found, label, s) in checks {
            if found {
                score = score.max(s);
                evidence.push(label.to_string());
            }
        }
        Detection { score, evidence }
    }

    fn resolve(&self, ctx: &Ctx) -> Harness {
        let mut b = Build::new(ctx, ID);
        b.detected_by = self.detect(ctx).evidence;
        let p = Paths::new(ctx);
        b.add_layer(
            "session",
            LayerKind::Session,
            "server environment (the OLLAMA_* variables the caller passed)",
            None,
            !ctx.env.is_empty(),
            LayerMode::Default,
        );
        let home_present = ctx.fs.exists(&p.ollama_home);
        b.add_layer(
            "user",
            LayerKind::User,
            "the Ollama home",
            Some(b.display(&p.ollama_home)),
            home_present,
            LayerMode::Default,
        );
        let mut run = Run { b: &mut b, p };
        run.settings();
        run.models();

        // Nothing of Ollama's own loads before your first line: what a session carries is the chosen model's
        // System text (an on-invoke item), assembled by its template.
        run.b.not_measured.push(NotMeasured {
            what: "the assembled prompt of the session's chosen model (its template joins the System text into the prompt)".into(),
            why: "rigswig measures the System directive's text, not the template that renders it".into(),
        });
        run.b.not_measured_common();
        for note in [
            "each model's parameters (the params blobs, num_ctx among them) and the GGUF-side context-window defaults",
            "the chat templates' directives and variables (only the System directive's text is measured)",
            "models under an OLLAMA_HOST server elsewhere (rigswig reads only the local model store; the default context window without OLLAMA_CONTEXT_LENGTH is 4096 tokens)",
            "Ollama's integrations config (config.json, which models each integration is onboarded to) and its cloud models",
            "the settings each `ollama launch <harness>` integration writes (they configure other harnesses; a launched harness is audited by its own adapter)",
            "the tool schemas and system prompt of whatever client harness is pointed at the server",
            "server.json keys other than disable_ollama_cloud",
        ] {
            run.b.note_not_modelled(note);
        }
        let repo_label = ctx.repo.file_name().map_or_else(|| ".".into(), |n| n.to_string_lossy().into_owned());
        b.finish(ADAPTER_VERSION, Some(TOOL_VERSION), printable(&repo_label))
    }
}

impl Run<'_, '_> {
    fn fs(&self) -> &dyn Fs {
        self.b.fs()
    }

    fn ctx_var(&self, name: &str) -> Option<String> {
        self.b.ctx.env.get(name).map(|v| printable(v))
    }

    /// A whole-number setting from the server's environment.
    fn env_setting(&mut self, name: &str, env: &str) {
        let Some(v) = self.ctx_var(env) else { return };
        let value = v.parse::<i64>().map(|n| Value::Number(n.into())).unwrap_or(Value::String(v.clone()));
        self.b.settings.insert(
            name.into(),
            Decision {
                value,
                source: Some("session".into()),
                trail: vec![TrailStep {
                    layer: "session".into(),
                    value: Value::String(v),
                    note: Some(env.to_string()),
                }],
            },
        );
    }

    /// The server settings: OLLAMA_* environment variables, plus the cloud toggle in server.json.
    fn settings(&mut self) {
        self.env_setting("contextLength", "OLLAMA_CONTEXT_LENGTH");
        self.env_setting("keepAlive", "OLLAMA_KEEP_ALIVE");
        self.env_setting("maxLoadedModels", "OLLAMA_MAX_LOADED_MODELS");
        self.env_setting("numParallel", "OLLAMA_NUM_PARALLEL");
        if let Some(v) = self.ctx_var("OLLAMA_HOST").and_then(|h| host_only(&h)) {
            self.b.settings.insert(
                "host".into(),
                Decision {
                    value: Value::String(v.clone()),
                    source: Some("session".into()),
                    trail: vec![TrailStep {
                        layer: "session".into(),
                        value: Value::String(v),
                        note: Some("OLLAMA_HOST (host only; no port, user info or path)".into()),
                    }],
                },
            );
        }
        // The cloud features: OLLAMA_NO_CLOUD speaks from the launch environment, server.json from the home.
        let mut trail: Vec<TrailStep> = Vec::new();
        let mut disabled: Option<bool> = None;
        if let Some(v) = self.ctx_var("OLLAMA_NO_CLOUD") {
            disabled = Some(!matches!(v.as_str(), "0" | "false" | "no" | "off" | ""));
            trail.push(TrailStep {
                layer: "session".into(),
                value: Value::String(v),
                note: Some("OLLAMA_NO_CLOUD (truthy turns cloud off)".into()),
            });
        }
        let server_json = read_json(self.fs(), &self.p.ollama_home.join("server.json"));
        if server_json.value().and_then(|v| v.get("disable_ollama_cloud")) == Some(&Value::Bool(true)) {
            trail.push(TrailStep {
                layer: "user".into(),
                value: Value::Bool(true),
                note: Some("server.json disable_ollama_cloud".into()),
            });
            disabled = disabled.or(Some(true));
        }
        if let Some(off) = disabled {
            let state = if off { "disabled" } else { "enabled" };
            let source = trail.first().map(|t| t.layer.clone());
            self.b.settings.insert("cloud".into(), Decision { value: Value::String(state.into()), source, trail });
        } // nothing turns it either way: the tool's default applies
    }

    /// One item per local model whose manifest sits in the model store: its System directive, measured.
    fn models(&mut self) {
        let manifests = self.p.models.join("manifests");
        let mut leaves: Vec<(PathBuf, Vec<String>)> = Vec::new();
        self.walk_manifests(&manifests, &manifests, 0, &mut leaves);
        for (file, parts) in leaves {
            if parts.len() != 4 {
                continue; // registry/namespace/model/tag
            }
            let [registry, namespace, model, tag] = parts.as_slice() else { continue };
            let j = read_json(self.fs(), &file);
            let Some(spec) = j.value() else {
                self.b.warn("malformed-manifest", "the model manifest is not valid JSON, skipped", Some(&file));
                continue;
            };
            // The displayed name: registry models keep the library form (`model:tag`); a namespace marks a
            // user-created model (`namespace/model:tag`). The registry label itself is not shown.
            let name =
                if namespace == "library" { format!("{model}:{tag}") } else { format!("{namespace}/{model}:{tag}") };
            let mut system_text: Option<String> = None;
            let mut blob_missing = false;
            for layer in spec.get("layers").and_then(Value::as_array).unwrap_or(&Vec::new()) {
                if layer.get("mediaType").and_then(Value::as_str) == Some(SYSTEM_MEDIA_TYPE) {
                    let digest = layer.get("digest").and_then(Value::as_str).unwrap_or("").replace(':', "-");
                    let blob = self.p.models.join("blobs").join(&digest);
                    match self.fs().read_to_string(&blob) {
                        Ok(text) => system_text = Some(text),
                        Err(_) => blob_missing = true,
                    }
                }
            }
            // Parameters (num_ctx among them) and templates live in their own blobs and are not modelled.
            let chars = system_text.as_deref().map(char_len).unwrap_or(0);
            let mut it = self.b.item(
                "user",
                &name,
                ItemDetail::Instruction(InstructionAttrs::default()),
                LoadWhen::OnInvoke,
                calc(chars),
            );
            let shown = format!("~/.ollama/models/manifests/{}", printable(&name));
            let _ = shown;
            let path = format!("{}/{}/{}/{}", self.b.display(&self.p.models), "manifests", registry, namespace);
            it.origin.file = Some(format!("{path}/{model}/{tag}"));
            it.load.content_hash = system_text.as_deref().map(sha256_hex);
            if blob_missing {
                self.b.warn(
                    "blob-missing",
                    format!("the System blob of {name} could not be read; it is counted as absent"),
                    Some(&file),
                );
                Build::disable(&mut it);
            }
            self.b.push(it);
        }
        if !self.fs().exists(&manifests) {
            self.b.note_not_modelled(
                "no model store was found at OLLAMA_MODELS (or the OS default), so no local model is listed here",
            );
        }
    }

    /// Every manifest file under the model store's `manifests/` tree, with its path parts.
    fn walk_manifests(&self, root: &Path, dir: &Path, depth: usize, out: &mut Vec<(PathBuf, Vec<String>)>) {
        if depth > 5 {
            return;
        }
        for e in self.fs().read_dir(dir).unwrap_or_default() {
            let name = e.name.clone();
            if name.starts_with('.') {
                continue;
            }
            let sub = dir.join(&name);
            match e.kind {
                crate::fs::EntryKind::Dir => self.walk_manifests(root, &sub, depth + 1, out),
                crate::fs::EntryKind::File | crate::fs::EntryKind::Symlink => {
                    let rel = sub.strip_prefix(root).unwrap_or(&sub).to_string_lossy().replace('\\', "/");
                    out.push((sub, rel.split('/').map(String::from).collect()));
                }
            }
        }
    }
}

//! The Ollama adapter on invented setups, one rule per test (docs/adapters/ollama.md). Every output keeps the
//! contract.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::Ollama;
use rigswig_core::check::contract_errors;
use rigswig_core::fs::MemFs;
use rigswig_core::model::*;
use rigswig_core::read::redact::sha256_hex;

const HOME: &str = "/home/dev";
const REPO: &str = "/home/dev/work/shop/app";
const MH: &str = "/home/dev/.ollama/models";

fn run_env(env: &[(&str, &str)], fs: &MemFs) -> Harness {
    let ctx = Ctx {
        fs,
        env: env.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect::<BTreeMap<_, _>>(),
        home: PathBuf::from(HOME),
        repo: PathBuf::from(REPO),
        os: Os::Linux,
    };
    let h = Ollama.resolve(&ctx);
    assert_eq!(contract_errors(&h), Vec::<String>::new(), "contract");
    h
}

fn run(fs: &MemFs) -> Harness {
    run_env(&[], fs)
}

/// An empty setup: the home exists, no models yet.
fn base() -> MemFs {
    MemFs::new().dir(REPO).dir(format!("{MH}/manifests"))
}

fn manifest(registry: &str, namespace: &str, model: &str, tag: &str) -> String {
    format!("{MH}/manifests/{registry}/{namespace}/{model}/{tag}")
}

fn system_layer(digest: &str) -> String {
    let t = "application/vnd.ollama.image.system";
    format!("{{\"schemaVersion\":2,\"layers\":[{{\"mediaType\":\"{t}\",\"digest\":\"sha256:{digest}\"}}]}}")
}

fn item<'h>(h: &'h Harness, id: &str) -> &'h Item {
    h.items
        .iter()
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no item {id}; have {:#?}", h.items.iter().map(|i| &i.id).collect::<Vec<_>>()))
}

#[test]
fn every_local_model_is_listed_by_its_chat_name() {
    let fs = base()
        .file(manifest("registry.ollama.ai", "library", "mistral-small", "latest"), "{\"layers\":[]}")
        .file(manifest("registry.ollama.ai", "library", "deepseek-r1", "8b"), system_layer("aaa1"))
        .file(format!("{MH}/blobs/sha256-aaa1"), "You are a helpful coding assistant.")
        .file(manifest("registry.ollama.ai", "kevinferron", "tiny-cat", "v3"), system_layer("bbb2"))
        .file(format!("{MH}/blobs/sha256-bbb2"), "You are a tiny cat.");
    let h = run(&fs);
    let listed = item(&h, "ollama:instruction:user:mistral-small:latest");
    assert_eq!(listed.load.when, LoadWhen::OnInvoke, "the model's System prompt joins its own session");
    assert_eq!(listed.load.chars.value, 0, "no System directive: nothing to say");
    let with_system = item(&h, "ollama:instruction:user:deepseek-r1:8b");
    assert_eq!(with_system.load.chars.value, 35);
    assert_eq!(
        with_system.load.content_hash.as_deref(),
        Some(sha256_hex("You are a helpful coding assistant.").as_str())
    );
    let namespaced = item(&h, "ollama:instruction:user:kevinferron/tiny-cat:v3");
    assert_eq!(namespaced.load.chars.value, 19, "a user-created model keeps its namespace in the name");
    assert_eq!(h.context.chars.value, 0, "Ollama itself loads nothing before your first line");

    let empty = run(&base());
    assert!(ids(&empty, "instruction").is_empty());
}

#[test]
fn launch_integrations_keep_local_and_cloud_model_choices() {
    let fs = base().file(
        format!("{HOME}/.ollama/config.json"),
        r#"{"integrations":{"hermes":{"models":["gemma4:31b","kimi-k3.1-pro:cloud","gpt-oss:120b-cloud"],"onboarded":true}}}"#,
    );
    let h = run(&fs);
    let integration = &h.settings["integrations"].value["hermes"];
    assert_eq!(integration["onboarded"], true);
    assert_eq!(integration["models"][0], serde_json::json!({"name":"gemma4:31b","execution":"local"}));
    assert_eq!(integration["models"][1], serde_json::json!({"name":"kimi-k3.1-pro:cloud","execution":"cloud"}));
    assert_eq!(integration["models"][2], serde_json::json!({"name":"gpt-oss:120b-cloud","execution":"cloud"}));
    assert_eq!(h.settings["integrations"].source.as_deref(), Some("user"));
    for model in ["gemma4:31b", "kimi-k3.1-pro:cloud", "gpt-oss:120b-cloud"] {
        let it = item(&h, &format!("ollama:instruction:user:{model}"));
        assert_eq!(it.load.when, LoadWhen::OnInvoke);
        assert_eq!(it.load.chars.confidence, Confidence::Unknown);
        assert_eq!(it.origin.file.as_deref(), Some("~/.ollama/config.json"));
    }
}

#[test]
fn an_invalid_launch_model_is_an_incomplete_read() {
    let fs = base().file(
        format!("{HOME}/.ollama/config.json"),
        r#"{"integrations":{"hermes":{"models":[true,"gemma4:31b"],"onboarded":"yes"}}}"#,
    );
    let h = run(&fs);
    assert_eq!(h.warnings.iter().filter(|w| w.code == "incomplete-launch-model").count(), 2);
    assert_eq!(h.settings["integrations"].value["hermes"]["onboarded"], false);
    assert_eq!(h.settings["integrations"].value["hermes"]["models"].as_array().unwrap().len(), 1);
}

#[test]
fn a_malformed_launch_registry_is_an_incomplete_read() {
    let fs = base().file(format!("{HOME}/.ollama/config.json"), r#"{"integrations":[]}"#);
    let h = run(&fs);
    assert!(h.warnings.iter().any(|w| w.code == "incomplete-launch-model"));
    assert!(!h.settings.contains_key("integrations"));
}

#[test]
fn server_settings_come_from_the_environment() {
    let fs = base();
    let h = run_env(
        &[
            ("OLLAMA_CONTEXT_LENGTH", "8192"),
            ("OLLAMA_KEEP_ALIVE", "-1"),
            ("OLLAMA_NUM_PARALLEL", "4"),
            ("OLLAMA_HOST", "user:pw@0.0.0.0:11434"),
        ],
        &fs,
    );
    assert_eq!(h.settings["contextLength"].value.as_i64(), Some(8192));
    assert_eq!(h.settings["contextLength"].source.as_deref(), Some("session"));
    assert_eq!(h.settings["keepAlive"].value.as_i64(), Some(-1), "a bare number is kept as-is");
    assert_eq!(h.settings["numParallel"].value.as_i64(), Some(4));
    assert_eq!(h.settings["host"].value, "0.0.0.0:11434", "user info is dropped");
    let trail: Vec<&str> = h.settings["contextLength"].trail.iter().map(|t| t.layer.as_str()).collect();
    assert_eq!(trail, ["session"]);

    let quiet = run(&base());
    assert!(!quiet.settings.contains_key("contextLength"), "the default is left unexplained");
}

#[test]
fn cloud_is_turned_off_by_server_json_or_the_environment() {
    let json_off = base().file(format!("{HOME}/.ollama/server.json"), "{\"disable_ollama_cloud\": true}");
    let h = run(&json_off);
    assert_eq!(h.settings["cloud"].value, "disabled");
    assert_eq!(h.settings["cloud"].source.as_deref(), Some("user"));

    let json_on = base().file(format!("{HOME}/.ollama/server.json"), "{\"disable_ollama_cloud\": false}");
    assert!(!run(&json_on).settings.contains_key("cloud"), "explicitly enabled is the default; nothing to say");

    let env_off = run_env(&[("OLLAMA_NO_CLOUD", "1")], &json_on);
    assert_eq!(env_off.settings["cloud"].value, "disabled");
    assert_eq!(env_off.settings["cloud"].source.as_deref(), Some("session"));

    let env_false = run_env(&[("OLLAMA_NO_CLOUD", "0")], &json_on);
    assert_eq!(env_false.settings["cloud"].value, "enabled", "an explicit falsy value is still somebody's say-so");
    assert_eq!(env_false.settings["cloud"].source.as_deref(), Some("session"));
}

#[test]
fn ollama_models_moves_the_store_and_detect_follows() {
    let store = MemFs::new()
        .dir(REPO)
        .dir("/srv/ollama/manifests/registry.ollama.ai/library/llama3.2")
        .file("/srv/ollama/manifests/registry.ollama.ai/library/llama3.2/3b", "{\"layers\":[]}");
    let h = run_env(&[("OLLAMA_MODELS", "/srv/ollama")], &store);
    assert!(h.items.iter().any(|i| i.name == "llama3.2:3b"), "the custom store is read");

    let env = BTreeMap::new();
    let mk = |fs: &MemFs| {
        let ctx = Ctx { fs, env: env.clone(), home: PathBuf::from(HOME), repo: PathBuf::from(REPO), os: Os::Linux };
        Ollama.detect(&ctx).score
    };
    let none = MemFs::new();
    assert_eq!(mk(&none), 0);
    let home_only = MemFs::new().dir(REPO).dir("/home/dev/.ollama");
    assert_eq!(mk(&home_only), 45, "the tool's home alone, without a model store");
    let with_models = base().file(manifest("registry.ollama.ai", "library", "m", "latest"), "{\"layers\":[]}");
    assert_eq!(mk(&with_models), 50, "a model store is home-folder evidence: never over 50");
}

fn ids(h: &Harness, kind: &str) -> Vec<String> {
    h.items.iter().filter(|i| i.detail.kind() == kind).map(|i| i.id.clone()).collect()
}

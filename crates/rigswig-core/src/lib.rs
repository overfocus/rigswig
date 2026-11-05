//! The core of rigswig: a harness-neutral data model ([`model`]) and one adapter per harness ([`adapter`]).
//!
//! Rules this crate keeps:
//! - no network, no process spawning, no writes: adapters read through an injected, read-only file system ([`fs`]);
//! - secrets (MCP `env`, `headers`, `args`, URL paths, inline hook code) never enter the model;
//! - every figure says whether it is native, calculated, estimated or unknown;
//! - every output passes [`check::contract_errors`].

pub mod adapter;
pub mod adapters;
pub mod check;
pub mod fs;
pub mod model;
pub mod read;

/// The crate version, shared by the CLI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The JSON Schema of [`model::Harness`], as checked in at `schema/harness.schema.json`.
pub fn json_schema() -> serde_json::Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(model::Harness)).expect("a schema serialises");
    if let Some(obj) = schema.as_object_mut() {
        obj.insert("$id".into(), "https://github.com/overfocus/rigswig/blob/main/schema/harness.schema.json".into());
        obj.insert("$comment".into(), format!("schemaVersion {}", model::SCHEMA_VERSION).into());
    }
    schema
}

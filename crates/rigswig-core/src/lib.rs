//! The core of rigswig: a harness-neutral data model and one adapter per harness (Claude Code, Codex, ...).
//!
//! Nothing is implemented yet; see `docs/ROADMAP.md`. Rules this crate will keep:
//! - no network, no process spawning, no writes: adapters read through an injected file system and return data;
//! - secrets (MCP `env`, `headers`, `args`, URL paths, inline hook code) never enter the model;
//! - every figure says whether it is native, calculated, estimated or unknown.

/// The crate version, shared by the CLI and the JSON output.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

//! Harness adapters. Each reads one tool's files into the shared model.

pub mod build;
pub mod claude_code;
pub mod codex;
pub mod hermes;
pub mod ollama;
pub mod shared;

pub use claude_code::ClaudeCode;
pub use codex::Codex;
pub use hermes::Hermes;
pub use ollama::Ollama;

use crate::adapter::Adapter;

/// Every adapter rigswig ships, in the order they are offered.
pub fn all() -> Vec<Box<dyn Adapter>> {
    vec![Box::new(ClaudeCode), Box::new(Codex), Box::new(Hermes), Box::new(Ollama)]
}

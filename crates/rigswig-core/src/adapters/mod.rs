//! Harness adapters. Each reads one tool's files into the shared model.

pub mod build;
pub mod claude_code;
pub mod codex;
pub mod shared;

pub use claude_code::ClaudeCode;
pub use codex::Codex;

use crate::adapter::Adapter;

/// Every adapter rigswig ships, in the order they are offered.
pub fn all() -> Vec<Box<dyn Adapter>> {
    vec![Box::new(ClaudeCode), Box::new(Codex)]
}

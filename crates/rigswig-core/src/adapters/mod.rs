//! Harness adapters. Each reads one tool's files into the shared model.

pub mod build;
pub mod claude_code;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod deepseek;
pub mod devin;
pub mod dsh;
pub mod gemini;
pub mod hermes;
pub mod hf;
pub mod ollama;
pub mod omp;
pub mod pi;
pub mod shared;

pub use claude_code::ClaudeCode;
pub use codex::Codex;
pub use copilot::Copilot;
pub use cursor::Cursor;
pub use deepseek::DeepSeekCli;
pub use devin::Devin;
pub use dsh::Dsh;
pub use gemini::Gemini;
pub use hermes::Hermes;
pub use hf::Hf;
pub use ollama::Ollama;
pub use omp::Omp;
pub use pi::Pi;

use crate::adapter::Adapter;

/// Every adapter rigswig ships, in the order they are offered.
pub fn all() -> Vec<Box<dyn Adapter>> {
    vec![
        Box::new(ClaudeCode),
        Box::new(Codex),
        Box::new(Copilot),
        Box::new(Cursor),
        Box::new(Gemini),
        Box::new(Hf),
        Box::new(Hermes),
        Box::new(Ollama),
        Box::new(Devin),
        Box::new(Dsh),
        Box::new(DeepSeekCli),
        Box::new(Pi),
        Box::new(Omp),
    ]
}

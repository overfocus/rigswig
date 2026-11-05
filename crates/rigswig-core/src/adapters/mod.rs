//! Harness adapters. Each reads one tool's files into the shared model.

pub mod build;
pub mod claude_code;

pub use claude_code::ClaudeCode;

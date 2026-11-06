# rigswig-core

The library behind [rigswig](https://github.com/overfocus/rigswig): a harness-neutral data model of what a
coding-agent session loads (instruction files, agents, skills, commands, hooks, MCP servers, permissions, settings,
with the layer each came from and a context estimate), one adapter per harness (Claude Code, OpenAI Codex CLI, Hermes
Agent, Ollama), and the audit.

It does no I/O of its own: adapters read through an injected, read-only file system (`fs::Fs`), so the same code runs
on disk, in memory (`fs::MemFs`) or over an archive, and prints, spawns and fetches nothing. The JSON form of the model
is described by `schema/harness.schema.json` in the repo, versioned separately from this crate.

Most people want the `rigswig` command instead: `cargo install rigswig`.

Licensed under Apache-2.0.

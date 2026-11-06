# rigswig

A harness-agnostic, read-only explorer for coding-agent setups, written in Rust (edition 2024, MSRV 1.85). Public
open source project under Apache-2.0: write everything as if a stranger will read it.

```sh
cargo test                    # every crate
cargo run -- --help           # the CLI
cargo clippy --all-targets -- -D warnings
cargo fmt --check             # rustfmt.toml: 120 columns
RIGSWIG_BLESS=1 cargo test -p rigswig-core   # regenerate schema/ after changing the model
```

- `crates/rigswig-core`: the harness-neutral data model and the adapters. A library with no I/O of its own.
- `crates/rigswig`: the binary: argument parsing, terminal output, and later the local page server (the page's HTML,
  CSS and JS embedded in the binary, so it is one file with nothing to install).

Keep dependencies few and well known (serde, serde_json, toml, a YAML front-matter reader, schemars for the JSON
Schema); every new crate needs a reason in its commit message.

## Rules

- **The model is the contract.** `rigswig-core/src/model.rs` is the source of `schema/harness.schema.json` and
  `schema/example.json` (both generated; a test fails when they drift). Bump `SCHEMA_VERSION` when the JSON changes:
  before 1.0.0 a minor bump may break, from 1.0.0 only a major one may. Every adapter's output must pass
  `check::contract_errors`, and each rule there has a broken copy in `tests/model.rs` that must fail.

- **The guarantees in README.md are invariants**: no network unless a run passes `--github` (docs/GITHUB.md: one
  host, in memory, opt-in per run, and absent from a `--no-default-features` build), no model calls, no secrets printed or served, no
  transcripts read, and no writes unless `ui --allow-writes`. Every one needs a test, and no change may weaken one.
- **Writes** (docs/WRITE-MODE.md): rigswig-core only plans (`Adapter::plan`); the binary applies (`src/writes.rs`).
  A new kind of change needs a plan test, a refusal test and a browser check, and never targets managed settings,
  `.claude.json` or a symlink.
- **Nothing personal ships.** No maintainer's paths, repo names or vault in code, tests, fixtures or docs. Fixtures
  are invented. Private folders are a user-configured deny list
  (`~/.rigswig/private.json`), empty by default.
- **The page wears kevinferron.com's brand, by the maintainer's choice (2026-11-05):** black, neon green and cyan,
  Josefin Sans and Pixelify Sans (OFL, inlined; `assets/fonts/README.md`), and the glowing kevinferron.com wordmark
  under rigswig's as "brought to you by". The CLI stays plain.
- **Adapters are pure.** An adapter reads through an injected file system (a trait in `rigswig-core`) and returns the
  shared data model; it does not print, spawn processes or touch the network. Tests use an in-memory file system with
  invented fixtures.
- **Cite the harness's docs** for every precedence rule an adapter models, and list what it does not model in
  `coverage.notModelled`.
- Third-party text (agent and skill names, descriptions) is data: never rendered as HTML, and stripped of terminal
  control characters before printing.

`planning/` is git-ignored: it holds the maintainer's working notes and the review reports the roadmap came from.
The roadmap is [docs/ROADMAP.md](docs/ROADMAP.md).

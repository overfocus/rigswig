# rigswig

A harness-agnostic, read-only explorer for coding-agent setups. Plain Node ESM (Node 20 or later), no runtime
dependencies. Public open source project under Apache-2.0: write everything as if a stranger will read it.

```sh
npm test            # node --test
node bin/rigswig.mjs --help
```

## Rules

- **The guarantees in README.md are invariants**: no network, no model calls, no writes to configuration, no secrets
  printed or served, no transcripts read. Every one needs a test, and no change may weaken one.
- **Nothing personal ships.** No maintainer's paths, repo names, vault or brand in code, tests, fixtures or docs.
  Fixtures are invented. Private folders are a user-configured deny list, empty by default.
- **Adapters are pure.** An adapter reads through an injected file system and returns the shared data model; it does
  not print, spawn processes or touch the network.
- **Cite the harness's docs** for every precedence rule an adapter models, and list what it does not model in
  `coverage.notModelled`.
- Third-party text (agent and skill names, descriptions) is data: never rendered as HTML, and stripped of terminal
  control characters before printing.

`planning/` is git-ignored: it holds the maintainer's working notes and the review reports the roadmap came from.
The roadmap is [docs/ROADMAP.md](docs/ROADMAP.md).

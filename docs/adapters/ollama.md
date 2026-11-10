# The Ollama adapter: the rules it models

Every rule below is from Ollama's documentation at docs.ollama.com, read on 2026-11-10, against the installed
release 0.40.16. Where the docs are silent and the adapter has to choose, the rule is marked **inferred** and listed
in the output's `coverage.notModelled`. When the docs change, change this file and the adapter together.

Ollama is a model server, not a coding-agent harness: nothing of its own loads before your first line, so the
context total is whatever the client harness brings (a different adapter's job). What rigswig can see on disk is the
local model store, the server's environment, the cloud toggle in `server.json`, and the integrations selected by
`ollama launch`. A session pointed at Ollama carries the chosen model's `SYSTEM` text when that text exists locally,
assembled by that model's template into its prompt.

## Layers

| Layer | Rank (lower wins) | Files |
|---|---|---|
| `session` | 0 | the `OLLAMA_*` variables rigswig reads (the server's launch environment) |
| `user` | 1 | the Ollama home: the model store, `config.json`, `server.json` |

The model store is `OLLAMA_MODELS`, or the OS default ([where models are
stored](https://docs.ollama.com/faq)): `~/.ollama/models` on macOS and Windows, `/usr/share/ollama/.ollama/models`
on Linux (the systemd service); on Linux, when the systemd store is absent, `~/.ollama/models` is read instead
— a manually run server keeps models there (**inferred**).

## Settings

- `contextLength` (`OLLAMA_CONTEXT_LENGTH`), `keepAlive` (`OLLAMA_KEEP_ALIVE`), `maxLoadedModels`
  (`OLLAMA_MAX_LOADED_MODELS`), `numParallel` (`OLLAMA_NUM_PARALLEL`): numbers when they parse as ones, strings
  otherwise. The default context window (4,096 tokens) is left unexplained, `null`-like, exactly as Codex leaves
  its unset settings.
- `host` (`OLLAMA_HOST`): the host only (and port, for a `host:port` value) — no user info, path or query.
- `cloud`: `disabled` when `server.json` sets `disable_ollama_cloud: true`, or `OLLAMA_NO_CLOUD` is truthy
  (`1`, `true`, `yes`, `on`); `enabled` when a written value says the cloud stays on; nothing when nothing says
  anything ([configure without the cloud](https://docs.ollama.com/faq)). `OLLAMA_NO_CLOUD` is the higher layer in
  the trail; both a truthy and a falsy value count as somebody's say-so.
- `integrations`: each harness in `config.json`, its `onboarded` flag, and its selected models, as recorded by
  [`ollama launch`](https://docs.ollama.com/integrations/hermes). Every model has `execution: local` or `cloud`,
  **inferred** from Ollama's `:cloud` / `-cloud` naming. A local endpoint is only the router: it never makes a cloud
  model local. Unknown integration keys are not copied, so this setting does not become a secrets surface.

## Models

Every manifest under `<store>/manifests/<registry>/<namespace>/<model>/<tag>` is a local model, named the way
`ollama run` names it: `model:tag` for the `library` models, `namespace/model:tag` otherwise (**inferred**: the
registry label is dropped; this matches the CLI's own display). Each is one item:

- an `instruction` whose text is the model's `SYSTEM` blob
  (`application/vnd.ollama.image.system` in `models/blobs/<digest>`), with `load.when: on-invoke` — it joins a
  session only when that model is chosen — and a character count and content hash of the system text. A model with
  no System blob is listed with a zero count.
- Every model selected by a launch integration is also listed when it has no local manifest, with unknown character
  confidence rather than an invented count. This covers cloud models and local selections not present in the store.
- Template blobs (`application/vnd.ollama.image.template`) and parameter blobs (num_ctx among them) are not
  modelled: the template joins the system text into the prompt, and per-model parameters are the model's own
  numbers, so both are named in `coverage.notModelled`.

## Config on disk that is not modelled

- `config.json` integration keys other than `models` and `onboarded`, and execution metadata beyond the local/cloud
  name inference.
- `launch/<name>/`: provider or web-search settings written for the launched harness. Those settings configure other
  harnesses; a launched harness is audited by its own adapter. Custom endpoints and fallback-provider routing are
  not resolved by the Ollama adapter.
- `server.json` keys other than `disable_ollama_cloud`, and every remote `OLLAMA_HOST` instance: rigswig contacts
  no server and reads no remote store.

## Env vars read

`OLLAMA_MODELS`, `OLLAMA_HOST`, `OLLAMA_CONTEXT_LENGTH`, `OLLAMA_KEEP_ALIVE`, `OLLAMA_MAX_LOADED_MODELS`,
`OLLAMA_NUM_PARALLEL`, `OLLAMA_NO_CLOUD`. No network: only files under the model store and the Ollama home.

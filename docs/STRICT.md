# Strict completeness checks

`rigswig check --strict` asks a narrower question than `rigswig check --no-warnings`: did the adapter have to leave
part of the setup unread, drop something it could not model, or guess between resolutions? It is for automation that
must not turn a partial reading into a green check.

```sh
rigswig check --strict
rigswig check --strict --max-tokens 40k --json
```

The `strict` row in the JSON verdict passes only when no `incomplete-*` warning is present. A failure exits 1, like a
budget failure. An adapter which has not opted into the completeness contract is refused before that adapter resolves
the setup: the CLI exits 2, and MCP returns a tool error. Candidate-harness detection still happens first. A
third-party adapter's default is therefore to fail closed, not to
look complete because it emitted no warnings. Every built-in adapter opts in through the shared classifier.

## What fails strict mode

The `incomplete-` warning namespace covers:

- malformed JSON, JSONC, TOML, YAML and model manifests;
- unsupported configuration constructs which had to be dropped, including agent front matter and YAML features;
- unreadable settings, packages, model system blobs, MCP bundles and enabled plugin content;
- present plain-text files which fail with permission, I/O, broken-link or UTF-8 errors (`incomplete-read-failed`);
  an absent optional file is not an incomplete read;
- incomplete agents, unresolved profiles or external directories, and setting patterns which were not applied;
- duplicate or otherwise genuinely ambiguous sources, transports and triggers;
- filesystem walks which stopped before discovering every item.

Each warning keeps its evidence; when it has a displayed path, that path is redacted under the ordinary output rules.

## What does not fail strict mode

`coverage.notModelled` is not a failure by itself. It documents boundaries every honest adapter has, such as settings
which exist only in a vendor service. Native truncation, documented precedence and deduplication, policy refusal,
pending approval, and rigswig's intentional refusal to follow a link outside its allowed roots are also not incomplete
reads. Advisory configuration risks such as an agent with every tool remain ordinary warnings.

Use `--no-warnings` when the policy is instead "no warnings of any kind". The flags compose: `--strict --no-warnings`
requires both complete resolution and a warning-free setup.

Strict mode does not prove that a harness's undocumented behavior is fully known, that unmeasured context is small,
or that a setup is safe. It proves only that the selected adapter reported no incomplete or ambiguous resolution under
this contract. Like every automated read, it uses repository evidence and never follows live-session activity.

MCP's `check` tool accepts the same `strict: true` argument and returns the same verdict rows.

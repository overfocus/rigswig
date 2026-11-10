# Check baselines: prevent new configuration debt

`rigswig check --baseline <file>` turns the ordinary CI check into a no-new-debt ratchet. Debt recorded in the
baseline is carried and reported; a new warning or a check which newly starts failing exits 1. Fixed debt is
reported as fixed and does not fail the run. This answers "did this change make the harness worse?" without forcing
an existing configuration to become spotless first.

Baselines do not change the guarantees: rigswig only reads the baseline. It never creates or updates one, writes
nothing outside `ui --allow-writes`, and uses repository evidence rather than live-session activity.

## Create and review a baseline

Run the same check options you intend to use in CI and save the JSON with your shell or CI system:

```sh
rigswig check --strict --max-tokens 40k --no-warnings --json > rigswig-baseline.json
```

The command exits 1 when the current configuration has debt; the JSON is still produced and can be reviewed as the
starting point. Review the file before committing it. It contains the harness id, adapter and assumed tool versions,
check rows, and warning code/message/file metadata. It does not contain session logs or message contents.

A baseline has `baselineVersion: 1`. Version 1 baselines are portable across checkout paths: the repository label is
informational, while the harness id must match. Adapter and tool versions are provenance, not a requirement to
regenerate the baseline after every rigswig upgrade.

## Enforce the ratchet

Use the same budgets and strict/no-warnings options in CI, plus the baseline:

```sh
rigswig check --strict --max-tokens 40k --no-warnings --baseline rigswig-baseline.json
```

- Existing warnings and failing checks are carried and allowed.
- Every new warning fails the ratchet, even without `--no-warnings`. This is what makes it a debt ratchet rather than
  only a budget comparison.
- A newly failing check fails. Adding another budget to an existing CI command therefore protects it immediately.
- A previously failing check must still be present. Dropping a failing budget, `strict`, or `no-warnings` row is a
  usage error (exit 2), not a way to escape the debt.
- Fixed warnings and newly passing checks are reported in `baseline.fixedWarnings` and `baseline.fixedFailures`.
- Refresh the baseline deliberately, with the same reviewed command used to create it. Do not regenerate it on every
  CI run, or the ratchet approves its own new debt.

The text output includes a `baseline` PASS/FAIL row. JSON additionally includes `newWarnings`, `newFailures`,
`carriedWarnings`, `carriedFailures`, `fixedWarnings`, and `fixedFailures`. `passed` and the process exit status
reflect the ratchet when `--baseline` is present; the individual rows still show which carried checks are failing.

## Warning identity and compatibility

Warnings are compared as multisets of `(code, file)`, so repeated warnings in one file still count separately while
message wording may improve between adapter versions without creating debt. File labels use rigswig's normal
repo-relative or home-relative display, not absolute checkout paths.

A missing or malformed baseline, unsupported `baselineVersion`, a different harness, an invalid warning/check shape,
or a dropped previously failing check exits 2. New debt exits 1. No new debt exits 0, including when the individual
rows show carried failures.

Baselines cover `check` warnings and verdict rows, not audit findings or token measurements which were not guarded
by a budget. Repository policy, explained suppressions, and SARIF remain separate roadmap work.

# Write mode

`rigswig ui --allow-writes` lets the page change a few settings. Without the flag the write routes do not exist and
the page cannot change anything.

## What it can change

| Harness | Change | Written to |
|---|---|---|
| Claude Code | a plugin on or off (`enabledPlugins["<name>@<marketplace>"]`) | the scope you pick: `.claude/settings.local.json` (this repo, you), `.claude/settings.json` (this repo, everyone: committed), `<config>/settings.json` (every repo, you) |
| Claude Code | a project `.mcp.json` server approved or disabled (`enabledMcpjsonServers`, `disabledMcpjsonServers`) | the same three files |

Never written: managed settings (your organisation's), `.claude.json` (Claude Code rewrites it itself, so a write
could collide with a running session), anything through a symlink, a settings file that is not valid JSON (fix it by
hand first). Codex is read-only for now: its `config.toml` needs an editor that keeps comments.

## How a change happens

1. **Plan** (`rigswig-core`, which has no way to write): the change is applied to a copy of the file in memory, and the
   whole setup is resolved again over that copy, so the page shows the exact file, the diff, the context budget before
   and after, and whether the change takes effect at all (a higher layer may still decide, and the page says so).
2. **Apply** (the binary): only if the file is byte for byte what the plan saw. The old file is saved to
   `~/.rigswig/backups/`, the new one is written to a temporary file beside it and renamed over it, so a crash leaves
   either the old file or the new one, never half of each.
3. **Undo**: the last change applied in this run of the page, only if the file is still exactly what rigswig wrote
   (otherwise restore it by hand from the backup). A file rigswig created is removed.

Changes take effect in the next session (or after `/reload-plugins`), never in a running one.

## The gate

On top of the page's own gate (127.0.0.1, the token, the exact `Host`), a write request must be a `POST` with the
token in an `X-Rigswig-Token` header (a cross-site form cannot set one), an exact `Origin`, a JSON body of at most
16 KB, and no cross-site `Sec-Fetch-Site`. The page's CSP allows `connect-src 'self'` only in write mode.

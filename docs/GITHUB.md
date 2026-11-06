# Auditing a repo that is not on your machine

```sh
rigswig audit --github owner/repo              # the default branch
rigswig audit --github owner/repo@v1.2.0       # a branch, tag or commit
rigswig audit --github https://github.com/owner/repo/tree/main
rigswig status --github owner/repo --json      # what a session there loads
rigswig audit --archive repo.tar.gz            # an archive you downloaded yourself: no network
rigswig ui --allow-github                      # the page, with a GitHub box in its Audit section
```

This is the only network request rigswig can make. Everything else rigswig does stays offline, so the rules here are
strict.

## The rules

1. **Opt-in, per run.** A request happens only in a run given `--github`, or from a page started with
   `rigswig ui --allow-github`, once per audit you start there. No setting, config file or environment variable turns
   it on, and `status`, `audit --repo`, `check`, `compare` and a page started without the flag never fetch anything:
   the page shows the GitHub box switched off, and its server refuses a GitHub audit (403).
2. **One host.** The input must be `owner/repo` with an optional `@ref`, or a `github.com` address, checked against
   GitHub's own name rules before anything is sent. It cannot name another host, a `file://` path or a query. The
   request goes to `https://codeload.github.com/<owner>/<repo>/tar.gz/<ref>`, and redirects are not followed.
3. **Nothing about you is sent.** No token, no cookies, no proxy settings from the environment, and a user agent
   of `rigswig/<version>`. GitHub still sees your IP address and which repo you asked for, as with any download.
   Public repos only.
4. **Nothing is written to disk.** The archive is unpacked into memory, the same in-memory file system the tests
   use, and is gone when the run ends.
5. **Links go nowhere.** Only regular files and folders are kept. Symbolic links, hard links and special files are
   dropped and counted, so a repo cannot point the audit at a file on your machine. A path that climbs out of
   the archive (`..`, or one starting at the root) fails the whole run.
6. **Limits.** A download of at most 64 MB, at most 512 MB unpacked (every entry counted, kept or not, which stops
   an archive that expands without end), at most 50,000 entries, and files over 1 MB or not UTF-8 kept by name
   only.
7. **Your setup stays out.** The repo is read with an empty, invented home folder and no environment variables, so
   your agents, plugins, instructions, memory, signal table and private list play no part, and the report holds
   nothing of yours. It describes what a stranger cloning the repo would get from its own files.
8. **Nothing runs.** As always, rigswig executes no hook, server or script from the repo, and does not use `git`.
9. **A build without it.** The download is a Cargo feature, `github`, on by default. `cargo install rigswig
   --no-default-features` builds a rigswig with no network code at all (no HTTP or TLS library in it), where
   `--github` refuses and `--archive` still works. CI tests both builds.

## Dependencies

`ureq` with `rustls` and the Mozilla root store (HTTPS, only in the `github` feature); `flate2` with its pure-Rust
backend and `tar` without extended attributes (reading archives, in every build).

## On the page

With `--allow-github`, the page's Audit section has a GitHub box: type `owner/repo` (or `owner/repo@ref`, or a
github.com address) and press **Audit on GitHub**. The page's own server makes the download; the browser still talks
only to `127.0.0.1`. The progress bar shows the megabytes received (GitHub sends no total, so the bar fills towards
40% while downloading), then unpacking and each check. The findings appear as for a local repo, without the buttons
that open items in the inventory (that inventory is your repo's, not this one's), and each "check it" command uses
`--github`. The same input rules apply: the server checks the name before anything is sent.

## Not yet

- Private repos, with a token read from an environment variable and sent to GitHub only.
- Several repos in one run, and fleet mode across GitHub repos.

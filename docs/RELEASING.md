# Releasing

A release is a version tag. Pushing `vX.Y.Z` runs `.github/workflows/release.yml`, which:

1. checks the tag matches the `rigswig` crate's version and runs the tests;
2. builds `rigswig` for macOS (Apple Silicon and Intel), Linux (x86_64 and arm64, static musl) and Windows (x86_64);
3. packages each with `LICENSE`, `NOTICE` and `README.md`, adds `SHA256SUMS`, and opens a **draft** GitHub release;
4. publishes both crates to crates.io, once a maintainer approves the `crates-io` job;
5. publishes the six npm packages, once a maintainer approves the `npm` job;
6. packages the editor extension's VSIX and attaches it to the draft ([below](#the-editor-extension)).

Nothing is public until a maintainer publishes the draft and approves the crates.io and npm jobs.

## Steps

1. Set the version in `Cargo.toml` (`[workspace.package] version`) and move the binary's pin on the library to the
   same version (`rigswig-core = { path = ..., version = "X.Y.Z" }` in `crates/rigswig/Cargo.toml`: the caret
   refuses a new minor). Run `cargo check` without `--locked` so `Cargo.lock` carries both, then `cargo test
   --locked`, and commit.
2. Tag the version from step 1 and push the tag: `git tag vX.Y.Z && git push origin vX.Y.Z`.
3. When the builds finish, read the draft release (notes, assets, checksums) and publish it.
4. Approve the `crates-io` job (the release run on GitHub, "Review deployments"); it publishes `rigswig-core`, then
   `rigswig`.
5. Approve the `npm` job; it publishes the five platform packages, then `rigswig`. (The first release with npm
   needs the bootstrap below first.)
6. Once the release is published, update the Homebrew formula ([below](#homebrew)).

## crates.io

Two crates, published in order: `rigswig-core` (the library), then `rigswig` (the binary, which depends on it by
version). Both carry copies of `LICENSE` and `NOTICE`, since a published crate holds only its own folder;
`tests/cli.rs` fails if a copy differs from the repo's. The binary's README is the repo's, which crates.io renders
with its relative links resolved against the repository.

The release workflow publishes them, after the binaries build, in its `crates-io` job: it gets a short-lived token
from crates.io through trusted publishing (GitHub's OIDC), so no token is stored, and it waits in the `crates-io`
environment until the maintainer approves the run on GitHub (Actions, the release run, "Review deployments"). Each
crate's Trusted Publishing settings on crates.io accept only this repo, `release.yml` and the `crates-io`
environment.

By hand, if the job cannot run (from a clean `main`, with `cargo login`):

```sh
cargo publish --workspace --dry-run   # packages both and builds the binary against the packaged library
cargo publish --workspace             # rigswig-core, then rigswig
```

A published version cannot be deleted, only yanked (`cargo yank --version X.Y.Z rigswig`). A re-run of the
workflow tolerates crates.io's "already": if the publish failed after a sibling, the retry reports nothing to do
instead of failing.

## npm

Six packages, published with each release by the workflow's `npm` job: `@kftcda/cli-darwin-arm64`,
`@kftcda/cli-darwin-x64`, `@kftcda/cli-linux-arm64`, `@kftcda/cli-linux-x64` and `@kftcda/cli-win32-x64`, each
carrying its platform's binary, then the small unscoped `rigswig`, which installs the right one through
`optionalDependencies` and runs it (`npx rigswig`, `npm install -g rigswig`). The Linux binaries are musl and
static, so they run on any Linux: one `cli-linux` package each. The packages' versions are the tag's, like the
crates'. The platform packages are staged from
the same matrix legs as the release binaries and the picker is built inside the `npm` job itself (`npm/build.mjs`,
tested by `npm/test.mjs`, which also checks the target list against the workflow's matrix).

The platform packages deliberately declare no `bin` field. They are never run directly, and if they shared the
picker's bin name, npm's install rollback would delete `node_modules/.bin/rigswig` whenever a sibling failed its
os/cpu check (four of the five on any host), leaving `npx` with no command. The shim (npm/rigswig/bin/rigswig.js)
locates the binary by package-relative path instead. (`npm/rigswig` as first published, 0.1.0, still carries those
`bin` fields and suffers exactly that; it was fixed before the 0.1.1 release.)

Publishing is npm trusted publishing in the `npm` environment, like crates.io's: no token is stored, each
package's Trusted Publisher settings on npmjs.com accept only this repo, `release.yml` and the `npm` environment,
and the run waits for a maintainer's approval. npm cannot grant trusted publishing to a package that does not
exist yet, so the first version of each of the six is bootstrapped by hand:

1. Download the five tarballs from a published release (v0.1.0 or later) and unpack them into one folder.
2. From the repository (a checkout that has `npm/`: the first bootstrap runs from the tree that added it,
   later ones from any tagged checkout), with `node >= 18` and after `npm login`:

   ```sh
   node npm/build.mjs --version v0.1.0 --releases /tmp/unpacked --out /tmp/npm-stage
   ```

3. Publish them in order, the five platform packages first, then `rigswig` (`npm pack --dry-run` shows what
   each ships): `cd /tmp/npm-stage/@kftcda/cli-darwin-arm64 && npm publish --access public`, then the other
   four, then `cd /tmp/npm-stage/rigswig && npm publish`.
4. Back on npmjs.com, each package's Settings → Trusted Publisher: repository `overfocus/rigswig`, workflow
   `release.yml`, environment `npm`. And on GitHub, Settings → Environments → `npm`, with required reviewers,
   mirroring what `crates-io` has.

The next tag's run then publishes everything itself.

A published npm version cannot be overwritten. To take one out of circulation, deprecate it:
`npm deprecate <package>@<version> "reason"`. Unpublishing (within 72 hours of publishing) leaves that version
number permanently unusable; avoid it.

## Homebrew

The formula lives in a tap, [overfocus/homebrew-tap](https://github.com/overfocus/homebrew-tap)
(`brew install overfocus/tap/rigswig`), and installs the release's own macOS and Linux tarballs, never a build from
source. `homebrew/formula.mjs` writes it from the published release's `SHA256SUMS`, so each URL carries the checksum
the release published; `homebrew/test.mjs` (in CI) fails when the release workflow's targets or asset names drift
from what the formula expects. For each release, in a checkout of the tap:

```sh
gh release download vX.Y.Z -R overfocus/rigswig -p SHA256SUMS -D /tmp/sums
node path/to/rigswig/homebrew/formula.mjs --version vX.Y.Z --sums /tmp/sums/SHA256SUMS > Formula/rigswig.rb
git commit -am "rigswig X.Y.Z"    # first: a local tap is a clone, so it sees commits, not the working tree
brew tap local/check "$PWD" && brew trust local/check && brew install local/check/rigswig && brew test local/check/rigswig \
  && brew audit --strict --online local/check/rigswig; brew uninstall rigswig; brew untap local/check
git push
```

Homebrew 7 loads formulae only from taps it trusts, so `brew trust local/check` comes before the install; without it
the audit refuses to load the formula (and its failure is easy to miss behind a pipe). The tap's own CI
(`brew test-bot`) runs on the push. Then `brew install overfocus/tap/rigswig` should give the new version.

## The editor extension

`vscode/` versions on its own: its `package.json` version, not the tag. A release attaches its VSIX, and
publishing the release runs `.github/workflows/extension.yml`, which publishes the manifest's version to the
Marketplace and Open VSX (each behind its own approval gate) and reports nothing to do when that version is already
there. So bump `vscode/package.json`'s version whenever the extension changes, or the publish skips it.

To check a build before it ships:

```sh
node vscode/test.mjs
cd vscode && npx -y @vscode/vsce ls --no-dependencies       # the files the VSIX will carry
npx -y @vscode/vsce package --no-dependencies               # rigswig-<version>.vsix, to install by hand
```

### Its icons

Two icons, for two places:

- **The Marketplace and Open VSX icon** (the extensions list, the extension's page): `vscode/media/icon.png`, named
  by `"icon"` in `vscode/package.json`. It must be a PNG (the Marketplace refuses SVG), square, at least 128x128;
  256x256 keeps it sharp on high-density screens. Keep the art inside a margin: the lists draw it small and round
  the corners. The page behind it is `galleryBanner.color` in the same file.
- **The activity bar icon** (the rigswig view): `vscode/media/activity.svg`, also on the page panel's tab. A 24x24
  SVG drawn in one colour: the editor masks it and paints it in the theme's colour, so fills and colours are
  ignored and only the shape shows. Strokes of about 1.5px read best.

To change either: replace the file, keeping its name (or change the path in `package.json`), run
`node vscode/test.mjs` (it checks that both exist, that the icon is a PNG of at least 128x128 and that the VSIX
ships them), bump the extension's version, and package it to look at the result: `code --install-extension
rigswig-<version>.vsix` shows the new icon in the extensions list. A registry shows the new icon only with a new
version, since a published version cannot change.

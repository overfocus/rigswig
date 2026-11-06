# Releasing

A release is a version tag. Pushing `vX.Y.Z` runs `.github/workflows/release.yml`, which:

1. checks the tag matches the `rigswig` crate's version and runs the tests;
2. builds `rigswig` for macOS (Apple Silicon and Intel), Linux (x86_64 and arm64, static musl) and Windows (x86_64);
3. packages each with `LICENSE`, `NOTICE` and `README.md`, adds `SHA256SUMS`, and opens a **draft** GitHub release;
4. publishes both crates to crates.io, once a maintainer approves the `crates-io` job.

Nothing is public until a maintainer publishes the draft and approves the crates.io job.

## Steps

1. Set the version in `Cargo.toml` (`[workspace.package] version`), run `cargo test`, and commit.
2. Tag and push: `git tag v0.1.0 && git push origin v0.1.0`.
3. When the builds finish, read the draft release (notes, assets, checksums) and publish it.
4. Approve the `crates-io` job (the release run on GitHub, "Review deployments"); it publishes `rigswig-core`, then
   `rigswig`.

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

A published version cannot be deleted, only yanked (`cargo yank --version X.Y.Z rigswig`).

## Not set up yet

- **npm** (`npx rigswig`): one package per platform carrying the binary, and a small `rigswig` package that picks the
  right one, as esbuild and Biome do. Needs an npm account.
- **Homebrew**: a tap formula pointing at the release assets and their checksums.

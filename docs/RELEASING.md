# Releasing

A release is a version tag. Pushing `vX.Y.Z` runs `.github/workflows/release.yml`, which:

1. checks the tag matches the `rigswig` crate's version and runs the tests;
2. builds `rigswig` for macOS (Apple Silicon and Intel), Linux (x86_64 and arm64, static musl) and Windows (x86_64);
3. packages each with `LICENSE`, `NOTICE` and `README.md`, adds `SHA256SUMS`, and opens a **draft** GitHub release.

Nothing is public until a maintainer reads the draft and publishes it.

## Steps

1. Set the version in `Cargo.toml` (`[workspace.package] version`), run `cargo test`, and commit.
2. Tag and push: `git tag v0.1.0 && git push origin v0.1.0`.
3. When the workflow finishes, read the draft release (notes, assets, checksums) and publish it.

## Not set up yet

- **crates.io**: `publish = false` in `Cargo.toml` keeps `cargo publish` from running by accident. Publishing needs the
  maintainer's crates.io account and a decision to claim the name.
- **npm** (`npx rigswig`): one package per platform carrying the binary, and a small `rigswig` package that picks the
  right one, as esbuild and Biome do. Needs an npm account.
- **Homebrew**: a tap formula pointing at the release assets and their checksums.

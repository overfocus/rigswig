# Publishing the editor extension

Handoff notes for taking `vscode/` from a packaged artifact to the two registries — the Visual Studio Marketplace
and Open VSX. Written when v0.1.4 shipped with the extension as a release asset; the registries are the remaining
step. Unlike crates.io and npm, **neither registry supports trusted publishing**: both publish with a stored
personal access token, which is why they get their own GitHub environments and approvals instead of riding the
npm gate.

## Assumptions

- Namespace/publisher id `kftcda`, matching the npm org; change consistently in both registries and the
  commands below if a different id is chosen.
- The extension version lives in `vscode/package.json` (independent of the crate version). Bump it only when
  the extension changed since the last tag; a re-publish of an unchanged version is a registry error. The
  release workflow already packages the VSIX with the tag's name; the version inside the VSIX is the
  manifest's.

## Step 1 — you: marketplace publisher

1. Sign in at `https://marketplace.visualstudio.com/manage` with a Microsoft account; create publisher `kftcda`.
2. Publishing needs an Azure DevOps organization behind the publisher: from the marketplace manage portal create
   one if asked, or at `https://dev.azure.com/kftcda`.
3. Personal access token in that organization: Settings → Personal access tokens → new token with scope
   **Marketplace → Manage**, organization **All accessible organizations**. That is `VSCE_PAT`.

## Step 2 — you: open vsx namespace

1. Sign in at `https://open-vsx.org` (an Eclipse account); profile → namespaces → create `kftcda`.
2. Profile → access tokens → create; that is `OVSX_TOKEN`.

## Step 3 — you: GitHub secrets and environments

- Repo secrets: `VSCE_PAT`, `OVSX_TOKEN`.
- Environments `marketplace` and `openvsx`, each with the required-reviewer gate the `crates-io`/`npm`
  environments carry.

## Step 4 — repo: the publish jobs (wire when the secrets land)

The release workflow gains two jobs after `npm`, mirroring its shape:

- `marketplace` (`environment: marketplace`):
  `npx @vscode/vsce package --no-dependencies && npx @vscode/vsce publish -p "$VSCE_PAT"` from `vscode/`, with
  tolerance: query `GET https://marketplace.visualstudio.com/_apis/gallery/publishers/kftcda/extensions/rigswig`
  first and skip when the manifest's version is already there (the re-run rule, like the crates job's "already").
- `openvsx` (`environment: openvsx`): `npx ovsx create-namespace kftcda --pat "$OVSX_TOKEN"` (needed once; the
  API tolerates an existing namespace — keep the `|| true` off the namespace call and read its output instead),
  then `npx ovsx publish --pat "$OVSX_TOKEN"`, with the same first-query tolerance against
  `https://open-vsx.org/api/kftcda/rigswig/<version>`.
- Both jobs: `node vscode/test.mjs` first, so a manifest edit that breaks the logic cannot publish.

## Step 5 — verification loop, every release

1. Marketplace: `GET
   https://marketplace.visualstudio.com/_apis/gallery/publishers/kftcda/extensions/rigswig` returns the version
   (or `npx @vscode/vsce show kftcda.rigswig --json` from a machine without the token).
2. Open VSX: `GET https://open-vsx.org/api/kftcda/rigswig/<version>`.
3. Install the VSIX attached to the release into an editor (`cursor --install-extension vscode/*.vsix` or the
   VS Code equivalent) and confirm the status bar and the page open; this one is a person's check, not CI's.

## Packaging by hand, any time

```sh
cd vscode
node test.mjs                              # logic against the built CLI when one is present
npx @vscode/vsce package --no-dependencies # rigswig-<version>.vsix (this vsce has no --licensePath flag)
```

The manifest warns about no icon: make `vscode/icon.png` (128x128) to quiet it; nothing else needs it.

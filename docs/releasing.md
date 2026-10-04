# Releasing (maintainers)

Releases are automatic: every push to `main` that touches code runs
`.github/workflows/release.yml`, computes the next version from Conventional
Commits (`scripts/next-version.sh`) and publishes a GitHub release. Nothing
has to be tagged by hand.

Each platform stamps that version into `Cargo.toml` and `Cargo.lock`
(`scripts/stamp-version.sh`) and builds with `--locked`. Releases therefore ship
exactly the dependency versions in `Cargo.lock` that CI tested. If stamping
would change anything in the lockfile besides our own crates' versions, the
release fails.

The base pipeline needs no secrets. Optional channels switch on when their
secrets exist; without them the steps are skipped and the release still
succeeds.

## macOS signing and notarization

Without these secrets the app is ad-hoc signed (users right-click → Open the
first time).

| Secret | Value |
| --- | --- |
| `MACOS_CERTIFICATE` | base64 of your *Developer ID Application* certificate exported as `.p12` (`base64 -i cert.p12`) |
| `MACOS_CERTIFICATE_PASSWORD` | the `.p12` password |
| `MACOS_SIGN_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | Apple ID email used for notarization |
| `APPLE_TEAM_ID` | your 10-character team ID |
| `APPLE_APP_PASSWORD` | an app-specific password for that Apple ID |

With the first three, binaries and the app are signed with the hardened
runtime. With all six, the `.dmg` is also notarized and stapled.

## Homebrew

Every release includes an `opensupercad.rb` cask asset, which can be
installed directly with `brew install --cask ./opensupercad.rb`. To publish
to a tap automatically:

1. Create a tap repository, e.g. `1ARdotNO/homebrew-tap`.
2. Set the repository **variable** `HOMEBREW_TAP` to `1ARdotNO/homebrew-tap`.
3. Add the secret `HOMEBREW_TAP_TOKEN`: a fine-grained token with
   *Contents: read and write* on the tap only.

Users then run `brew install --cask 1ardotno/tap/opensupercad`.

## AUR

`packaging/arch/PKGBUILD` builds from source and is suitable for an
`opensupercad` AUR package. Release CI maintains the prebuilt
`opensupercad-bin` package:

1. Create an AUR account and register the package name `opensupercad-bin`
   (push an initial commit once by hand).
2. Add the account's SSH private key as the secret `AUR_SSH_PRIVATE_KEY`.

## Bumping the OpenSCAD pins

The OpenSCAD builds the app downloads are pinned in
`crates/osc-update/openscad-pins.json`. Renovate can't track them, so the
`OpenSCAD pins` workflow checks them every week and fails when upstream
removes a pinned snapshot (snapshots stay online for about a year). To bump:

1. Pick a snapshot from <https://files.openscad.org/snapshots/> that exists
   for every platform (AppImage, `.dmg`, `-x86-64.zip`).
2. Download each file and record its size and SHA-256. Check them against
   upstream's `.sha256` files and the GPG signatures (`.asc`, key
   `E2EBDADD336FF516ADD51A78F3E12CCC22164A0F` from
   <https://files.openscad.org/OpenSCAD_public_key.asc>).
3. Update the pins and open a PR. The workflow installs and test-runs the new
   builds on Linux, macOS and Windows before it can merge.

## Bumping the Node.js pins

The Node.js the app downloads for the Claude Code and Codex adapters (when the
user has none) is pinned in `crates/osc-update/node-pins.json`. The same
workflow checks it every week. To bump to a newer LTS:

1. Take the version from <https://nodejs.org/dist/index.json> (an `lts`
   entry, 22 or newer, since the adapters require it).
2. Download `SHASUMS256.txt` and `SHASUMS256.txt.sig` from
   `https://nodejs.org/dist/v<version>/`, and check the signature with the
   keyring from <https://github.com/nodejs/release-keys>
   (`gpgv --keyring pubring.kbx SHASUMS256.txt.sig SHASUMS256.txt`).
3. Copy the hashes of the `linux-x64`, `linux-arm64` and `darwin-*` `.tar.gz`
   files and the `win-*` `.zip` files, plus their sizes, into the pins. Then
   open a PR; the workflow installs and runs the new builds on every OS.

## One-time repository settings

- Install the [Renovate GitHub App](https://github.com/apps/renovate).
- Settings → General → **Allow auto-merge**.
- Branch protection or a ruleset on `main` that requires the `ci-ok` check.
- Settings → General → Default branch: **`main`**.
- Settings → Pages → Source: **GitHub Actions** (for `pages.yml`).
- Settings → Environments → `github-pages` → Deployment branches and tags:
  allow `main`. The environment is created when Pages is first enabled and
  only allows the default branch of that moment; otherwise deploys fail with
  "Branch "main" is not allowed to deploy to github-pages".
- Settings → General → Releases: **immutable releases**, so a published
  release's assets and `SHA256SUMS` can't be changed afterwards.

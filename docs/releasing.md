# Releasing (maintainers)

Releases are automatic: every push to `main` that touches code runs
`.github/workflows/release.yml`, computes the next version from Conventional
Commits (`scripts/next-version.sh`) and publishes a GitHub release. Nothing
has to be tagged by hand.

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

## One-time repository settings

- Install the [Renovate GitHub App](https://github.com/apps/renovate).
- Settings → General → **Allow auto-merge**.
- Branch protection or a ruleset on `main` that requires the `ci-ok` check.
- Settings → General → Default branch: **`main`**.
- Settings → Pages → Source: **GitHub Actions** (for `pages.yml`).
- Settings → General → Releases: **immutable releases**, so a published
  release's assets and `SHA256SUMS` can't be changed afterwards.

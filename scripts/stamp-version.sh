#!/usr/bin/env bash
# Stamp the release version into the workspace Cargo.toml and Cargo.lock, so
# the release can build with `--locked`: exactly the dependencies CI tested.
#
#   scripts/stamp-version.sh 0.11.0
#
# Only our own crates' versions may change in Cargo.lock; anything else fails.
set -euo pipefail

version=${1:?usage: stamp-version.sh VERSION}
# The first `version =` line is [workspace.package]. Portable across GNU and
# BSD sed (macOS runners).
perl -0pi -e "s/^version = \".*?\"/version = \"$version\"/m" Cargo.toml
cargo update --workspace --quiet

changed=$(git diff -U0 Cargo.lock | grep -E '^[-+][^-+]' | grep -vE '^[-+]version = ' || true)
if [[ -n "$changed" ]]; then
  echo "::error::stamping the version changed more than our crates' versions in Cargo.lock:" >&2
  echo "$changed" >&2
  exit 1
fi
echo "stamped $version"

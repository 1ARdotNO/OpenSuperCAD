#!/usr/bin/env bash
# Compute the next release version from conventional commits since the last
# `vX.Y.Z` tag. Prints the version without the leading `v`, or nothing if there
# is nothing to release.
#
#   feat:                         -> minor bump
#   fix:, perf:, chore(deps): ... -> patch bump
#   type!: / BREAKING CHANGE:     -> major bump (minor while < 1.0)
#
# With no tag yet, the version in the workspace Cargo.toml is released as is.
set -euo pipefail

last_tag=$(git describe --tags --abbrev=0 --match 'v[0-9]*.[0-9]*.[0-9]*' 2>/dev/null || true)
if [[ -z "$last_tag" ]]; then
  sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n1
  exit 0
fi

range="$last_tag..HEAD"
if [[ -z "$(git rev-list "$range")" ]]; then
  exit 0 # nothing new since the last release
fi

IFS=. read -r major minor patch <<<"${last_tag#v}"
log=$(git log --format='%s%n%b' "$range")

if grep -qE '^[a-z]+(\([^)]*\))?!:|^BREAKING[ -]CHANGE:' <<<"$log"; then
  bump=major
elif grep -qE '^feat(\([^)]*\))?:' <<<"$log"; then
  bump=minor
else
  bump=patch
fi
# Pre-1.0: breaking changes bump the minor version.
if [[ "$bump" == major && "$major" == 0 ]]; then
  bump=minor
fi

case "$bump" in
  major) echo "$((major + 1)).0.0" ;;
  minor) echo "$major.$((minor + 1)).0" ;;
  patch) echo "$major.$minor.$((patch + 1))" ;;
esac

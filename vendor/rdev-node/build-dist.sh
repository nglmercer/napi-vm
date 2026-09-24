#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source_root="$repo_root/vendor/rdev-node"
dist_root="$repo_root/dist/rdev-node"

cd "$source_root"
bun install --frozen-lockfile
bun run build
mkdir -p "$dist_root"
rm -f "$dist_root"/node-rdev.*.node
cp package.json index.js index.mjs index.d.ts UPSTREAM_COMMIT "$dist_root/"
shopt -s nullglob
addons=(node-rdev.*.node)
if ((${#addons[@]} == 0)); then
  echo 'rdev-node build produced no native addon' >&2
  exit 1
fi
cp "${addons[@]}" "$dist_root/"
printf 'Built %s from %s\n' "$dist_root" "$(cat UPSTREAM_COMMIT)"

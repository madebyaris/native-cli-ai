#!/usr/bin/env sh
# Installs this fixture into the plugin home given by NCA_PLUGIN_HOME (use a throwaway one).
# `nca add` clones a git repository, so the fixture is copied into a temporary one first.
# Usage: NCA_PLUGIN_HOME=/tmp/spin-home ./install.sh [path-to-nca]
set -eu
: "${NCA_PLUGIN_HOME:?set NCA_PLUGIN_HOME to a throwaway directory}"
nca="${1:-nca}"
here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cp "$here/nca-plugin.toml" "$here/plugin.wasm" "$tmp/"
git -C "$tmp" init -q
git -C "$tmp" add .
git -C "$tmp" -c user.email=fixture@example.invalid -c user.name=fixture commit -qm fixture
"$nca" add "$tmp" --yes

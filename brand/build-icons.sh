#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Regenerate every Tutti app icon + the SvelteKit favicon from the single
# source of truth (brand/tutti-appicon.svg). Never hand-edit the raster icons.
#
# Requires: rsvg-convert (librsvg), cargo-tauri.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
src="$here/tutti-appicon.svg"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

png="$tmp/appicon-1024.png"
rsvg-convert -w 1024 -h 1024 "$src" -o "$png"

# Full platform icon set -> tutti-app/src-tauri/icons/
( cd "$root/tutti-app" && cargo tauri icon "$png" )

# Browser-tab favicon for the SvelteKit shell
rsvg-convert -w 512 -h 512 "$src" -o "$root/tutti-app/static/favicon.png"

echo "Regenerated app icons + favicon from $src"

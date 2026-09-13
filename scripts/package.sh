#!/usr/bin/env bash
# Stage a distributable tutti: the release binary plus the skills the design chain injects
# (and, when available, the codegraph binary the existing-repo grounder uses), laid out so the
# shipped binary resolves them beside itself with no environment set.
#
#   scripts/package.sh [DEST]
#
# DEST defaults to dist/tutti. The layout is:
#   <DEST>/tutti            the release binary
#   <DEST>/skills/          the design movement + diagram skills (resolved beside the binary)
#   <DEST>/codegraph        the codegraph binary, when CODEGRAPH_BIN points at one (optional)
#
# codegraph is MIT-licensed, so bundling it is permitted. When it is not bundled, the grounder
# falls back to a `codegraph` on PATH, and if none is present the chain still runs on stack +
# docs grounding alone (codegraph adds the domain/structure signal, it is not required).
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
dest="${1:-$repo_root/dist/tutti}"

echo "packaging tutti into $dest"
rm -rf "$dest"
mkdir -p "$dest"

# The release binary.
( cd "$repo_root" && cargo build --release -p tutti-cli )
cp "$repo_root/target/release/tutti" "$dest/tutti"

# The skills the design chain loads beside the binary (skills_dir()/RepoGrounder resolve here).
cp -R "$repo_root/skills" "$dest/skills"

# codegraph, when provided. Set CODEGRAPH_BIN=/path/to/codegraph to bundle it; otherwise the
# shipped tutti uses a codegraph on PATH, or degrades to stack+docs grounding when none exists.
if [ -n "${CODEGRAPH_BIN:-}" ] && [ -f "${CODEGRAPH_BIN}" ]; then
  cp "${CODEGRAPH_BIN}" "$dest/codegraph"
  chmod +x "$dest/codegraph"
  echo "bundled codegraph from ${CODEGRAPH_BIN}"
else
  echo "codegraph not bundled (set CODEGRAPH_BIN to bundle it); the shipped tutti will use a codegraph on PATH if present"
fi

echo "done: $dest"

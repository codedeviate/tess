#!/usr/bin/env bash
# Build MANUAL.pdf from MANUAL.md via recon's typst engine (IBM Plex Sans,
# cover + ToC + page numbers). Mirrors sercon's recipe; see
# scripts/typst-safe.awk for the preprocessing rationale.
# Override the recon binary with RECON=/path/to/recon.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VER="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/')"
RECON="${RECON:-recon}"
sed "s/{{VERSION}}/$VER/g" "$ROOT/MANUAL.md" \
	| awk -f "$ROOT/scripts/typst-safe.awk" \
	| "$RECON" --md-to-pdf - -o "$ROOT/MANUAL.pdf" \
		--gfm --page-break-on-h1 --font "IBM Plex Sans" \
		--cover --toc --toc-depth 4 --toc-plain --toc-title "Contents" \
		--doc-title "tess User Manual" \
		--doc-subtitle "Less-style terminal pager with structured-log filtering and pretty-printing" \
		--doc-version "$VER" \
		--doc-date "$(date +%Y)" \
		--doc-author "Thomas Björk" \
		--doc-keywords "tess, pager, terminal, less, rust, logs"
echo "wrote $ROOT/MANUAL.pdf ($VER)"

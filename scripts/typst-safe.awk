# typst-safe.awk — make MANUAL.md renderable by recon's typst --md-to-pdf engine.
#
# Adapted from sercon's scripts/typst-safe.awk, plus one tess-specific rule.
#
# recon's typst engine rejects ALL raw HTML, but MANUAL.md legitimately uses
# angle brackets in prose (placeholders like <mouse>, operators like FIELD<op>VALUE)
# and structural HTML comments. This filter rewrites the stream so typst accepts
# it, WITHOUT touching MANUAL.md on disk:
#
#   * Fenced code blocks (``` … ```) pass through verbatim.
#   * Outside fences: single-line HTML comments (<!-- … -->) are removed.
#   * Outside fences AND outside inline-code spans (`…`): < becomes \< and >
#     becomes \> so the markdown engine renders them literally.
#   * tess-specific: when an inline-code span closes (`) and is immediately
#     followed by '.', a zero-width space (U+200B) is inserted between them.
#     Without it, recon's translator emits typst `raw("x").Word` (field access)
#     for a code span that ends a list-item paragraph before a continuation,
#     failing with `raw does not have field "..."`. The U+200B is invisible.
BEGIN { fence = 0; zwsp = sprintf("%c%c%c", 226, 128, 139) }  # U+200B utf-8
{
	line = $0
	if (line ~ /^[[:space:]]*```/) { print line; fence = !fence; next }
	if (fence) { print line; next }
	gsub(/<!--[^>]*-->/, "", line)
	out = ""; incode = 0; n = length(line)
	for (i = 1; i <= n; i++) {
		c = substr(line, i, 1)
		if (c == "`") {
			if (incode && substr(line, i+1, 1) == ".") { out = out c zwsp; incode = 0; continue }
			incode = !incode; out = out c
		}
		else if (c == "<" && !incode) { out = out "\\<" }
		else if (c == ">" && !incode) { out = out "\\>" }
		else { out = out c }
	}
	print out
}

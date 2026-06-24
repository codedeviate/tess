# Changelog

All notable changes to `tess` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Dates are ISO 8601. Pre-1.0 minor bumps may include small breaking changes; those
are called out where relevant.

## [Unreleased]

## [0.53.1] — 2026-06-24

### Fixed

- **`--gitdiff` on a working-tree-deleted file.** A committed file that's been
  deleted from the working tree now diffs its `HEAD` version (left) against an
  empty right side (all-removed), as documented, instead of erroring `… is
  outside the git repository`. (`git::resolve`'s canonicalize fallback produced a
  relative path for a missing bare filename, which couldn't strip the absolute
  repo root.)

## [0.53.0] — 2026-06-24

### Added

- **`--gitdiff` now takes revisions and `--staged`.** Beyond the v1 working-tree
  vs `HEAD`, you can now diff against any revision and the index. Revisions are
  leading positionals (the last positional is always the file):
  - `tess --gitdiff REV FILE` — `REV` (left/old) vs working tree.
  - `tess --gitdiff R1 R2 FILE` — `R1` vs `R2` (commit ↔ commit).
  - `tess --gitdiff --staged FILE` (alias `--cached`) — `HEAD` vs the index.
  - `tess --gitdiff REV --staged FILE` — `REV` vs the index.

  A revision where the file doesn't exist diffs against an empty side; a typo'd
  revision errors with `bad revision '<rev>'`. Errors on `--staged` with two
  revisions, `--staged` without `--gitdiff`, and more than three positionals.
  Still single-file — multi-file diffs and rename detection remain deferred.

## [0.52.0] — 2026-06-23

### Added

- **`--gitdiff FILE`** — open the aligned side-by-side diff of a file's committed
  `HEAD` version (left/old) against the working tree (right/new): "show me what
  I've changed since the last commit." Reuses the existing diff engine
  (alignment, gutter signs, char-level intra-line highlighting, `]c`/`[c` hunk
  nav) — it just sources the old side from `git show HEAD:<path>`. A new/untracked
  file shows an empty old side (all added); a deleted file shows an empty new
  side (all removed). Honors `--diff-ignore-whitespace` / `--diff-force`.
  Mutually exclusive with `--diff` / `--split` / `--right-*` / the `--` per-pane
  form. Errors cleanly on: not a git repository, no commits yet (unborn HEAD),
  not exactly one file, or a file that is neither in HEAD nor on disk. v1 scope:
  working tree vs `HEAD`, single file — arbitrary revisions, staged-vs-`HEAD`,
  multi-file diffs, and rename detection are deferred.

## [0.51.0] — 2026-06-23

### Added

Batch of `less`-compatibility flags:

- **`-p PATTERN` / `--pattern PATTERN`** — start at the first line matching
  PATTERN (equivalent to `+/PATTERN`; honors `-i`/`-I`).
- **`-a` / `--search-skip-screen`** — forward search (and `n`) starts below the
  last displayed line, so matches already on screen are skipped. Runtime toggle
  `:search-skip-screen`; applies in line, filter/hide, and records modes; per
  split pane.
- **`-g` / `--hilite-search`** — highlight only the match last jumped to, rather
  than every match of the active pattern (the landed line's first match per
  display row). `-G` / `--no-hilite-search` (no highlight at all) still wins.
- **`--tilde`** — show a dim `~` on lines past end-of-file (the classic `less`
  look). Opt-in; default stays blank. Runtime toggle `:tilde`. Note: this is the
  inverse direction of `less -~` (which *disables* tildes) — tess defaults to
  blank and `--tilde` enables, a documented divergence. Long flag only.

## [0.50.0] — 2026-06-23

### Added

- **Frozen left content-columns.** `--header L,C` (and runtime `:header L [C]`)
  now pins the first `C` display columns in chop mode (`-S`): they stay put while
  `←`/`→` scroll the rest of each line, separated by a dim `│` divider. The
  freeze engages only when scrolled. Works in the single-pane view and per split
  pane (each pane honors its own `--header`). Previously the `,C` half of
  `--header` was parsed but inert. No-op in wrap/hex/raw/image; a pane too narrow
  to hold the frozen region + divider renders normally. A width-2 char straddling
  the boundary is dropped at the edge (not slid into the gutter).

## [0.49.0] — 2026-06-22

### Changed

- **Mouse wheel scrolls the pane under the cursor.** Under `--mouse` in a split,
  wheel events (all axes — vertical, Shift+wheel, and native horizontal) now
  scroll whichever pane the pointer is over, instead of always the focused pane.
  Keyboard focus is unchanged (the `*`/Tab target stays put). Scroll-locked
  panes still move together and diff still scrolls as one view; a single pane is
  unaffected.

## [0.48.0] — 2026-06-22

### Added

- **`--` per-pane argv form.** A standalone `--` splits the command line into
  per-pane view sections — `tess a --grep X -- b --grep Y -- c` opens three
  vertical panes, each with its own file and per-view flags. The first section
  (before any `--`) carries the session globals plus the focused pane; each
  later section is its own view spec (`--grep`/`--filter`/`--format`/`--display`/
  `--encoding`/`-i`/`-I` + display flags). User-defined groups expand
  per-section. Built on the N-pane split substrate.
  - `--diff a -- b` (exactly two sections) renders the aligned diff.
  - Additive and **mutually exclusive** with `--split`/`--right-*` (using both is
    an error). OR-groups (`--or-*`) and `+CMD` are **first-section-only** (an
    `--or-*` in a later section is rejected).
  - A `--` that would leave an empty section (`tess -- -dash-file`, a trailing
    `tess a --`) keeps its POSIX "end of options" meaning — only all-non-empty
    splits become per-pane.

### Changed

- **Split panes now resolve their own encoding** from each file's own head bytes
  rather than inheriting the first pane's BOM detection. Explicit `--encoding`
  is unchanged (the label wins regardless of content); only default BOM
  auto-detection across differing files is affected (now per-file — strictly
  more correct).

## [0.47.0] — 2026-06-21

### Added

- **N-pane vertical split.** The split view is no longer capped at two panes:
  `tess --split a b c` opens one vertical pane per file (≥2 files → N panes;
  one file → 2 views of it, as before). `Tab` cycles focus forward through the
  panes, **`BackTab` (Shift-Tab)** / `focus-prev-pane` cycles backward. Each pane
  keeps its own scroll/search/follow state and shows its own status segment.
  Columns divide evenly with dividers between; if the terminal is too narrow for
  all panes at a usable minimum, the focused pane renders full-width until
  there's room.
- **Scroll-lock generalizes to N panes:** `=` / `:scrolllock` couples *all*
  panes to the focused one by their captured offsets, scrolling them together
  (`Tab`-invariant). Aligned diff (`:diff`) still operates on **exactly 2 panes**
  — with 3+ it flashes a hint and stays in the plain split.

### Changed

- **`--diff` now requires exactly two files** (`--diff a b c` is an error; a
  single `--diff file` no longer self-diffes). Small pre-1.0 CLI tightening —
  use `--split` for N-pane viewing. The per-pane `--right-*` flags continue to
  seed the **second** pane; panes 3+ use the shared flags. (Uniform per-pane
  flags via a `--` argv split are a planned follow-on.)

## [0.46.0] — 2026-06-21

### Added

- **Per-pane case sensitivity at startup** for the split's second pane:
  `--right-ignore-case` (smart-case, the pane-B analog of `-i`) and
  `--right-IGNORE-CASE` (force insensitive, analog of `-I`), mutually exclusive.
  Applies to both pane B's `--right-*` predicates and its interactive search.
  (Per-pane case already worked at runtime via `:case` on the focused pane;
  this is the startup shorthand.)

### Changed

- **Pane B's case mode is now independent of the global `-i`/`-I`** (defaults to
  case-sensitive unless `--right-ignore-case`/`--right-IGNORE-CASE` is given),
  matching the no-inheritance rule of the other `--right-*` flags. Previously
  pane B's predicates inherited the session case mode. Small pre-1.0 change;
  use `--right-ignore-case`/`--right-IGNORE-CASE` to set pane B explicitly.

## [0.45.0] — 2026-06-21

### Added

- **Per-pane filter/format predicates + runtime filtering.** Each split pane can
  now carry its own predicates, and predicates can be changed at runtime (tess
  previously only accepted them at startup).
  - **Runtime colon commands** (apply to the focused pane): `:grep PAT` /
    `:nogrep`, `:filter FIELD<op>VALUE` / `:nofilter`, `:format NAME` /
    `:noformat`, `:display TEMPLATE` / `:nodisplay`. `:filter`/`:display` need a
    format on the pane (`:format NAME` sets one); `:noformat` also clears the
    filter. Compile errors flash on the status line. These work single-pane too
    — tess gains live filtering in general.
  - **Per-pane via `Tab`:** because the commands target the focused pane,
    `Tab` to a pane and set its predicates independently; each pane's
    half-width status shows its own `[grep]`/filter/format badges.
  - **Startup `--right-grep` / `--right-filter` / `--right-format` /
    `--right-display`** seed the split's second pane (pane B), independent of
    the left. `--right-filter` requires `--right-format`. The existing
    `--grep`/`--filter`/`--format`/`--display` continue to seed pane A.
  - Per-pane case sensitivity already works at runtime: `:case` targets the
    focused pane, and `:grep`/`:filter` compile with that pane's case mode.

### Notes / v1 limitations

- Runtime `:filter` sets a single replacing spec (startup `--filter`/
  `--right-filter` still repeat and AND). OR-groups remain pane-A/startup-only.
  `--dim` stays a single global flag. Per-pane predicates are inactive while
  `:diff` is on (diff works on raw lines). 2-pane vertical only.

## [0.44.0] — 2026-06-21

### Added

- **Aligned side-by-side diff mode.** Building on the split view, `--diff` /
  `:diff` compute a line-level alignment of the two panes and render them so
  matching lines sit beside each other — a real side-by-side compare inside the
  pager. Completes the split-view series (split → sync-scroll → charset → diff).
  - `--diff A B` opens the split and aligns in one shot; `:diff` enters diff on
    an existing split, `:diff!` bypasses the size cap, `:nodiff` returns to the
    plain split.
  - **Alignment + fillers:** a Myers diff (via the `similar` crate) classifies
    each line pair as equal / changed / added / removed; inserts/deletes show a
    blank filler on the opposite side so the rest stays aligned. Long lines
    wrap with **pair-padding** (each aligned pair occupies `max(left, right)`
    rows). Per-line gutter signs `~`/`+`/`-` and colors; **intra-line character
    highlighting** marks the differing characters within a changed line pair.
  - **Navigation:** `]c` / `[c` jump to the next / previous change hunk
    (remappable `diff-next-change` / `diff-prev-change`); the status shows a
    `[diff i/n]` hunk counter.
  - **Ignore whitespace:** `--diff-ignore-whitespace` (and runtime `:diffws`)
    treat lines differing only in whitespace as equal.
  - **Charset-aware:** the diff honors the active `--encoding`/`:encoding` —
    decoded text drives both the alignment display and the intra-line char diff.
  - **Huge files:** diffing reads both files fully and runs an O(ND) diff, so a
    cap (~500k lines) refuses with a message and stays in the plain
    (sync-scrollable) split unless `:diff!` / `--diff-force`. Diff is a snapshot
    — `--follow`/`--live` auto-update is suspended while it's active.

### Notes / v1 limitations

- 2-pane vertical only (no horizontal/N>2). Diff operates on **raw file lines**
  (no `--filter`/`--format`/`--display`). Intra-line highlighting is computed
  only for changed lines that fit a single row on their pane. `Tab` (focus
  swap) is locked while in diff mode — the panes scroll as one. A numbered
  `<n>G` jumps to the top in diff mode (use `]c`/`[c` to navigate changes).

## [0.43.0] — 2026-06-21

### Added

- **Charset support — read non-UTF-8 files.** tess can now decode and display
  files in legacy/non-UTF-8 encodings (ISO-8859-1, Windows-1252, Shift-JIS, and
  the rest of the WHATWG family via `encoding_rs`), instead of showing their
  bytes as `<HH>`.
  - `--encoding LABEL` selects the input charset (default `utf-8`); a runtime
    `:encoding LABEL` switches it live (and `:encoding` with no argument shows
    the current one). An unknown label is a startup error / a status flash at
    runtime.
  - **Rendering and matching both operate on the decoded text** — search,
    `--grep`, `--filter`, `--format`, and OR-groups match what you see, so
    `/café` finds a Latin-1 `café` on screen.
  - **BOM:** when the encoding is left at the default, a leading UTF-8 BOM
    resolves to UTF-8; an explicit `--encoding` always wins over a BOM.
  - **Copy/export emit UTF-8:** `:yank`, `--to-clipboard`, and `-o`/`--stdout`
    write the decoded text as correct Unicode regardless of the source encoding.
  - `-r` raw passthrough and `--hex` are unaffected (they show raw bytes); the
    UTF-8 default path is byte-identical to before (still `<HH>` for invalid
    bytes).

### Notes

- The label `iso-8859-1` (and `latin1`) uses the **windows-1252** decoder, per
  the WHATWG encoding standard — the right behavior for real-world Latin-1 text
  (the two differ only in the `0x80–0x9F` range, where windows-1252 has
  printable glyphs).
- **UTF-16 is not supported:** tess indexes lines by splitting on a lone `0x0A`
  byte, which UTF-16 code units embed, so a UTF-16 file would misalign after the
  first line. `--encoding utf-16le`/`utf-16be` and a UTF-16 BOM are rejected with
  a clear message rather than rendering mojibake.

## [0.42.0] — 2026-06-21

### Added

- **Synchronized scrolling for the split view.** Couple the two split panes so
  scrolling one scrolls the other, keeping a chosen alignment — for comparing
  two files (or two regions of one) side by side.
  - Toggle with `=` (remappable as `scroll-lock-toggle` in
    `~/.config/tess/keys.toml`), the `:scrolllock` colon command, or the
    `--scroll-lock` startup flag (meaningful only with `--split`; ignored
    otherwise).
  - **Relative (delta) lock:** the offset between the two panes' top lines is
    captured the moment you enable lock, so you can align two interesting
    regions first and then scroll together. Coupling is by **logical line**
    (the partner's wrap sub-row resets to its line start), and the partner is
    re-derived from the fixed offset on every move — so it **survives an
    EOF/top clamp and restores** the alignment when there's room again, with no
    drift.
  - `Tab` focus-swap never disturbs the alignment (the offset is stored in
    stable left/right terms). While locked, the focused pane drives — it alone
    auto-tails under `--follow`/`--live`; the partner is governed by the lock.
  - Status shows `[lock]` on the focused pane; `<lock>` is available as a
    `--prompt` / per-format prompt placeholder.
  - v1 is **line-based sync only** — diff alignment (aligned hunks, change
    highlighting, filler rows) remains a separate future cycle.

## [0.41.0] — 2026-06-20

### Added

- **Side-by-side / vertical split view.** View two panes in columns for
  lightweight compare within the pager.
  - `--split` launch flag opens a vertical 2-pane split. With ≥2 file args it
    shows the first two; with a single file it opens a second view of it.
    Interactive only (no-op for batch/`--stdout`).
  - Runtime colon commands: `:vsplit [file]` / `:split [file]` open or
    duplicate into a split (no argument duplicates the focused file at its
    current scroll position; stdin can't be duplicated — gives an error). `:only`
    / `:close` collapse back to a single pane.
  - `Tab` switches the focused pane (remappable as `focus-other-pane` in
    `~/.config/tess/keys.toml`). Scroll, search, and colon commands target the
    focused pane; the other pane keeps its own scroll/search state and follows /
    tails independently (including rotation re-open).
  - Each pane gets a **half-width status line**; the focused pane's is prefixed
    with `*`. A vertical divider separates the columns. Each pane needs ≥8
    usable columns — in a terminal too narrow to fit both, the focused pane
    renders full-width until there's room.
  - **v1 limitations (deferred):** vertical 2-pane only (no horizontal split,
    no N>2); no synchronized scrolling and no diff alignment (the two views are
    independent); the **second pane shows the plain file** — `--filter` /
    `--grep` / `--format` / `--display` predicates apply to the focused/first
    pane only; the split compositor is cell-based, so protocol images
    (Kitty/Sixel) render as **ASCII** and `-r` raw content renders through the
    cell pipeline (Interpret) while split; runtime `:vsplit` panes also omit
    `--tabs` / `--header` / status-prompt theming (the startup `--split` pane
    applies them); frozen left content-columns (`--header ,C`) per pane remain
    deferred.

### Fixed

- **Test suite is now green under default (parallel) `cargo test`, not just
  `--test-threads=1`.** Tests that mutate the process-global `HOME` / `SHELL` /
  `TESS_GLOBAL_CONFIG_DIR` env vars previously used per-module locks (and two
  were unguarded), so they raced and corrupted each other under parallelism,
  with a poisoned `Mutex` cascading the failures. They now share one crate-wide
  poison-tolerant lock. No runtime behavior change.

## [0.40.1] — 2026-06-18

### Fixed

- **Status-bar flicker during image animation on terminals that ignore
  synchronized output (e.g. Warp).** The status row was redrawn each frame as
  `Clear` then `Print` — a brief blank window a mid-repaint refresh could catch,
  which flickered the bar during animation (most visibly in the heavier
  Kitty/Sixel protocol path). The status row is now drawn **in place**: a single
  full-width write, padded by **display width** (so multibyte glyphs like `×` /
  `»` / `▶` fill the row correctly), with no preceding `Clear`. A custom
  `--prompt` carrying raw escape sequences keeps the clear-then-print path, since
  its display width can't be measured. (`--blocks` never affected this — the
  flicker was intermittent regardless of protocol/style.)

## [0.40.0] — 2026-06-18

### Changed

- **Animated images that can't be decoded now hint instead of silently showing
  a static frame.** When a source is genuinely animated but its frames can't be
  decoded — most notably a **16-bit APNG**, which the underlying `image` crate
  rejects — `tess` falls back to the static first frame *and* flashes
  `couldn't decode animation; showing first frame` on the status line, rather
  than dropping the animation with no explanation. Plain static images (even
  16-bit PNGs) are unaffected: the APNG path is gated on the `acTL` chunk, so a
  still image is never mistaken for a failed animation. Internally
  `image_render::decode_animation` now returns an `AnimationDecode`
  (`Static` / `Animated` / `Unsupported`) instead of `Option<Animation>`.

## [0.39.0] — 2026-06-17

### Added

- **Animated image playback** — animated GIFs (and APNG / WebP, where the
  decoder supports them) now *play* instead of showing only the first frame.
  Playback works in **every render mode**: the colored-ASCII / Unicode
  half-block path animates on any terminal (including Warp), and the
  `--image-protocol kitty` / `sixel` paths animate on graphics-capable
  terminals via per-frame re-emit.
  - **Auto-plays on open**, honoring the GIF loop count: after N loops it rests
    on the last frame; a loop count of `0` or absent (the common case) loops
    forever. WebP / APNG loop counts are not parsed and are treated as infinite.
  - **Transport keys** (active globally, no-op without an animation, all
    remappable in `~/.config/tess/keys.toml`): `p` pause/resume (also revives a
    finished animation), `.` step forward, `,` step back (both auto-pause),
    `Backspace` restart. Kebab binding names: `anim-pause`, `anim-step-forward`,
    `anim-step-back`, `anim-restart`.
  - **`--no-animate`** opens the static first frame instead of playing.
  - The status line in image mode shows a transport badge: `[play i/n]` /
    `[pause i/n]` / `[done n/n]`.
  - Batch export (`-o FILE` / `--stdout`) emits a single static image —
    animation is interactive-only. Native Kitty animation protocol is deferred
    (every mode uses per-frame re-emit). Behind the default-on `image` Cargo
    feature.

## [0.38.0] — 2026-06-16

### Added

- **Inline image rendering via the Kitty graphics protocol and Sixel** — render
  detected images at true pixel fidelity instead of colored ASCII art. A new
  `--image-protocol auto|kitty|sixel|ascii` flag selects the path (default
  `auto`):
  - `auto` queries the terminal for graphics support (a Kitty graphics query
    plus a DA1 query for Sixel) and falls back to the existing colored-ASCII /
    Unicode half-block rendering when neither is available. Explicit `kitty` /
    `sixel` / `ascii` skip detection entirely. When both protocols are detected,
    Kitty wins over Sixel.
  - Images fit to the terminal width with **vertical scroll** — scroll down
    through a tall image. `←`/`→` horizontal scroll is a no-op in protocol mode.
    `--image-width N` overrides the fit width.
  - Encoders are hand-rolled with **no new dependencies**; Sixel reuses tess's
    existing 256-color downsampling for its palette, and Kitty emits chunked
    base64 `_G` APC sequences.
  - Terminal support: Kitty protocol on Kitty / iTerm2 / WezTerm / Ghostty;
    Sixel on foot / xterm(+sixel) / mlterm / WezTerm.
  - With an explicit `--image-protocol kitty` or `sixel` combined with `-o FILE`
    / `--stdout`, tess writes the raw encoded escape-sequence bytes to the file
    or pipe (e.g. save a `.sixel`); `auto` / `ascii` continue to export colored
    ASCII as before.
  - `--blocks` (an ASCII style) is ignored when a non-ASCII protocol is active;
    `--no-image` still forces the raw-bytes view. The status line shows
    `[kitty]` / `[sixel]` in image mode.
  - Behind the default-on `image` Cargo feature.

## [0.37.0] — 2026-06-16

### Added

- **Four `less`-flag pickups** for closer drop-in compatibility:
  - `-x` / `--tabs LIST` sets explicit, possibly variable, tab stops. The
    argument is a comma list of column positions: a single value behaves like
    `--tab-width` (every-N stops), while multiple values pin those stops and
    repeat the **last interval** past the final entry (e.g. `-x4,8,16` stops at
    4, 8, 16, 24, 32, …). Overrides `--tab-width`. Mirrors `less -x`.
  - `-R` / `--RAW-CONTROL-CHARS` is an accepted alias for tess's default
    ANSI-interpret mode — a no-op provided for drop-in `less -R` muscle memory.
    Conflicts with `-r` (raw passthrough) and `--no-color`.
  - `-#` / `--shift N` sets the column count for the `←`/`→` horizontal-scroll
    commands; `0` keeps the half-screen default. Mirrors `less -#` / `--shift`.
  - `--wheel-lines N` sets the absolute number of body lines scrolled per
    mouse-wheel notch under `--mouse`. Default `3` (preserves the prior feel).
  - `--incsearch` enables incremental search: while typing in the `/` or `?`
    prompt, the view jumps to and highlights the first match found from where
    the prompt opened. `Esc` restores the original position; `Enter` commits to
    the previewed match. Off by default; toggle at runtime with `:incsearch`.
    Mirrors `less --incsearch`.
  - `-J` / `--status-column` adds a one-column far-left gutter showing a mark
    letter on marked lines, otherwise `*` on lines containing a current-search
    match (a mark beats `*`). The gutter is fixed under horizontal scroll, drawn
    only on the first wrap-row of a line, and is a no-op in `--hex`, `-r`, and
    image modes. Match detection runs on line content only (never the gutter)
    and works even when `-G` suppresses the visual highlight. Mirrors `less -J`.
- **Clipboard integration** — a new tess-native feature (not a `less` flag) that
  shells out to OS clipboard tools (`pbcopy`/`pbpaste` on macOS;
  `wl-copy`/`wl-paste`, then `xclip`, then `xsel` on Linux); tool failures are
  surfaced on the status line.
  - `--from-clipboard` reads the system clipboard as input. Conflicts with file
    arguments and is rejected together with `--follow`/`--live`.
  - `--to-clipboard` is a batch sink: it applies any filters / `--head` /
    `--tail` / `--prettify`, copies the result to the clipboard, and exits.
    Conflicts with `-o`/`--stdout` and is rejected with `--follow`.
  - `--clipboard` enables interactive yank: the `:yank` colon command copies the
    current line to the clipboard. A remappable `clipboard-yank-line` keybinding
    is provided but left unbound by default so it doesn't clobber `y`
    (scroll-up).

## [0.36.0] — 2026-06-01

### Added

- **Horizontal scrolling** for non-wrapping content. A `left_col` column offset
  shifts the visible window in chop mode (`-S`) and image view
  (`--image-width N` wider than the terminal). Keys: `←`/`→` scroll half a
  screen, `Shift-←`/`Shift-→` scroll 8 columns; all four remappable in
  `keys.toml` (`hscroll-left`, `hscroll-right`, `hscroll-left-step`,
  `hscroll-right-step`). Scroll fully left to return to column 0. No-op in wrap,
  hex, and raw (`-r`) modes.
  - The line-number gutter (`-N`) stays fixed while text scrolls. Chopped text
    shows a `<` "more content left" marker mirroring the existing `>`
    (`--rscroll`) marker; images shift cleanly with no markers. The status line
    shows a `»{col}` offset when scrolled, exposed as the `<col-offset>`
    `--prompt` placeholder.
  - Under `--mouse`, a native horizontal swipe (`ScrollLeft`/`ScrollRight`) or
    `Shift`+scroll-wheel also scrolls horizontally **where the terminal reports
    it** (iTerm2 / kitty / WezTerm / xterm). Warp and macOS Terminal.app report
    neither, so the keyboard is the horizontal path there.
  - Frozen left content-columns (`--header ,C`) remain deferred.

## [0.35.0] — 2026-06-01

### Added

- **OR-group filtering.** New flags `--or-filter FIELD<op>VALUE`,
  `--or-grep PATTERN`, and `--or-group NAME`. A record is shown when all
  required `--filter`/`--grep` match **and** every non-empty OR-group has at
  least one matching condition (OR within a group, AND across groups).
  `--or-group NAME` is a section marker — subsequent `--or-filter`/`--or-grep`
  join that group until the next marker; conditions before any marker form the
  implicit `default` group, which alone behaves as a single "match any of
  these" pool. `--or-filter` requires `--format` (reads named fields);
  `--or-grep` works on any input. The status line shows `[or]` when active,
  with a `<or-tag>` prompt placeholder.
- `[group.NAME]` definitions in `formats.toml` gain `or_filter` / `or_grep`
  arrays (the default OR-group) and `[group.NAME.or.<subname>]` sub-tables
  (named OR-groups), expanded into the same predicate as the CLI flags.

### Changed

- `--dim` is now accepted with OR-only conditions (previously required
  `--filter`/`--grep`); it dims records that don't satisfy the combined
  predicate.

## [0.34.0] — 2026-06-01

### Added

- `display` field for `[group.NAME]` definitions in `formats.toml`. Maps to
  `--display <template>` at group-expansion time, so a group can carry its own
  output template. Requires the group (or a CLI flag) to also set a `format`.

### Fixed

- `--display` (and other value-taking flags) used after a group token no
  longer have their value rewritten into a `--filter`. Previously
  `tess --mygroup --display '<msg>'` failed with *"a value is required for
  '--display'"* because the template was consumed as a filter. The
  value-taking-flag list used by group expansion was completed — it had been
  missing `--display`, `--prompt`, `--content-type`, `--header`, `--rscroll`,
  `--window`, `--output`, `--truecolor`, and others, plus the separated short
  value flags (`-o`, `-z`, `-t`, `-T`).

### Changed

- Repeated scalar flags now take the last value (`less`-style "last wins")
  instead of erroring with *"cannot be used multiple times"*. This makes a CLI
  flag after a group token override the group's injected value — e.g.
  `tess --mygroup --display '<custom>'` overrides the group's `display`.
  Repeatable flags (`--filter`, `--grep`) still accumulate.

## [0.33.3] — 2026-05-31

### Documentation

- README: new `## Command-line flags` section listing all 52 flags
  alphabetically by long name, matching `tess --help` (the ordering
  introduced in 0.33.2). Cut as a release so the crates.io page — whose
  rendered README is pinned per published version — carries the table.

## [0.33.2] — 2026-05-31

### Changed

- `tess --help` now lists flags alphabetically by long name
  (case-insensitive) instead of struct-declaration order. The `Args` fields
  were reordered and a test pins the ordering; the man page was regenerated
  to match. No functional change.

## [0.33.1] — 2026-05-29

### Documentation

- README: added image-rendering examples to the Images section.

### Changed

- Internal cleanup: silenced compiler warnings, fixed the Criterion
  benchmarks, and brought `clippy` to green. No user-visible change.

## [0.33.0] — 2026-05-29

### Added

- **Image-to-ASCII rendering.** `tess cat.png` auto-detects
  PNG/GIF/JPEG/BMP/WebP/TIFF/TGA/ICO/PNM by magic bytes and renders colored
  ASCII art (24-bit truecolor SGR by default, plain under `--no-color`). New
  flags:
  - `--blocks` — Unicode half-block (`▀`) mode for ~2× vertical resolution.
  - `--image-width N` — scale the rendered art to N columns.
  - `--no-image` — treat a detected image as raw bytes instead of rendering.
  - Export with `-o FILE` / `--stdout` (ANSI-colored, or plain text under
    `--no-color`).

  GIFs render their first frame. Lives behind the default-on `image` Cargo
  feature; build with `--no-default-features` for a smaller binary that
  treats all inputs as text.

## [0.32.0] — 2026-05-29

### Added

- `top_row` support in hide mode (`--filter` / `--grep`): a wrapping match
  taller than the screen can now scroll to reveal its end, matching the
  wrap-aware scrolling already available outside hide mode.

## [0.31.1] — 2026-05-29

### Fixed

- Re-pin the bottom on terminal resize, and make hide-mode `goto_bottom`
  wrap-aware, so End / follow land on the true last display row when the
  final record wraps over multiple rows.

## [0.31.0] — 2026-05-29

### Changed

- `--live` now starts at the end of the file on open (symmetric with `-f` /
  `--follow`), so the newest content is visible immediately.

### Fixed

- Wrap-aware bottom anchor: the End key and follow/live now land on the
  actual last display row instead of roughly one page above it when the last
  line wraps over multiple rows.

## [0.30.0] — 2026-05-24

### Added

- Global config layer at `/etc/tess/formats.toml` and `/etc/tess/keys.toml`
  (override path via `$TESS_GLOBAL_CONFIG_DIR`). The per-user files at
  `~/.config/tess/` are now layered on top with per-section-key replace
  semantics: a local `[format.X]` overrides the global `[format.X]` of
  the same name, but every other global entry survives. Same rule for
  `[group.X]` and individual keys inside `[bindings]`.
- `tess --list-formats` annotates each format with its source:
  `[built-in]`, `[global]`, `[local]`, or
  `[<layer>, overrides <lower-layer>]`.

### Changed

- A malformed global config file prints a warning on stderr and is
  treated as empty; the binary continues with built-ins + local config.
  (Malformed local configs still fail startup as before.)

## [0.29.1] — 2026-05-24

### Fixed

- `Viewport::frame_hex` was missing `word_wrap: false` in its
  `RenderOpts` struct literal — a stray oversight from the 0.28.0
  `--wordwrap` work. **0.29.0 source did not compile from a clean
  checkout** (`cargo install tess-cli@0.29.0` and the Homebrew formula
  build both failed); the local release build only worked because the
  working tree had been patched before commit. 0.29.1 ships the fix.
  Hex mode never word-wraps (chop, fixed columns), so the value is
  always `false`.

### Documentation

- README badge polish: standardized header layout/colors, added logos to the
  license and release badges.
- CLAUDE.md: tagging now requires creating the matching GitHub release in the
  same step.
- `OUT-OF-SCOPE.md` cleanup:
  - Removed the contradictory "remove on ship; note in CHANGELOG"
    rule that fought the cumulative `Picked up:` tracking on the
    long-tail less-flag entry.
  - Reformatted the long-tail entry so each picked-up release is its
    own bullet, with a sentence pointing at the Out-of-scope section
    to disambiguate "we pick up flags" from "we aim for `less` parity".
  - Replaced the brief "Bug-for-bug compatibility with GNU `less`"
    item with an explicit **"`less` parity is not a goal"** section:
    a four-bullet opener (no drop-in replacement, no byte-for-byte
    layout, no undocumented-quirk chasing, no `less` config files /
    env), a 17-row table of specific `less` features `tess` won't
    pursue with rationale + `tess` equivalent, and a "Specific
    intentional divergences" subsection.
  - Moved **Windows support** from Deferred to Waiting. Reframed as
    "not a primary goal" with a clearer policy (macOS + Linux daily
    driver; open to a real Windows use case + someone willing to
    drive integration testing). Added a note on NTFS file-system
    semantics around `--follow`.
- `CHANGELOG.md`: backfilled `0.20.0` through `0.29.0` (ten releases)
  in Keep-a-Changelog form.

## [0.29.0] — 2026-05-24

### Added

- `--follow-name` flag accepted for `tail -F` / `less --follow-name`
  compatibility. tess already follows by path (rotation/truncation
  detected on every poll and re-opened from offset 0, shipped in 0.25.0);
  this flag is a no-op for consistency. Emits a one-line stderr note if
  given without `-f`.
- `--exit-follow-on-close` flag. In follow mode with piped stdin, exit
  when the upstream writer closes the pipe. Default off (today's
  behavior preserved). No-op for file sources.

## [0.28.0] — 2026-05-24

### Added

- `-s` / `--squeeze-blank-lines`. Collapse runs of two or more
  consecutive blank lines into a single blank at display time. Real line
  numbers, search, and tag jumps are unaffected.
- `--header=L[,C]`. Pin top `L` source rows at the top of the viewport.
  The `C` (left columns) field is wired but currently inert — future
  horizontal-scroll work can opt into it without re-plumbing. Runtime
  adjustment via `:header L [C]`.
- `--rscroll=CHAR`. Character displayed at the right edge of a line
  chopped in `-S` chop mode, signaling "more content right". Default
  `>`. Pass `--rscroll ''` to disable.
- `-z N` / `--window=N`. PageDown / PageUp step size in lines.
  Default: full body height. Half-page commands always advance by half
  the screen regardless.
- `--wordwrap`. In wrap mode, break lines at the last whitespace before
  `cols` instead of mid-character. Falls back to mid-character break
  when no whitespace fits.

## [0.27.0] — 2026-05-24

### Added

- `-X` / `--no-init`. Skip alt-screen entry on startup; content remains
  in terminal scrollback after exit. Crucial for piped use and
  git-pager-style workflows.
- `-F` / `--quit-if-one-screen`. When the entire source fits within one
  screen and is not still being streamed, print verbatim and exit — no
  pager. Pairs naturally with `-X`.
- `-K` / `--quit-on-intr`. Accepted for `less` compatibility; no-op
  since Ctrl-C already quits.
- `-e` / `--quit-at-eof` and `-E` / `--QUIT-AT-EOF`. Auto-exit when
  scrolling past end-of-file. `-e` quits on the second consecutive
  forward-motion at EOF; `-E` quits on the first. Mutually exclusive.
- `+CMD` startup commands. Pre-clap argv pass extracts `+G`, `+NUM`,
  `+/pattern`, `+?pattern` tokens and applies them against the viewport
  before the event loop. Honors `-i` / `-I` for the search forms.

### Changed

- `TerminalGuard::enter` gains a `with_alt_screen: bool` parameter.
  When false, raw mode is still enabled but `EnterAlternateScreen` is
  skipped, and the drop path doesn't emit `LeaveAlternateScreen`.

## [0.26.0] — 2026-05-24

### Added

- `-i` / `--ignore-case`. Smart-case search: case-insensitive unless
  the pattern contains an uppercase character. Matches less / ripgrep /
  vim smartcase. Applies to `/`, `?`, `--grep`, and `--filter ~ / !~`
  regex operators.
- `-I` / `--IGNORE-CASE`. Force case-insensitive search regardless of
  pattern case. Mutually exclusive with `-i`.
- `-G` / `--no-hilite-search`. Disable search-match highlighting at
  startup. Search navigation (`n` / `N`) still works.
- `:case [sensitive|smart|insensitive]` colon command. Cycles when
  given without an argument. Re-compiles any active search so the new
  policy takes effect on the next frame.
- `:hlsearch` / `:nohlsearch` colon commands. Toggle search-match
  highlighting at runtime.

### Changed

- `GrepPredicate::compile` and `CompiledFilter::compile` now accept a
  `case_mode: CaseMode` parameter. Threaded through main.rs from `-i`
  / `-I` resolution. Library API change for downstream callers.

## [0.25.0] — 2026-05-24

### Added

- Follow-mode auto-reopen on rotation or truncation. `FileSource`
  stat's the path on every pump tick; a shrinking size or changed
  inode flips a one-shot rotation flag. The app loop reacts by
  re-opening the source from its path, clearing the line index, and
  snapping to bottom. Status flashes `(F reopened)` for ~1s.
- `(F idle)` status indicator. After ~5s of no new bytes in follow
  mode, the marker changes from `(F)` to `(F idle)` so users can tell
  the source is being watched but quiet.
- `--follow-suspend-on-motion` flag. Opt-in `less +F` semantics — any
  motion command (scroll, page, goto-line) suspends following.
  Re-engage with `Shift-F`. Bare `G` (goto-bottom) intentionally never
  suspends since it's the user re-engaging.

## [0.24.0] — 2026-05-24

### Added

- Tab completion in the `:tag` / `Ctrl-]` prompt. Extends to the
  longest common prefix on first Tab; second consecutive Tab shows the
  match count.
- Auto-reload of the tags file when its mtime changes. Before every
  tag operation (`:tag NAME`, `Ctrl-]`, `:tnext` / `:tprev` /
  `:tselect`, Tab completion), tess re-stats the tags file and
  re-parses if newer. Successful reload surfaces `[tags reloaded]`.
- Chained `;` tag addresses (`/foo/;/bar/`). Each step searches from
  the line matched by the previous one, matching vim behavior. `;`
  inside `/.../` or `?...?` patterns is treated as literal.
- Graceful skip of unsupported tag-address forms (`:s/...`, `:call ...`,
  etc.). Jump goes to line 1 of the target file with a status hint
  rather than silently failing.
- `:tselect [NAME]` colon command. Opens a picker overlay listing every
  match for the tag. `↑`/`↓` or `j`/`k` navigate; Enter or 1–9 picks
  directly. Without a name, uses the currently-active multi-match list.

### Changed

- `TagAddress` enum gains `Chained(Vec<TagAddress>)` and
  `Unsupported(String)` variants alongside the existing `Line` and
  `Pattern`.

## [0.23.0] — 2026-05-24

### Added

- `:color [strict|interpret|raw]` colon command. Cycles through the
  three ANSI policies when given without an argument, or sets one
  directly.
- `--truecolor=auto|never|always` flag. `auto` (default) checks
  `$COLORTERM` and downsamples 24-bit RGB to the xterm 256-color
  palette when truecolor isn't advertised; `never` always downsamples;
  `always` passes RGB through.
- `--status-style=SPEC` and `--prompt-style=SPEC` flags. Style the
  status row and prompt row with attribute / fg / bg tokens. Grammar:
  `bold,dim,italic,underline,reverse,fg=COLOR,bg=COLOR`. COLOR is a
  named color (`black`..`white`, optional `bright-` prefix), `#RRGGBB`,
  or 0–255. Empty string disables theming.
- Per-format `prompt_style = '...'` key in `formats.toml`. CLI
  `--prompt-style` wins; format-level wins over `--status-style`.
- Backslash escapes in `--display` and `--prompt` literals: `\e` /
  `\x1b` / `\033` (ESC), `\n`, `\t`, `\r`, `\xHH`, `\NNN`. Lets users
  embed raw SGR sequences directly in templates.
- True `-r` / `--raw-control-chars` passthrough. When `AnsiMode::Raw`
  is active, the writer emits original source bytes for each visible
  row verbatim, so cursor moves and non-SGR CSI sequences flow to the
  terminal. Wrap math is best-effort, matching `less -r`.

## [0.22.0] — 2026-05-22

### Added

- `--hex-group N` flag. Sets hex grouping in `--hex` mode to 2, 4, 8,
  16, or 32 hex characters per group (1 / 2 / 4 / 8 / 16 bytes).
  Default 4 (matches `xxd`). 32 collapses each row to a single
  unspaced group.
- `:hex N` colon command. Changes the group size live without
  restarting.

## [0.21.2] — 2026-05-22

### Documentation

- Lib crate ships with `README.md` embedded as crate docs, so the
  docs.rs front page renders the same content as GitHub.

## [0.21.1] — 2026-05-21

### Changed

- `--help` output is now colorized to match the `--examples` palette
  (yellow headers, cyan literals, bold descriptions).
- Removed brittle `display_order` numbering from clap derives; help
  flag order is now derived from declaration order, which is easier
  to maintain.

## [0.21.0] — 2026-05-21

### Added

- Interactive **help overlay**. `:help` / `:h` / `F1` opens a
  category-grouped, filter-enabled overlay showing every key binding
  and command, including any user remaps from `~/.config/tess/keys.toml`.
  Scrollwheel and `j`/`k` navigate the cursor; type to filter.
- `--mouse` flag and `TerminalGuard` toggles mouse capture. Scrollwheel
  scrolls the body and click-/scroll-events drive the file picker.
- Right-aligned `:help` discoverability hint on the default status
  line. Users get pointed at the help overlay without having to
  read the man page.

## [0.20.0] — 2026-05-21

### Added

- `:b` / `:buffers` colon command. Opens a **full-screen file picker
  overlay** listing the current working set. Type to filter, `↑`/`↓`
  or `j`/`k` to navigate, Enter to switch, Ctrl-D to drop a file.
  Each row shows the path, an indicator for the currently-open file,
  and the saved top-line offset so re-entry restores scroll position.

## [0.19.0] — 2026-05-20

### Changed

- Frame rendering uses **synchronized output** (DEC private mode 2026):
  every frame is wrapped in `\x1b[?2026h` … `\x1b[?2026l` so terminals
  that support it (iTerm2, Kitty, WezTerm, Alacritty, Ghostty, foot,
  recent VTE, Windows Terminal) buffer the whole frame and present it
  atomically. Terminals that don't recognize the sequence ignore it.
- The previous full-screen `Clear(All)` before each redraw is gone.
  Each row now does its own `Clear(UntilNewLine)` after `MoveTo(0, i)`
  immediately before painting, which also covers the shrink-on-resize
  case (old cells past the new edge are wiped).
- Together these eliminate the visible flicker that used to appear on
  every `j` / `k` / arrow keystroke, every poll tick in follow mode,
  and during resizes.

## [0.18.5] — 2026-05-20

### Documentation

- CLAUDE.md: the post-commit build chore skips the debug profile by
  default; only `cargo build --release` runs. Debug is built only when
  actually needed or on explicit request.

## [0.18.4] — 2026-05-20

### Changed

- Records-mode `--filter` now evaluates the format regex against the full
  multi-line record bytes with dotall + multi-line flags enabled, instead
  of just the record's header line. Greedy captures such as
  `(?P<message>.*)$` consume the entire record body across newlines, so
  `--filter message~foo` matches when `foo` appears anywhere in the
  record (header *or* continuation lines), which is how a user thinks
  about a multi-line record. The 0.18.2 header-only behavior was a too
  conservative first cut — fields that are bounded by line-end patterns
  (`[^\]]+`, `\w+`, etc.) keep their old semantics because the bound is
  honored regardless of dotall.

### Fixed

- `--stdout` / `--output` no longer drops records where the filter
  predicate only matches text in the body, mirroring the same change in
  the interactive viewport.

## [0.18.3] — 2026-05-20

### Fixed

- Records-mode status line no longer produces inverted record ranges like
  `R290-8/538631`. In hide mode (filter / grep without `--dim`) the
  status-line `bottom` is a position in `visible_lines`, not a logical
  line index. The R-block was passing that position into `line_to_record`,
  which resolved to an early record in the file (`8`) instead of the
  record actually visible at the bottom of the viewport. A new
  `bottom_visible_line()` helper resolves the real logical line at the
  bottom of the body — `visible_lines[cur + body_rows - 1]` in hide mode,
  `top_line + body_rows - 1` otherwise — and the R-block is derived from
  that. A defensive clamp keeps `rec_bottom >= rec_top` against future
  regressions.

## [0.18.2] — 2026-05-20

### Fixed

- `--filter` in records mode now keeps the entire matching record visible,
  not just the header line. The filter is evaluated against the record's
  header line (where the format regex was designed to anchor with `$`) and,
  on a match, all of the record's physical lines are kept. Previously the
  format regex was applied to the full multi-line record bytes; the `$`
  anchor never matched, the predicate returned `NotParsed`, and every
  record was hidden — or, in batch mode, only the header line was emitted.
- Batch mode (`--stdout` / `--output`) is now records-aware. It walks
  records (not lines), evaluates the filter against the header and grep
  against the full record bytes, and emits every physical line of each
  matching record.

## [0.18.1] — 2026-05-20

### Fixed

- `--dim` actually dims non-matching rows again. The frame writer was queuing
  a row-level `SetAttribute(Dim)` and then immediately clearing it on the
  first cell because each `Cell::Char` carried `Style::default()` (dim=false)
  and the per-cell style diff emitted `NormalIntensity`. The row-level dim
  is now OR'd into each cell's effective style (bold cells still win, since
  bold and dim share the SGR intensity slot), and `Cell::Empty` padding
  inherits the row's dim instead of resetting to default.

## [0.18.0] — 2026-05-19

### Added

- ANSI color support. SGR escapes (colors, bold, underline, italic, inverse,
  strike-through, 8/16/256/truecolor) and OSC 8 hyperlinks are interpreted by
  default instead of being shown as literal escape sequences.
- `--no-color` flag and `-r` / `--raw-control-chars` to opt back into the
  pre-0.18 byte-faithful rendering.
- `ansi` parser module with `strip_sgr` helper; `Cell::Char` now carries
  `Style` and optional hyperlink target.
- Cross-line SGR state: when scrolling into the middle of a styled region,
  tess reconstructs the active style by replaying up to 256 prior lines so
  colors don't visually reset on scroll-back.
- Frame writer now diff-emits crossterm color/attribute commands and wraps
  OSC 8 hyperlinks across the active body.

### Changed

- Non-SGR CSI sequences (cursor moves, screen clears) are silently stripped
  to protect the layout; search/filter/grep operate on the SGR-stripped text.

## [0.17.0] — 2026-05-19

### Added

- `man/tess.1` generated via `clap_mangen` from the CLI definition; a
  `gen-manpage` binary regenerates it.
- `--examples` output is now colorized (cyan command lines, yellow section
  headers).

## [0.16.0] — 2026-05-19

### Added

- ctags / etags tag jumping.
  - `-t NAME` jumps to a tag at startup; `-T PATH` selects an explicit
    tags file; without `-T`, tess walks up from the current file looking
    for `tags` / `TAGS`.
  - `:tag NAME` runtime prompt, `Ctrl-]` jumps to the tag under the cursor.
  - `Ctrl-T` pops the tag stack; `:tnext` / `:tprev` cycle multiple matches.
  - `<tag-tag>` prompt template placeholder reports the current tag.
- `tags` module: ctags + etags parsing, lookup table, and walk-up discovery.

## [0.15.0] — 2026-05-18

### Added

- Multi-file navigation. A `FileSet` working set owns paths, the active
  cursor, and append/delete/next/prev semantics.
  - Colon-command mode: `:n` / `:p` next/previous file, `:e` open,
    `:f` show filename, `:q` quit, `:d` drop current, `:x` remove from
    set, `:t` list set.
  - Marks now carry a `file_index`, and the previous-position slot is
    session-wide across files.
- README gets a badge header (GitHub / release / Rust / crates / Homebrew /
  MIT).

### Changed

- `main` extracts an `open_source_for_path` helper used by file switching.

## [0.14.0] — 2026-05-18

### Added

- Shell integration.
  - `!cmd` shell escape: drops the alt-screen, runs the command via the
    user's `$SHELL`, and resumes on keypress.
  - `--preprocess '|cmd %s'` flag and `$LESSOPEN` env-var fallback to pipe
    files through an external preprocessor before display.
  - User-remappable keybindings via `~/.config/tess/keys.toml`, including
    inline `!cmd` bindings.

### Fixed

- `Ctrl-J` no longer falsely adds the Shift modifier.
- Shell escape re-enables raw mode before reading the resume key.
- `--preprocess` is now in the mutex set with `--live`; the pdftotext
  example is correct.

## [0.13.0] — 2026-05-18

### Added

- `--hex` flag: xxd-style rendering for binary inputs, with byte offsets
  in the status line.
- `--prompt TEMPLATE` and per-format `prompt = '...'` to customize the
  status line; template placeholders include `<tag-tag>` and the active
  format name.
- `--grep`/`--filter` `[hide]` token in formats.toml `grep` field for group
  presets (renamed from the earlier hide-mode token).

### Fixed

- Hex status line shows byte offsets instead of row indices.
- Closed a `RESERVED_LONG_FLAGS` gap that allowed flag/template collisions.

## [0.12.0] — 2026-05-17

### Added

- Session-local marks: `m<x>` sets a mark, `'<x>` jumps to it.
- `Ctrl-X Ctrl-X` jumps to the previous position (round-trip).

## [0.11.0] — 2026-05-15

### Added

- Multi-line records. A `record_start` regex (in format definitions or via
  `--record-start`) groups continuation lines into a single logical record.
  - `line_index` tracks `record_starts`; viewport reports a dual L/R
    line/record readout in the status line when records are active.
  - Search, filter, and grep evaluate against whole records.
- Numeric prefix on motions: `Ng` / `NG` / `N%` go-to wiring.

### Fixed

- `goto_percent(100)` lands at the last line, not the top.
- `record_count(head_cap=0)` no longer panics; dropped dead
  `pending_record_start` field.
- Viewport tests renamed to silence `non_snake_case` warnings.

### Tests

- Property-based tests (`proptest`) covering the render kernel invariants.
- PTY smoke tests for spawn / quit / SIGTERM / resize.
- Criterion benchmarks for `line_index`, scroll math, search, and render.
- Integration / property / PTY / bench coverage wired for records mode.

## [0.10.5] — 2026-05-15

### Documentation

- README documents `cargo bench` and `cargo test -- --test-threads=1`.
- Out-of-scope: dropped the already-resolved `Read` import entry.

## [0.10.1] — 2026-05-15

### Changed

- `Cargo.lock` is now committed (binary-crate convention).

## [0.10.0] — 2026-05-13

### Added

- `--grep PATTERN` raw-line regex filtering. Repeatable, AND'd, composable
  with `--filter`. `GrepPredicate` (regex AND on raw lines) hides or dims
  non-matching lines and surfaces grep state in the status line. Threads
  through interactive mode and `--output`/`--stdout` batch mode.
- `--dim` is now permitted alongside `--grep` (keeps non-matches visible
  but faded).

### Fixed

- `expand_argv` handles `--grep` values; `--grep` is in the reserved-flag
  set so user-defined groups can't collide with it.

## [0.9.1] — 2026-05-08

### Changed

- Published on crates.io as `tess-cli` (the `tess` name was unavailable);
  binary is still `tess`. Out-of-scope adopts
  Waiting / Deferred / Not-yet / Out-of-scope buckets.

## [0.9.0] — 2026-05-07

### Added

- `--display TEMPLATE` and per-format `display` key: templated rendering of
  parsed log fields (e.g. compact, colorless, custom field order).

## [0.8.0] — 2026-05-07

### Added

- Non-interactive batch mode: `--output FILE` and `--stdout` write the
  resolved view (with filters / grep / display template applied) without
  entering the alt-screen — useful in pipelines and CI.

## [0.7.0] — 2026-05-07

### Added

- Comparison operators in `--filter`: `<`, `<=`, `>`, `>=` (in addition to
  `=` / `!=` / regex match).

### Documentation

- MANUAL documents the nested-capture-group pattern for log formats.
- Out-of-scope: multi-line log records (`record_start`) deferred (later
  landed in 0.11.0).

## [0.6.6] — 2026-05-05

First crates.io / Homebrew-ready release.

### Changed

- Full crates.io metadata: expanded description, homepage, documentation
  URL, keywords, categories, `exclude` list to drop local artifacts/notes.
- MSRV pinned at `rust-version = "1.85"` (clap_lex 1.1.0 → edition 2024
  → Rust 1.85).
- Release profile tuned.

## [0.6.5] — 2026-05-05

### Added

- MIT license; Cargo metadata for publishing.

### Documentation

- `README.md`.

## [0.6.x development] — 2026-04-27 → 2026-05-05

The initial run from project scaffold to publishable crate. Notable
milestones, in chronological order:

### Added — kernel and core

- `error` enum and exit-code mapping (0 clean / 1 startup / 2 runtime).
- `render` kernel: cell types and ASCII layout, tab expansion to next tab
  stop, control-byte `^X` and invalid-byte `<HH>` rendering, UTF-8
  grapheme cluster decoding with width-2 support, correct wrap and chop at
  width-2 boundaries, `count_rows` fast path for scroll math.
- `source`: `Source` trait, `FileSource` (mmap + fallback), `MockSource`
  for tests, `StdinSource` (synchronous and threaded streaming modes).
- `line_index`: lazy + incremental newline scan.
- `viewport`: state, frame composition, line scroll, paging / half-paging
  / goto / resize, toggles.
- `input`: full key-map event-to-command translation.
- `terminal`: `TerminalGuard` (alt-screen RAII), panic hook, signal flag.
- `app`: main event loop with frame writing.
- `cli`: clap-based argv parsing.
- `main`: CLI wiring, source resolution, terminal guard, app loop.

### Added — features

- Follow mode (`-f` / `--follow`, interactive `Shift-F`).
- `--head N` and `--tail N` (reverse byte-offset scan for `--tail`).
- Log-format parsing with named regex captures and field-based filtering
  (`--filter FIELD<op>VALUE`, repeatable, AND'd).
  Built-in formats: apache-common, apache-combined, nginx-combined.
  User-defined formats in `~/.config/tess/formats.toml`.
- User-defined CLI groups (`[group.NAME]` in `formats.toml`):
  `--<groupname>` expands to a fixed flag bundle and turns positionals
  into filters.
- Interactive regex search (`/`, `?`, `n`, `N`) with row highlight.
- Alphabetical `--help`, `--manual`, and `--examples` (auto-page on TTY);
  `INSTALL.md`.
- `--live` flag for in-place file rewrites, plus the `R` reload key.
- `--prettify` and `--content-type` for JSON / YAML / TOML / XML / HTML /
  CSV.
- `J` / `K` jump to next/prev logical line; status shows wrap row.

### Fixed

- Eliminated flicker and restored keyboard input on piped stdin.
- Switched `crossterm` to `use-dev-tty` and scoped the stdin redirect to
  pipe mode (the default mio source failed on macOS with piped stdin).
- `line_index::extend_to_line` breaks when `head_cap` is hit.
- Search `/<Enter>` repeats; scroll walks wrap rows of the last line.
- Per-substring search highlight + attribute-bleed fix.

### Documentation

- `CLAUDE.md`, `OUT-OF-SCOPE.md`, `MANUAL.md` (with extensive examples,
  including the bash history-expansion gotcha for `!`).
- `INSTALL.md` documents the macOS 26 SIGKILL gotcha (codesign on
  recovery).

### Renames

- Crate renamed from `rustless` to `tess`.
- Project directory `Test` → `tess` in `CLAUDE.md` paths.

### Tests

- Golden-frame integration test exercising
  `FileSource → LineIndex → Viewport → render`.

[Unreleased]: https://github.com/codedeviate/tess/compare/v0.36.0...HEAD
[0.36.0]: https://github.com/codedeviate/tess/compare/v0.35.0...v0.36.0
[0.35.0]: https://github.com/codedeviate/tess/compare/v0.34.0...v0.35.0
[0.34.0]: https://github.com/codedeviate/tess/compare/v0.33.3...v0.34.0
[0.33.3]: https://github.com/codedeviate/tess/compare/v0.33.2...v0.33.3
[0.33.2]: https://github.com/codedeviate/tess/compare/v0.33.1...v0.33.2
[0.33.1]: https://github.com/codedeviate/tess/compare/v0.33.0...v0.33.1
[0.33.0]: https://github.com/codedeviate/tess/compare/v0.32.0...v0.33.0
[0.32.0]: https://github.com/codedeviate/tess/compare/v0.31.1...v0.32.0
[0.31.1]: https://github.com/codedeviate/tess/compare/v0.31.0...v0.31.1
[0.31.0]: https://github.com/codedeviate/tess/compare/v0.30.0...v0.31.0
[0.30.0]: https://github.com/codedeviate/tess/compare/v0.29.1...v0.30.0
[0.29.1]: https://github.com/codedeviate/tess/compare/v0.29.0...v0.29.1
[0.29.0]: https://github.com/codedeviate/tess/compare/v0.28.0...v0.29.0
[0.28.0]: https://github.com/codedeviate/tess/compare/v0.27.0...v0.28.0
[0.27.0]: https://github.com/codedeviate/tess/compare/v0.26.0...v0.27.0
[0.26.0]: https://github.com/codedeviate/tess/compare/v0.25.0...v0.26.0
[0.25.0]: https://github.com/codedeviate/tess/compare/v0.24.0...v0.25.0
[0.24.0]: https://github.com/codedeviate/tess/compare/v0.23.0...v0.24.0
[0.23.0]: https://github.com/codedeviate/tess/compare/v0.22.0...v0.23.0
[0.22.0]: https://github.com/codedeviate/tess/compare/v0.21.2...v0.22.0
[0.21.2]: https://github.com/codedeviate/tess/compare/v0.21.1...v0.21.2
[0.21.1]: https://github.com/codedeviate/tess/compare/v0.21.0...v0.21.1
[0.21.0]: https://github.com/codedeviate/tess/compare/v0.20.0...v0.21.0
[0.20.0]: https://github.com/codedeviate/tess/compare/v0.19.0...v0.20.0
[0.19.0]: https://github.com/codedeviate/tess/compare/v0.18.5...v0.19.0
[0.18.5]: https://github.com/codedeviate/tess/compare/v0.18.4...v0.18.5
[0.18.4]: https://github.com/codedeviate/tess/compare/v0.18.3...v0.18.4
[0.18.3]: https://github.com/codedeviate/tess/compare/v0.18.2...v0.18.3
[0.18.2]: https://github.com/codedeviate/tess/compare/v0.18.1...v0.18.2
[0.18.1]: https://github.com/codedeviate/tess/compare/v0.18.0...v0.18.1
[0.18.0]: https://github.com/codedeviate/tess/compare/v0.17.0...v0.18.0
[0.17.0]: https://github.com/codedeviate/tess/compare/v0.16.0...v0.17.0
[0.16.0]: https://github.com/codedeviate/tess/compare/v0.15.0...v0.16.0
[0.15.0]: https://github.com/codedeviate/tess/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/codedeviate/tess/compare/v0.13.0...v0.14.0
[0.13.0]: https://github.com/codedeviate/tess/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/codedeviate/tess/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/codedeviate/tess/compare/v0.10.5...v0.11.0
[0.10.5]: https://github.com/codedeviate/tess/compare/v0.10.1...v0.10.5
[0.10.1]: https://github.com/codedeviate/tess/compare/v0.10.0...v0.10.1
[0.10.0]: https://github.com/codedeviate/tess/compare/v0.9.1...v0.10.0
[0.9.1]: https://github.com/codedeviate/tess/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/codedeviate/tess/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/codedeviate/tess/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/codedeviate/tess/compare/v0.6.6...v0.7.0
[0.6.6]: https://github.com/codedeviate/tess/compare/v0.6.5...v0.6.6
[0.6.5]: https://github.com/codedeviate/tess/releases/tag/v0.6.5

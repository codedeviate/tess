# Changelog

All notable changes to `tess` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Dates are ISO 8601. Pre-1.0 minor bumps may include small breaking changes; those
are called out where relevant.

## [Unreleased]

### Documentation

- README badge polish: standardized header layout/colors, added logos to the
  license and release badges.
- CLAUDE.md: tagging now requires creating the matching GitHub release in the
  same step.

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

[Unreleased]: https://github.com/codedeviate/tess/compare/v0.18.1...HEAD
[0.18.1]: https://github.com/codedeviate/tess/compare/v0.18.0...v0.18.1
[0.18.0]: https://github.com/codedeviate/tess/releases/tag/v0.18.0
[0.17.0]: https://github.com/codedeviate/tess/releases/tag/v0.17.0
[0.16.0]: https://github.com/codedeviate/tess/releases/tag/v0.16.0
[0.15.0]: https://github.com/codedeviate/tess/releases/tag/v0.15.0
[0.14.0]: https://github.com/codedeviate/tess/releases/tag/v0.14.0
[0.13.0]: https://github.com/codedeviate/tess/releases/tag/v0.13.0
[0.12.0]: https://github.com/codedeviate/tess/releases/tag/v0.12.0
[0.11.0]: https://github.com/codedeviate/tess/releases/tag/v0.11.0
[0.10.5]: https://github.com/codedeviate/tess/compare/v0.10.1...v0.11.0
[0.10.1]: https://github.com/codedeviate/tess/compare/v0.10.0...v0.10.5
[0.10.0]: https://github.com/codedeviate/tess/compare/v0.9.1...v0.10.0
[0.9.1]: https://github.com/codedeviate/tess/releases/tag/v0.9.1
[0.9.0]: https://github.com/codedeviate/tess/releases/tag/v0.9.0
[0.8.0]: https://github.com/codedeviate/tess/compare/v0.6.6...v0.9.0
[0.7.0]: https://github.com/codedeviate/tess/compare/v0.6.6...v0.9.0
[0.6.6]: https://github.com/codedeviate/tess/releases/tag/v0.6.6
[0.6.5]: https://github.com/codedeviate/tess/compare/v0.6.6...v0.6.6

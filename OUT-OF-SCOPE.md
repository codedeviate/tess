# Out of Scope & Wishlist

A living list of items raised during design, implementation, or feature sweeps
that are either explicitly deferred, decided against, or noted as "maybe
later". Also doubles as a wishlist — items under "Waiting" are things worth
building once someone explicitly asks. Kept here so ideas don't disappear
into the black hole of spec files after each release.

Organized into four buckets by reason for non-inclusion. When an item ships,
remove it from this file and note the shipping version in the CHANGELOG
entry rather than leaving a crossed-out line here.

- **Waiting** — can be done; nobody's asked for it.
- **Deferred** — possible to implement; actively put off (scope/complexity
  trade-off or waiting on a concrete use case).
- **Not yet supported** — blocked by upstream / ecosystem maturity; may ship
  when the blocker clears.
- **Out of scope** — fundamentally can't be implemented, architecturally
  mismatched, or intentionally declined by policy.

---

Each entry carries a rough size tag (**S** / **M** / **L**). None of these
should be added without their own spec → plan cycle.

## Waiting

### Cargo.lock policy — **S**

For a shipped binary, `Cargo.lock` should normally be committed. Currently it's in `.gitignore` because this started as a personal learning project. Flip when shipping or when collaborators join.

### PTY-based integration tests — **M**

The MVP relied on a single golden-frame test plus manual smoke testing. Adding `expectrl`/`rexpect` would let us verify keyboard interaction, terminal restoration on panic/SIGTERM, and resize behavior in CI. Not free — PTY tests are flaky if not carefully scoped.

### Performance benchmarks — **S**

`cargo bench` baseline for big-file open, scroll, and search. Wait until there's a perceivable performance issue or a redesign that risks regressions.

### `proptest` for `render` — **S**

The render kernel has small, well-defined inputs and outputs. Property tests like "`count_rows(b, opts) == render_line(b, opts).len()` for any `b` and any `opts.cols >= 1`" would catch edge cases beyond the hand-picked tests. Fair amount of value.

### Redundant `use std::io::Read;` in `src/source.rs` — **S**

There's both a top-level import and a function-local one. Harmless, but a one-line cleanup.

---

## Deferred

### Multi-file navigation (`:n`, `:p`, `:e`, file list) — **M**

The CLI already accepts multiple files but only opens the first; we even emit a stderr warning about ignored ones. Need a small `file_set` module that owns a list of `Source`s and a current-index, plus the colon-prefix command interface.

Touches: `cli` (already collects them), `app` (colon command mode), new module.

### Marks and jumps (`m<x>`, `'<x>`, `123g`, `50p`, `^X^X`) — **S/M**

Save and restore positions. Single-keystroke goto-line and percentage jumps need a numeric-prefix accumulator in `input`/`app`.

### Shell integration — **M**

- `!cmd` to escape to the shell while keeping the file open.
- `LESSOPEN` / `LESSCLOSE` preprocessor (e.g., view PDFs as text via a configured pipe).
- `lesskey` custom keybindings.

Each is its own sub-feature; they don't have to land together.

### Prompt customization (`-P` / `lessrc`) — **S**

Replace the hardcoded status format with a templated prompt. Needs a tiny format-string parser. Currently the status is `"<label>  <top>-<bottom>/<total>  <pct>%"`, baked into `viewport::format_status`.

### Tags (`-t`, `-T`) — **M**

Jump to a tag (ctags-style). Requires parsing a tags file.

### Hex display — **S**

When the file looks binary, show a hex dump instead of byte-faithful text. Could be a `--hex` flag.

### Multi-line log records (`record_start` in formats.toml) — **M**

Some log formats (notably PHP's default error log, Java stack traces, multi-line debug payloads) emit records that span many physical newlines. Today every `\n` is a hard logical-line boundary, so a record like

```
[2026-05-06 10:23:11] ERROR Failed to render template
  #0 /var/www/app/Renderer.php(214): App\Tpl::render()
  #1 /var/www/app/Controller.php(88): App\Renderer->show()
  #2 {main}
```

is four searchable units, none of which contain the full message. A regex search for `Renderer.php.*ERROR` would never match.

Proposed shape (kept compatible with the existing format system):

```toml
[format.php-app]
# A new record begins on any line whose start matches this regex.
# Lines that don't match are appended to the previous record.
record_start = '^\['
# `pattern` then runs against the FULL multi-line record string with
# embedded \n's, so multi-line capture groups Just Work.
pattern = '^\[(?P<ts>[^\]]+)\]\s+(?P<level>\w+)\s+(?P<msg>[\s\S]+)$'
```

Display model: preserve physical newlines as-is (a 5-line record renders as 5 visible rows), but treat the record as one searchable unit:
- **Indexing**: `LineIndex` (or a new `RecordIndex` wrapper) tracks record-start byte offsets, computed in the same scan pass as newline offsets.
- **Search/filter/highlight**: operate on whole records. `n` jumps to the next record with a match; hide-mode shows all physical lines of a matching record; the matched phrase is reverse-video'd on whichever physical row carries it.
- **Status line**: `<top>-<bottom>/<total>` becomes record counts (or gains a record/line dual readout).

Touches: `format` (schema extension), `line_index` (record offsets), `viewport` (search/filter/status all currently key off line-N), and `filter` (matches against the full record, not the first line). Wrap-row scrolling continues to work inside records — record boundaries are higher-level than wrap rows.

Alternative considered: collapse `\n` → `␊` on render. Trivial (only `LineIndex` needs to know about records) but defeats the readability point for stack traces. Don't go this way.

### Follow-mode follow-ups — **S each**

- **File rotation / truncation**: real `tail -F` re-opens the file when it shrinks or its inode changes. We currently keep a single `File` handle and would read garbage past a truncation. Detect via `metadata().len() < known_size` (or inode change) and re-open from offset 0.
- **No-content idle hint**: when in follow mode and nothing has arrived for a while, an indicator like `(F idle)` could be useful. Trivially derivable from "ticks since last growth".
- **Press-any-key suspends follow** (real `less +F` semantics): right now `Shift-F` is the explicit toggle and movement keys leave follow on (auto-scroll just doesn't fire because user isn't at bottom). If a user finds this surprising, change `ScrollLines(-…)` and friends to also `set_follow_mode(false)`.

### Long tail of `less` flags — **L (cumulative)**

`less --help` lists ~80 options. Many are trivial alias toggles, some are non-trivial behavior. Add as needed; document each in its own commit.

### Windows support — **M**

`crossterm` already supports Windows; the redirect / `dup2` path and `signal-hook` are Unix-only. Need `#[cfg(windows)]` branches that:
- Skip the dup2 (no `/dev/tty` on Windows).
- Use Windows console-mode equivalents for raw mode (crossterm handles).
- Replace `signal-hook` with Windows console events.

### `anyhow` / `thiserror` — **S**

The handwritten `enum Error` works for MVP but the boilerplate grows linearly with error sites. If error variety expands meaningfully, swap to `thiserror` for the enum and `anyhow::Result` at boundaries. Not before there's a real reason.

### Cell representation — **S**

`Cell::Char { ch: char, width: u8 }` is fine for MVP but a screen-buffer of `Cell` is a lot of bytes. If memory becomes a concern, consider a denser representation (e.g., parallel `Vec<char>` and `Vec<u8>` for widths). Not before measurement.

---

## Not yet supported

_Nothing here today — no items are currently blocked on upstream or ecosystem
maturity._

---

## Out of scope

### Bug-for-bug compatibility with GNU `less`

Explicitly **not** a goal. If we ever did pursue it, we'd need to sit `less -V` against `tess -V` and walk every flag. Most users care about the daily-driver flags, which is what the items above prioritize — chasing exact-match quirks (cursor-positioning edge cases, undocumented escape handling, prompt-format minutiae) is open-ended work without a real user benefit.

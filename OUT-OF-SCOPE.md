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

---

## Deferred

### Tag-feature follow-ups — **S each**

- Tag-name completion in the `:tag` / `Ctrl-]` prompt.
- Auto-reload of the tags file when its mtime changes.
- Support for `ex` command tag addresses beyond simple `/pattern/` and line numbers.
- `:tselect` — a numbered picker for multi-match tags up front (vim has both cycling and select).

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

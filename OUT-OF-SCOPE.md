# Out of Scope & Wishlist

A living list of items raised during design, implementation, or feature
sweeps that are either explicitly deferred, decided against, or noted as
"maybe later". Also doubles as a wishlist — items under **Waiting** are
things worth building once someone explicitly asks. Kept here so ideas
don't disappear into the black hole of spec files after each release.

Organized into four buckets by reason for non-inclusion:

- **Waiting** — can be done; nobody's asked for it.
- **Deferred** — possible to implement; actively put off (scope/complexity
  trade-off or waiting on a concrete use case).
- **Not yet supported** — blocked by upstream / ecosystem maturity; may
  ship when the blocker clears.
- **Out of scope** — fundamentally can't be implemented, architecturally
  mismatched, or intentionally declined by policy.

Each entry carries a rough size tag (**S** / **M** / **L**). None should
be added without its own spec → plan cycle.

When a deferred item ships, capture the version in CHANGELOG.md. Items
that are themselves cumulative (e.g. the **Long tail of `less` flags**
entry) keep an inline `Picked up:` log here so future readers can see at
a glance which sub-items have already been delivered without scanning
the full changelog.

---

## Waiting

### Windows support — **M**

Not a primary goal. `tess` targets the macOS + Linux daily driver, and
that's where the user base is. Windows isn't actively pursued, but the
work is well-scoped enough that if someone shows up with a concrete
use case and is willing to drive the integration testing, it's not a
hard "no".

Technical sketch when the time comes:

- `crossterm` already supports Windows; the existing `dup2` redirect
  path and `signal-hook` usage are the Unix-specific bits.
- Need `#[cfg(windows)]` branches that:
  - Skip the `dup2` step (no `/dev/tty` on Windows).
  - Use Windows console-mode equivalents for raw mode (crossterm
    handles these natively).
  - Replace `signal-hook` with Windows console events for Ctrl-C /
    Ctrl-Break handling.
- File-system semantics around `--follow` rotation/truncation differ
  on NTFS — inode equivalents and file-locking behavior would need
  their own design pass.

Until someone has a real Windows use case, the maintenance cost (CI
matrix, manual testing on a platform the maintainer doesn't run) isn't
worth eating.

---

## Deferred

### Long tail of `less` flags — **L (cumulative)**

`less --help` lists ~80 options. Many are trivial alias toggles, some
are non-trivial behavior. Add as the need arises; each pickup gets its
own commit and a `Picked up:` line below.

This entry covers flag-by-flag pickup, **not** drop-in `less` parity —
see the **Out of scope** section below for what `tess` deliberately
does not pursue.

**Picked up:**

- `0.26.0` — `-i`, `-I`, `-G` (search ergonomics).
- `0.27.0` — `-X`, `-F`, `-K`, `-e`, `-E`, `+CMD` startup commands.
- `0.28.0` — `-s`, `--header=L[,C]`, `--rscroll=c`, `-z N`, `--wordwrap`.
- `0.29.0` — `--follow-name`, `--exit-follow-on-close`.
- `0.37.0` — `-x`/`--tabs`, `-R`, `-#`/`--shift`, `--wheel-lines`, `--incsearch`, `-J`/`--status-column`.

### `anyhow` / `thiserror` — **S**

The handwritten `enum Error` works for MVP but the boilerplate grows
linearly with error sites. If error variety expands meaningfully, swap
to `thiserror` for the enum and `anyhow::Result` at boundaries. Not
before there's a real reason.

### Cell representation — **S**

`Cell::Char { ch: char, width: u8 }` is fine for MVP but a screen-buffer
of `Cell` is a lot of bytes. If memory becomes a concern, consider a
denser representation (e.g., parallel `Vec<char>` and `Vec<u8>` for
widths). Not before measurement.

---

## Not yet supported

_Nothing here today — no items are currently blocked on upstream or
ecosystem maturity._

---

## Out of scope

### `less` parity is not a goal

`tess` deliberately picks up high-value `less` flags one at a time
(see **Long tail of `less` flags** under Deferred above). What's
explicitly **not** a goal:

- **Drop-in replacement.** `tess` is not aiming to pass a hypothetical
  `less` test suite, match `less -V` flag-for-flag, or be invokable as
  `less` via a symlink.
- **Byte-for-byte status / prompt layout.** Default status formatting,
  prompt placeholder ordering, and key-binding details differ where
  `tess` thinks the alternative is clearer (see "Intentional
  divergences" below).
- **Undocumented quirks.** Cursor-positioning edge cases, ambiguous
  escape handling, behavior under malformed input — `tess` follows
  whatever Rust + crossterm produce naturally rather than
  reverse-engineering `less`'s output.
- **`less` config files and env.** `~/.lesskey` / `~/.less` are not
  consulted. `LESS`, `LESSCHARSET`, `LESSCLOSE`, `LESSANSIENDCHARS`,
  `LESSEDIT`, `LESSMETACHARS`, `LESSSECURE`, `LESS_TERMCAP_*`, and
  friends are ignored. (`$LESSOPEN` *is* consulted as a fallback for
  `--preprocess` — that's an existing exception, documented as such.)
- **Compatible binary on PATH.** No "rename me to less and I'll
  pretend" mode. No `argv[0]` sniffing.

#### Specific `less` features `tess` won't pursue

| less feature | Why | tess equivalent (if any) |
|---|---|---|
| `&pattern` (filter-only-matching-lines from search prompt) | UI conflates search and filter; we keep them separate. | `--grep PATTERN` (raw line), `--filter FIELD~PATTERN` (parsed field), or runtime `:case` toggle for the active search. |
| Search prefix modifiers `^E` / `^F` / `^K` / `^R` / `^S` / `^W` (typed mid-search) | Hidden modal state inside an input field is hard to discover; we prefer flags / colon commands. | `-i` / `-I` / `:case` for case, `:hlsearch` / `:nohlsearch` for highlight, `--grep` for non-regex matching. |
| Bracket matching (`{`, `(`, `[` go to matching brace) | Pager-vs-editor blurring; not in the daily-driver flag set. | None planned. Use an editor for structural navigation. |
| `-Dxcolor` granular palette overrides | Less-internal color slot model. | `--status-style`, `--prompt-style` (theming), `--truecolor` (downsample policy), `:color` (runtime mode). |
| `-u` / `-U` (`--underline-special` — special CR / backspace handling) | Predates UTF-8 / ANSI passthrough; conflicts with our cell pipeline. | `-r` / `:color raw` for byte-faithful passthrough. |
| `-w` / `-W` (`--hilite-unread` — highlight first-unread row on each page) | Adds a stateful read-cursor that doesn't compose with `--grep` / `--filter`. | None planned. |
| `-jn` / `--jump-target=n` (place jump target at screen row N) | Niche scrolling preference; conflicts with `--header=L` pinning. | None planned. |
| `--lesskey-*` (binary keytable loaded from `~/.less`) | We have a text-format key map already. | `~/.config/tess/keys.toml`. |
| `LESSOPEN` (env var, full less semantics including type sniffing) | Honored as a fallback only — see note above. | `--preprocess '|cmd %s'`. |
| `LESSCLOSE` (cleanup hook after LESSOPEN) | Without LESSOPEN's full lifecycle, this is moot. | None — `--preprocess` runs once and exits. |
| `--save-marks` (marks persist across runs) | Stateful storage outside the process; out of scope without an explicit use case. | None planned. |
| `-bn` / `-B` / `--buffers=n` (kbytes of buffer to keep) | mmap-based source; the kernel manages this. | None — no equivalent needed. |
| `-c` / `-C` / `--clear-screen` modes | `tess` already uses synchronized-output (DEC 2026); no flicker. | None — already default. |
| `-d` / `--dumb` (suppress dumb-terminal errors) | We use `crossterm`; this is its concern. | None. |
| `-q` / `-Q` / `--quiet` / `--silent` (bell control) | Terminal-bell behavior is `crossterm`'s domain. | None. |
| `--proc-backspace` / `--proc-return` / `--proc-tab` and inverse forms | Less-internal control-byte handling switches. | `--no-color` (caret form) / default (interpret) / `-r` (raw passthrough). |
| `--no-vbell`, `--no-keypad`, `--no-histdups` | Less-internal niceties. | None. |
| `--modelines=n` (parse vim modelines) | Pager-vs-editor blurring; mode lines are an editor concern. | None planned. |

#### Intentional divergences from `less`

These are places where `tess` decided on a different default or syntax
rather than match `less` literally:

- **Status line format.** `tess` shows `<label>  <top>-<bottom>/<total>
  <pct>%` plus mode badges (`(F)`, `(L)`, `[grep]`, etc.) by default.
  `less -m` / `-M` long-prompt presets are not provided as flags;
  users with custom prompt needs use `--prompt TEMPLATE` (analog of
  `less -P`) or per-format `prompt = '…'` in `formats.toml`.
- **Search highlight scope.** `tess` reverse-videos only the *matched
  phrase* per row (using regex match offsets); `less -g` highlights the
  whole logical line. We picked the tighter highlight because it
  composes better with colored input.
- **Tag-jump UX.** `tess` exposes `:tag NAME` / `Ctrl-]` / `Ctrl-T` /
  `:tnext` / `:tprev` / `:tselect` (picker overlay for multi-match).
  `less`'s `t` / `T` key bindings are not bound by default — `:tnext`
  / `:tprev` are colon-prompt-driven rather than single-key.
- **Multi-file navigation.** `tess` uses `:n` / `:p` / `:e` / `:d` /
  `:x` / `:t` matching `less`'s `:n` / `:p` / `:e` / `:d` / `:x` /
  `:t`. Where they overlap, behavior matches. We additionally provide
  `:b` (file picker overlay).
- **Filtering.** `tess` makes filtering a first-class CLI surface
  (`--filter`, `--grep`, `--format`, `--display`, `--prettify`)
  rather than relying on `less` + `grep | less`. There's no equivalent
  in `less` because `less` doesn't aim at structured-log workflows.
- **Config layout.** `~/.config/tess/keys.toml`, `formats.toml`. No
  binary keytable, no XDG-vs-home-dir fallback dance.

The flags `tess` accepts and how they behave are governed by `tess
--help` and `MANUAL.md`, not `less(1)`. When a `less` flag is picked
up, we match its surface and the spirit of its behavior; we don't
promise pixel-identical output.

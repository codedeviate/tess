# `tess` — Claude Code project notes

A `less`-style terminal pager written in Rust. macOS + Linux daily driver. Capabilities: follow mode (`-f` / `--follow`, interactive `Shift-F`) for `tail -f`-style log watching with rotation/truncation auto-reopen, `(F idle)` indicator after ~5s of silence, and optional `--follow-suspend-on-motion` for `less +F` semantics, `--head N` / `--tail N` for cheap views of huge files (`--tail` reverse-scans for the byte offset and only indexes from there forward), structured-log support (`--format <name>` parses lines via named regex captures; built-ins for apache-common, apache-combined, nginx-combined; user-defined formats in `~/.config/tess/formats.toml`), field-based filtering (`--filter FIELD<op>VALUE`, repeatable, AND'd) with optional `--dim` to keep non-matches visible but faded, raw-line regex filtering (`--grep PATTERN`, repeatable, AND'd, composable with `--filter`), OR-group filtering (`--or-filter FIELD<op>VALUE` / `--or-grep PATTERN` / `--or-group NAME` where conditions within a group are OR'd, every non-empty OR-group must have ≥1 match, and OR-groups are AND'd with required `--filter`/`--grep`; status line shows `[or]` when active; `<or-tag>` prompt placeholder), user-defined CLI groups (`[group.NAME]` in formats.toml expands `--<groupname>` into a fixed flag bundle and turns positionals into filters; groups support `filter`, `grep`, `or_filter`, `or_grep`, `display` fields, and `[group.NAME.or.<subname>]` sub-tables for named OR-groups), named layouts (`[layout.NAME]` with an `orientation` (`vertical` default / `horizontal`) + ordered `[[layout.NAME.pane]]` sub-tables that reuse the full `[group]` field vocabulary plus a required `file`; `load_layouts` mirrors `load_groups`/`promote_group`, `expand_layout_argv` rewrites `--<layoutname>` into the `--` per-pane form + an orientation before `split_argv_sections`, builders live in `crate::layout` (`build_panes_from_sections`/`build_second_pane`/`resolve_pane_predicates`/etc.), runtime `:layout NAME` rebuilds the focused-loose + `others` panes via `try_parse_from`; names disjoint from groups, mutually exclusive with `--split`/`--hsplit`/`--diff`/`--gitdiff`/`--right-*`, flat orientation only), multi-line records (`record_start` regex groups continuation lines; search/filter/grep operate on whole records), hex-dump display (`--hex`) for binary inputs, customizable status line (`--prompt TEMPLATE` or `prompt = '...'` per format), interactive regex search (`/`, `?`, `n`, `N`) with reverse-video row highlighting (smart-case via `-i`, force insensitive via `-I`, runtime cycle via `:case`; `-G` / `:hlsearch` / `:nohlsearch` toggle the visual highlight without changing navigation, `-g` / `--hilite-search` narrows highlighting to only the match last jumped to (the landed line's first match per row, tracked via `last_match_line`; `-G` still wins), `-a` / `--search-skip-screen` / `:search-skip-screen` anchors forward search at `bottom_visible_line` so on-screen matches are skipped (line/hide/records modes, per pane), `-p` / `--pattern` opens at the first match (flag form of `+/`); optional incremental search via `--incsearch` / `:incsearch` that previews + jumps to the first match as you type, Esc restoring the original position, Enter committing), a marked/match status column (`-J` / `--status-column`: far-left 1-col gutter showing a mark letter, else `*` on lines containing a current-search match — mark beats `*`; first wrap-row only, no-op in hex/raw/image, works even when `-G` suppresses highlight), explicit/variable tab stops (`-x` / `--tabs LIST` — comma list; single value == `--tab-width`, multiple values pin stops and repeat the last interval; overrides `--tab-width`), clipboard integration (`--from-clipboard` input source, `--to-clipboard` batch sink that applies filters/head/tail/prettify then copies + exits, `--clipboard` enabling interactive `:yank` / the unbound-by-default `clipboard-yank-line` binding; shells out to pbcopy/pbpaste / wl-copy/wl-paste → xclip → xsel, surfacing tool failures on the status line), drop-in `less` compat shims (`-R` / `--RAW-CONTROL-CHARS` no-op alias for default ANSI-interpret, conflicts with `-r`/`--no-color`; `-#` / `--shift N` for the `←`/`→` step size; `--wheel-lines N` for mouse-wheel body lines, default 3), `+CMD` startup commands (`+G` / `+NUM` / `+/pat` / `+?pat`), exit / startup control (`-X` skip alt-screen, `-F` quit-if-one-screen, `-e` / `-E` quit-at-eof, `-K` compat no-op), display tweaks (`-s` squeeze blanks, `--header=L,C` pin top rows + left cols, `--rscroll=c` chop-marker, `-z N` page-step size, `--wordwrap` whitespace-aware wrapping, `--tilde` / `:tilde` to show a dim `~` past EOF — opt-in, the inverse direction of `less -~`), `!cmd` shell escape (drops alt-screen, runs command, resumes on keypress), input preprocessing (`--preprocess '|cmd %s'` / `$LESSOPEN`) to pipe files through external tools before display, and user-remappable keybindings via `~/.config/tess/keys.toml` (including inline `!cmd` bindings), multi-file navigation (`:n` / `:p` / `:e` / `:d` / `:x` / `:t` colon commands; `file_set` module owns the working set; marks and previous-position slot are session-wide across files), and ctags/etags tag jumping (`-t NAME` startup jump, `-T PATH` explicit tags file, `:tag NAME` / `Ctrl-]` runtime prompt with `Tab` completion, `Ctrl-T` pop stack, `:tnext` / `:tprev` multi-match cycling, `:tselect` numbered picker overlay, chained `/foo/;/bar/` ctags addresses, graceful skip of unsupported address forms, auto-reload of the tags file when its mtime changes, `<tag-tag>` prompt placeholder), runtime ANSI mode toggle (`:color [strict|interpret|raw]` cycles or sets the SGR interpretation policy live), 24-bit→256 truecolor downsampling (`--truecolor=auto|never|always` with `$COLORTERM` detection), status/prompt theming (`--status-style`, `--prompt-style`, plus per-format `prompt_style` in formats.toml — accepts `bold,fg=cyan,bg=#ff0080`-style tokens), embedded SGR/escape sequences in `--display` / `--prompt` templates (`\e`, `\xHH`, `\NNN`, `\n`, `\t`, `\r`), and true `-r` raw passthrough that emits the source bytes verbatim past the cell pipeline (wrap math best-effort, matching `less -r`), horizontal scrolling (`←`/`→` half-screen, `Shift-←`/`Shift-→` 8-col step, and under `--mouse` a native horizontal swipe or `Shift`+scroll where the terminal reports it — terminal-dependent: iTerm2/kitty/WezTerm/xterm report one or both, while Warp and macOS Terminal.app report neither horizontal scroll nor scroll modifiers, so the keyboard is the only horizontal path there) for chop-mode text (`-S`) and images — line-number gutter stays fixed, text shows `<`/`>` edge markers while images shift cleanly with none, `»{col}` status readout and `<col-offset>` prompt placeholder when offset > 0; no-op in wrap/hex/raw; all four directions remappable in `~/.config/tess/keys.toml` (`hscroll-left`, `hscroll-right`, `hscroll-left-step`, `hscroll-right-step`); frozen left content-columns (`--header ,C`) pin the first C display columns in chop mode while the rest scrolls, via a two-pass `render_line` (frozen prefix + dim `│` divider + scrolled remainder) engaged only when `left_col > 0`; the hscroll clamp reserves the frozen region + divider, the `<` marker yields to the divider, a width-2 char straddling the boundary is blanked at the edge, and it works per pane in the split (each viewport's own `header_cols`), side-by-side vertical split view (`--split` launch flag, or runtime `:vsplit`/`:split` to open/duplicate and `:only`/`:close` to collapse; `Tab` switches the focused pane, remappable as `focus-other-pane`; two independent viewports — each with its own scroll/search/follow state — composited by the pure `pane::compose_split` with a divider column and per-pane half-width status, the focused pane's prefixed `*`; N vertical panes — `--split a b c` opens one pane per file (`others: Vec<Pane>` + focused-loose-locals + `focused_pos`; `pane::split_widths_n`/`compose_panes`; `Tab`/`BackTab` cycle via `rotate_focus`); scroll-lock (`=`) couples all panes by per-pane offsets relative to pane 0 (`locked_pane_top`, `lock_offsets: Vec<isize>`); horizontal (stacked) split via a whole-split `Orientation{Vertical,Horizontal}` (0.54.0 — `--hsplit`/`:hsplit`/`:rotate`): the pure duals `pane::split_heights_n` (MIN 2, no divider row — each pane's own status row separates, heights sum to exactly `rows`) + `compose_panes_horizontal` (stacks frames, non-bottom statuses become body separator rows, bottom pane's status lifts to `frame.status` so `body.len()==rows-1` fits write_frame unchanged) + `pane::pane_at_row` (mouse routing by `me.row`); the ~4 sizing sites (`panes_init`/`resize_split_aware`/render/mouse) branch on orientation, Vertical = byte-identical to before; `:rotate` flips the live split (re-derives sizes, no-op in diff/single-pane), diff stays vertical-only; nested grids (a recursive layout tree) deferred to Sub-project 2); each pane needs ≥8 cols or the focused pane stays full-width; aligned diff (`:diff`) requires exactly 2 panes; `--diff` requires exactly 2 files; vertical only, the second pane shows the plain file with no `--filter`/`--grep`/`--format`/`--display` predicates, the cell-based compositor renders protocol images as ASCII and `-r` raw through cells while split, and horizontal-split/diff-alignment are deferred (per-pane frozen left columns now work for startup-configured panes); the `--` per-pane argv form — `tess a --grep X -- b --grep Y -- c` — where a standalone `--` splits argv into one view-spec section per pane via the pure `cli::split_argv_sections` (falls back to a single section — POSIX end-of-options — whenever any section would be empty, so `tess -- -dashfile` and a trailing `tess a --` still work), section 0 runs the full preprocessing pipeline (`+CMD`/group-expand/OR/clap) carrying globals + the focused pane, later sections group-expand + clap-parse into their own `Args` built by `build_panes_from_sections` (file + per-view `--grep`/`--filter`/`--format`/`--display`/`--encoding`/`-i`/`-I` + display flags; per-section predicates via the same `resolve_pane_predicates` that `build_second_pane` now takes pre-resolved — that refactor also moved encoding resolution into `build_second_pane`, per-pane from its own head); additive & mutually exclusive with `--split`/`--right-*` (`validate_per_pane_argv`), `--diff a -- b` needs exactly 2 sections, section 0 caps at one file, OR-groups/`+CMD` are section-0-only; under `--mouse` the wheel (all axes) scrolls the pane the cursor is over rather than the focused pane, via the pure `pane::pane_at_column` hit-test against the live `split_widths_n` layout — no focus change, bypassed under scroll-lock/diff/single-pane; a left-button press (0.56.1) instead focuses the pane under the cursor via the same `pane_at_column`/`pane_at_row` hit-test + the pure `set_focus(target)` (which `rotate_focus` now delegates to) — works in both orientations and under scroll-lock like `Tab`, no-op in diff/single-pane/already-focused, the press always consumed); pane zoom (0.57.0 — `Ctrl-X z` chord / `:zoom` colon command / `zoom-pane` bindable key name; `zoomed: bool` loop-local mirrors `scroll_lock`; `toggle_zoom` enters by resizing the focused pane to full-screen + `viewport.set_zoomed(true)`, exits via `resize_split_aware`; render branch gates on `zoomed` — when true the focused pane fills the whole terminal, `others` are hidden but keep following/tailing; resize handler branches on `zoomed`; auto-unzoom on `rotate_focus`/`:vsplit`/`:hsplit`/`:rotate`/`:only`/`:layout`/`:diff` via `unzoom` helper; no-op + flash in single-pane or diff mode; `[zoom]` status badge on focused pane, `<zoom>` prompt placeholder; `zoom-pane` maps to `Command::ZoomPane`; no config default, no `:nozoom`, `ctrl-shift-x` deliberately unbound by default), synchronized split scrolling (`=` / `:scrolllock` / `--scroll-lock` toggle a relative line-offset lock between the two split panes — captured at enable in stable physical left/right terms so `Tab` is alignment-invariant; the non-focused pane is re-derived each frame via the pure `pane::locked_partner_top` from the fixed offset, clamped, so EOF/top clamps hold without drift and restore; coupling is by logical line; the focused pane drives follow/tail while locked; `[lock]` badge on the focused pane, `<lock>` prompt placeholder, `scroll-lock-toggle` keys.toml name; line-based only — diff alignment deferred), and image-to-ASCII rendering (`tess cat.png` auto-detects PNG/GIF/JPEG/BMP/WebP/TIFF/TGA/ICO/PNM by magic bytes and renders colored ASCII art; `--blocks` for Unicode half-block mode, `--image-width N` to scale, `--no-image` to view raw bytes; animated images auto-play in all render modes (animated GIF — primary, fully tested — plus APNG/WebP where the decoder supports them, honoring the GIF loop count and resting on the last frame, loop count 0/absent = infinite the common case; `p` pause/resume, `.`/`,` step fwd/back, `Backspace` restart — all remappable as `anim-pause`/`anim-step-forward`/`anim-step-back`/`anim-restart`, no-op without an animation; `--no-animate` opens the static first frame; `[play i/n]`/`[pause i/n]`/`[done n/n]` status badge; batch export `-o`/`--stdout` emits a static frame); export the art with `-o FILE`/`--stdout` — ANSI-colored by default, plain under `--no-color`; plus true-pixel rendering via the Kitty graphics protocol or Sixel (`--image-protocol auto|kitty|sixel|ascii`, default `auto` terminal-detects with ASCII fallback, Kitty>Sixel — Kitty on Kitty/iTerm2/WezTerm/Ghostty, Sixel on foot/xterm/mlterm/WezTerm; fit-to-width with vertical scroll, `←`/`→` no-op in protocol mode; `[kitty]`/`[sixel]` status; explicit `kitty`/`sixel` + `-o`/`--stdout` writes the encoded escape bytes; protocol animation re-emits each frame); behind the default-on `image` Cargo feature), and charset decoding for non-UTF-8 input (`--encoding LABEL` / runtime `:encoding LABEL`; the `charset` module wraps `encoding_rs` behind `Encoding`/`decode_line`, threaded onto `RenderOpts.encoding`; the UTF-8 path is byte-identical including `<HH>` for invalid bytes, other charsets decode-then-pipeline; rendering AND matching — search/`--grep`/`--filter`/`--format`/OR-groups — operate on the decoded text; BOM honored only at the default with explicit `--encoding` winning; copy/export emit decoded UTF-8; `-r`/`--hex` bypass; WHATWG maps `iso-8859-1`/`latin1` to the windows-1252 decoder; UTF-16 is rejected because the line index splits on a lone `0x0A` that UTF-16 embeds), and aligned side-by-side diff mode (`--diff A B` / `:diff` / `:diff!` / `:nodiff` / `:diffws`; the pure `diff` module wraps `similar` for a `Vec<DiffPair>` line alignment Equal/Changed/Added/Removed + `char_spans` intra-line + `hunks`/`next_hunk`/`prev_hunk`; `diff_view::compose_diff` is a dedicated pure renderer bypassing `compose_split` — pair-padding wrap (pair height = max of both sides, shorter padded with fillers), gutter signs + class colors + charset-aware intra-line char highlighting on single-row changed pairs; `app::run` holds `DiffState{pairs,pos:(pair,sub_row),ignore_ws,hunk_total}` + `build_diff` with a ~500k-line cap overridable by `:diff!`/`--diff-force`; `]c`/`[c` hunk nav via bracket-prefix chord; `[diff i/n]` status; follow/live suspended (snapshot); diff honors `--encoding` via `diff_pane_opts.encoding = viewport.encoding()` while line keys stay byte-based; `Tab` is locked in diff mode to avoid swapping `src`/`idx` out from under the pairs; `--gitdiff FILE` reuses this whole diff pipeline as a source-acquisition feature — the new `git` module (`classify` pure stderr→FailKind, `resolve`, `head_blob` via `git show HEAD:<rel>`) supplies the left/old pane as a `MemorySource` of the HEAD blob while the right/new pane is the working-tree file (`build_pane_from_source`, extracted from `build_second_pane`, builds either; empty `MemorySource` for new-file/deletion sides), the startup diff branch triggers on `args.diff || args.gitdiff`, validated single-file + in-repo + mutually exclusive with `--diff`/`--split`/`--right-*`/`--` form; single-file; revisions + staged shipped 0.53.0 — pure `parse_gitdiff_spec` maps leading positionals (last = file, 0/1/2 before = revs) + `--staged`/`--cached` to a `GitDiffSpec{file,left_rev,right:WorkingTree|Index|Rev}`, `resolve_gitdiff_sources` runs git once to produce left bytes + right `GitDiffRight{WorkingTree|Blob}` with a both-absent error; `git::rev_blob`/`index_blob` + the 4-way `classify` Absent/NoCommits/BadRev/Other give empty-side for path-absent, friendly "no commits"/"bad revision" errors; multi-file/rename + `R1..R2` token deferred; known: a working-tree-deleted file mis-errors in `git::resolve` canonicalization — pre-existing), and per-pane / runtime predicates (predicates are already per-viewport, so runtime colon commands `:grep`/`:filter`/`:format`/`:display` + `:no*` set them on the FOCUSED viewport in `dispatch_colon_command` — per-pane via `Tab`, and tess gains live filtering generally; `:filter`/`:display` need a format, `:noformat` also clears the filter; startup `--right-grep`/`--right-filter`/`--right-format`/`--right-display` seed pane B in `build_second_pane` via the shared `resolve_pane_predicates` helper, `--right-filter` requires `--right-format`; after any predicate change a hide-mode pane re-runs `idx.extend_to_end` + `viewport.extend_visible_lines` to rebuild the visible-line cache — omitting this blanks the pane; OR-groups/`--dim` stay global/startup-only, predicates inactive in diff; per-pane case at startup via `--right-ignore-case`/`--right-IGNORE-CASE` — an independent `right_case_mode` (default sensitive, NOT inheriting global `-i`/`-I`) threaded into pane B's predicate compile + `viewport.set_case_mode`; runtime per-pane case already works via `:case` on the focused pane), and mouse capture on by default (`--no-mouse` disables at startup, `--mouse` kept as an explicit-on alias, conflicts enforced by clap; `:mouse` runtime toggle / `:mouse on` / `:mouse off` set state live; `mouse-toggle` keybinding name (unbound by default, user-addable in `keys.toml`); `[settings] mouse = true|false` in `formats.toml` is the first key of the new `[settings]` table and sets a persistent default override — CLI flags win; `[nomouse]` status badge (all panes) while capture is off, `<mouse>` prompt placeholder renders `nomouse`/empty; most terminals disable native text selection under capture — hold Shift, or Option on iTerm2/macOS, to select, or use `--no-mouse`). See `OUT-OF-SCOPE.md` for the full deferred list.

## Build, run, test

```
cargo build --release           # binary at target/release/tess
cargo test                      # unit + integration tests
cargo run -- Cargo.toml         # quick interactive run
ls -la | cargo run --release    # piped stdin
```

## Module layout

The codebase splits into small, single-purpose units under `src/`. Dependencies flow downward — no upward edges.

```
cli (clap parsing) ─→ main ─→ app (event loop) ─→ viewport (scroll state, frame composition)
                                                    │
                                                    ├→ render (pure: bytes → display cells)
                                                    ├→ source (FileSource mmap, StdinSource buffer, MockSource)
                                                    └→ line_index (lazy line-start offsets)

ansi   (parser: SGR, CSI, OSC 8; Style/Color types)           used by render + line_index
terminal (RAII guard, panic hook, signal flag)        used by main + app
error (Error enum, exit codes)                        used everywhere
input (KeyEvent → Command translation)                used by app
hex    (xxd-style row formatter)                      used by viewport
prompt (--prompt template parser; wraps format::DisplayTemplate)  used by viewport
shell  (!cmd and shell-command binding helper)         used by app
preprocess (--preprocess / $LESSOPEN resolver and runner)  used by main
keys   (~/.config/tess/keys.toml loader and dispatch interceptor)  used by app
file_set (multi-file working set: paths, cursor, next/prev/delete semantics)  used by app
open   (source-construction helper: path → Source + LineIndex)  used by main + app
tags   (ctags/etags parser, tag-stack, multi-match cursor)        used by app
```

- **`render` is the kernel.** Pure functions, no I/O, no terminal. The hard rules (UTF-8 cluster decode, tab stops, control-byte `^X` form, invalid-byte `<HH>` form, wrap vs chop with width-2 char boundaries) all live here so they get the densest unit-test coverage.
- **`source` abstracts byte sources.** `FileSource` mmaps the original content and keeps a separate file handle for streaming follow-mode reads (new bytes go into an appended `Vec`). `StdinSource` has two modes — synchronous `read_all` (no `-f`) or threaded `spawn_streaming` (with `-f`); the streaming variant dups stdin onto a private fd before main can `dup2` `/dev/tty` over fd 0. `MockSource` is for tests.
- **`line_index` lazily scans for newlines** (and optional record-start matches when a `record_start` regex is set), supporting incremental growth via `notice_new_bytes`.
- **`viewport` owns scroll state** (`top_line`, `top_row` for wrap-aware scrolling) and composes `Frame { body, status }`. It uses `render::count_rows` to compute scroll math without allocating cells.
- **`app::run` is the event loop**: render-on-change → `poll(250ms)` → dispatch. On `poll()` error, sleep the timeout to avoid spinning.

## Key design decisions worth knowing

- **`crossterm` uses the `use-dev-tty` feature.** The default mio-based event source in 0.27 fails on macOS with piped stdin (`Failed to initialize input reader`). The `use-dev-tty` alternative uses `poll(2)` + `signal-hook` pipes and works in both file and pipe modes.
- **Stdin path uses `dup2` to redirect fd 0 to `/dev/tty`** *only when stdin was actually drained from a pipe*. In file mode we leave fd 0 alone — replacing the shell's healthy tty fd breaks crossterm's event source init.
- **Byte-faithful rendering is now opt-in via `--no-color`.** By default we interpret ANSI SGR escapes (colors, bold, underline, etc.) and OSC 8 hyperlinks; non-SGR CSI (cursor moves, screen clears) is silently stripped to protect layout. UTF-8 grapheme clusters still decoded via `unicode-segmentation`; widths via `unicode-width`. Pre-0.18 byte-faithful behavior is one flag (`--no-color`) away.
- **Body height = `rows - 1`.** Last row is the status line (`<label>  <top>-<bottom>/<total>  <pct>%`, `+` suffix on total when source is incomplete).
- **No async runtime.** Sync main loop. Without `--follow`, stdin is read synchronously up-front. With `--follow`, a background thread reads from a duped stdin fd into a shared buffer, and the main loop's timeout branch calls `src.pump()` to fold any new bytes into the index, auto-scrolling to bottom when the viewport was at bottom before the growth.
- **Flicker-free frames rely on synchronized output (DEC private mode 2026).** Each frame is wrapped in `\x1b[?2026h … \x1b[?2026l` (`SYNC_UPDATE_BEGIN`/`END` in `app.rs`) so the terminal presents the whole repaint atomically. **Warp does not honor mode 2026** (nor scroll modifiers / horizontal scroll — see the capability line) — so on Warp a continuous full-screen repaint (e.g. image animation) can tear. Mitigate by minimizing per-frame churn rather than relying on 2026: the status row is drawn *in place* (one display-width-padded write, no clear-then-print gap — `write_status_row`), which is what fixed the Warp animation status-bar flicker in 0.40.1. Keep that principle for any new continuous-repaint feature.

## Testing strategy

- `render` is exhaustively unit-tested — every byte category, tab stops at varied positions, multi-byte UTF-8, wide chars at boundaries, wrap vs chop. Plain inputs, plain outputs, no terminal needed.
- `line_index` and `viewport` use `MockSource` to simulate growing inputs and exercise scroll math.
- One integration test (`tests/golden_frame.rs`) exercises the full `FileSource → LineIndex → Viewport → render` flow against a fixture.
- The terminal layer is verified manually — no PTY-based tests in MVP (high effort, low value before features that warrant it).

## Conventions

- Conventional commits: `feat(scope):`, `fix(scope):`, `test:`, `chore:`.
- Errors via a small handwritten `enum Error` in `src/error.rs` — no `anyhow` for MVP. Exit codes: 0 clean, 1 startup error, 2 runtime error.
- Don't write code comments unless they capture *why* (a hidden constraint, a workaround, a non-obvious invariant). The names should carry the *what*.

## Versioning

Use [Semantic Versioning](https://semver.org). The single source of truth is `version` in `Cargo.toml`.

- **PATCH** (`0.2.0` → `0.2.1`): bug fixes, doc-only changes, internal refactors with no user-visible behavior change.
- **MINOR** (`0.2.0` → `0.3.0`): new flags, new features, new config keys; backwards-compatible changes to existing behavior.
- **MAJOR** (`0.x` → `1.0` once stable; afterwards `1.x` → `2.0`): incompatible CLI/config changes, removed flags, behavior breaks.

Bump the version in `Cargo.toml` as part of the same commit that introduces the change. Pre-1.0 we permit small breakage at MINOR boundaries when called for, but flag it in the commit message.

**Tagging implies three downstream publishes — same release, no exceptions.** When you push a `vX.Y.Z` tag, the release isn't complete until all three surfaces below are updated. A tag with only one or two updated leaves install paths out of sync: shields.io badges turn stale, `cargo install tess-cli` and `brew upgrade tess` keep returning the old version, and anyone following the README installs an older binary. Treat all three as part of the tag — same flow, no follow-up commits needed on this repo itself.

### 1. GitHub release

```sh
gh release create vX.Y.Z --generate-notes
```

(If you're also shipping the `.deb` artifacts described in the "Linux release artifacts" section below, `gh release upload` is the follow-up — but that's adding assets to the same release, not creating a second one.)

### 2. crates.io

From the repo root, with the new version already in `Cargo.toml`:

```sh
cargo publish
```

The crate name is **`tess-cli`** (per `Cargo.toml [package] name`). The binary inside the crate is still **`tess`**. Requires `cargo login` to have been run once (token stored in `~/.cargo/credentials.toml`). `cargo publish` runs its own checks (clean working tree, no path-dependencies, etc.) and aborts cleanly if anything is wrong — fix the underlying issue rather than passing `--allow-dirty`.

### 3. Homebrew tap (`../homebrew-cli/`)

The tap repo at `../homebrew-cli/` carries `Formula/tess.rb`. Update two fields:

- `url "https://github.com/codedeviate/tess/archive/refs/tags/vX.Y.Z.tar.gz"`
- `sha256 "<new-tarball-sha256>"`

Compute the sha256 from the GitHub-generated tarball after the release exists. **Always pass `-H "Cache-Control: no-cache"` so the fetch bypasses any intermediate caches (your ISP, corporate proxy, local resolver) and goes through to GitHub's origin:**

```sh
curl -sL -H "Cache-Control: no-cache" \
    https://github.com/codedeviate/tess/archive/refs/tags/vX.Y.Z.tar.gz \
    | shasum -a 256
```

**Important: GitHub's auto-generated tarball CDN can serve a transient/incomplete payload for the first minute or two after the tag is pushed.** Run the `shasum` command twice with a short pause between, and only proceed if both runs return the same hash. If they differ, wait 30–60 seconds and re-check until the hash stabilises. Using an unstable hash is the single most common cause of "homebrew reports wrong checksum" reports after a release — the tap history (`git log --oneline ../homebrew-cli`) shows multiple `... — fix sha256` follow-ups across recon, batty, webrunner. recon v0.85.0 hit this exact race because the recheck without `Cache-Control: no-cache` re-read the same cached payload twice (a "stable" but wrong hash); the mismatch surfaced only when users ran `brew install`.

The `Cache-Control: no-cache` rule isn't optional even on a "fresh" shell. Network paths cache aggressively; a single `curl` without the header is allowed to return whatever was last cached for that URL — which can be an early CDN payload that no longer matches what GitHub serves to homebrew clients.

Then commit and push the tap repo:

```sh
cd ../homebrew-cli
git add Formula/tess.rb
git commit -m "tess X.Y.Z"
git push origin main   # tap default branch is `main`, not `master`
```

(Tap commits follow the convention `<formula> X.Y.Z` — see `git log --oneline` in `../homebrew-cli` for examples.)

If `shasum` produces a hash that, after pasting into `tess.rb`, makes `brew install --build-from-source tess` fail with "SHA256 mismatch", recompute against the URL the formula points to (case matters — `vX.Y.Z` not `VX.Y.Z`) and amend with a follow-up commit like the existing `... — fix sha256` precedents in the tap log.

## Build / packaging discipline

After every commit on this branch:

1. **Build the release profile** (the debug profile is skipped by default):
   ```
   cargo build --release
   ```
   Skip `cargo build` (debug). If the debug profile is actually needed (e.g. for a debug-only repro), build it on its own or wait for an explicit request — don't bundle it into the post-commit chore by default.
2. **Generate a source tarball** of everything needed to compile `tess` on another machine, named `tess-<version>.tar.gz` (where `<version>` matches `Cargo.toml`), placed in the repo root next to this `CLAUDE.md`. Contents: `Cargo.toml`, `Cargo.lock`, `src/`, `tests/`, `benches/`, `man/`, `README.md`, `MANUAL.md`, `MANUAL.pdf`, `CLAUDE.md`, `OUT-OF-SCOPE.md`, `INSTALL.md`, `LICENSE`, `.gitignore`. Excluded: `target/`, `.git/`, `.claude/`, any `.DS_Store`. The tarball is `.gitignore`d (see `tess-*.tar.gz`).
3. **Regenerate the man page** when CLI flags or behavior change:
   ```
   cargo run --release --bin gen-manpage
   ```
   Output: `man/tess.1`. Commit it alongside the change.
4. **Regenerate `MANUAL.pdf`** when `MANUAL.md` changes:
   ```
   scripts/gen-manual-pdf.sh
   ```
   Output: `MANUAL.pdf` (repo root). Commit it alongside the change. Requires
   `recon` on PATH (Homebrew: `brew install recon`); `awk` is POSIX-standard.
   Self-contained typst engine — no Chrome / agent-browser needed.

If a commit only touches docs and doesn't change the version, the tarball can be skipped — the previous one is still current. If the version bumped, regenerate.

## Linux release artifacts (`.deb` for amd64 + arm64)

On **every release** (i.e. whenever the version in `Cargo.toml` bumps and gets tagged), also produce statically-linked musl `.deb` packages for the two Linux architectures we ship: `amd64` (`x86_64`) and `arm64` (`aarch64`). These belong in the GitHub release as upload assets.

Prerequisites (one-time, host machine — macOS):

- `cargo install cargo-zigbuild cargo-deb`
- `brew install zig`
- `rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl`
- `~/.cargo/bin` must come **before** `/opt/homebrew/bin` on `PATH`, otherwise Homebrew's `rustc` (which only has the host target) intercepts the build and you get `error[E0463]: can't find crate for core/std` even though the targets are installed. The rustup-managed toolchain is the one that knows about Linux targets.

Build sequence — run from the repo root after the release commit lands:

```sh
for tgt in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
  cargo zigbuild --release --target "$tgt"
  cargo deb --target "$tgt" --no-build
done
```

Outputs:

- `target/x86_64-unknown-linux-musl/release/tess`  — ELF, statically linked musl, x86-64
- `target/aarch64-unknown-linux-musl/release/tess` — ELF, statically linked musl, aarch64
- `target/debian/tess-cli_<version>-1_amd64.deb`
- `target/debian/tess-cli_<version>-1_arm64.deb`

Note: Debian calls the 64-bit ARM arch `arm64`; Rust/Linux call it `aarch64`. They are the same architecture — one target triple, not two. `cargo deb` translates the Rust target triple to the Debian arch name automatically.

The `cargo deb` step emits a `warning: Command dpkg-shlibdeps failed to launch` line. That is a Debian-only tool we don't have on macOS; for static musl binaries there are no shared-lib deps to auto-detect, so the warning is harmless and the `.deb` is still produced correctly.

Attach both `.deb` files to the GitHub release:

```sh
gh release upload vX.Y.Z \
  target/debian/tess-cli_X.Y.Z-1_amd64.deb \
  target/debian/tess-cli_X.Y.Z-1_arm64.deb
```

Also attach the manual PDF to the release:

```sh
gh release upload vX.Y.Z MANUAL.pdf
```

(If the release was created with `--generate-notes` per the Versioning section above, this is the follow-up step that adds the binaries to it.)

## Where to put new work

- New design specs → `~/Development/Starweb/superpowers/tess/specs/YYYY-MM-DD-<topic>-design.md`
- Implementation plans → `~/Development/Starweb/superpowers/tess/plans/YYYY-MM-DD-<feature>.md`
- Session reports (after a brainstorm → plan → implement cycle) → `~/Development/Starweb/superpowers/tess/reports/YYYY-MM-DD-<feature>.md`

The MVP design lives in `specs/2026-04-27-rust-less-clone-design.md`; the implementation plan in `plans/2026-04-27-rust-less-clone-plan.md`; session report in `reports/2026-04-27-rust-less-clone.md`.

## WISHLIST.md → OUT-OF-SCOPE.md

`WISHLIST.md` (repo root) is the user's raw idea inbox: they jot feature ideas
there — one per line, terse, unformatted — without touching `OUT-OF-SCOPE.md`. The
agent owns moving them across.

**When there's nothing else to do** (idle between tasks, or after finishing a
cycle), check `WISHLIST.md`. If it has any idea lines:

1. For each idea, write a proper `OUT-OF-SCOPE.md` entry — expand the terse note
   into a real entry (a `### Title — **S/M/L**` heading + a short paragraph
   capturing the idea, design considerations, and trade-offs, the same style as
   existing entries) and place it in the right bucket (**Waiting** = nice-to-have
   nobody's urgently asked for; **Deferred** = actively put off; **Out of scope**
   = won't pursue; **Not yet supported** = blocked upstream). Use judgment on
   bucket + size.
2. Empty `WISHLIST.md` back to just its header (keep the header lines; remove the
   idea lines).
3. Commit both files together (`chore: move wishlist idea(s) into OUT-OF-SCOPE`).

Don't ask the user to confirm routine moves — just do it correctly. If an idea is
ambiguous or large enough to need its own brainstorm, still capture it in
OUT-OF-SCOPE (as a candidate) rather than leaving it in WISHLIST. The goal is that
WISHLIST.md sits empty, and every idea lands in OUT-OF-SCOPE.md properly worded.

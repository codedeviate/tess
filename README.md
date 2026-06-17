# tess

[![GitHub](https://img.shields.io/badge/github-codedeviate%2Ftess-181717?logo=github)](https://github.com/codedeviate/tess)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue?logo=opensourceinitiative)](LICENSE)
[![Rust edition 2021](<https://img.shields.io/badge/rust-2021_edition_(MSRV_1.85)-CE422B?logo=rust>)](https://www.rust-lang.org)
<br/>
[![Latest release](https://img.shields.io/github/v/release/codedeviate/tess?logo=semanticrelease&label=release&color=blue)](https://github.com/codedeviate/tess/releases/latest)
[![crates.io](https://img.shields.io/badge/crates.io-tess--cli-fc8d62?logo=rust)](https://crates.io/crates/tess-cli)
[![Homebrew](https://img.shields.io/badge/homebrew-codedeviate%2Fcli%2Ftess-fbb040?logo=homebrew)](https://github.com/codedeviate/homebrew-cli)

A `less`-style terminal pager for files, pipes, and live logs — with first-class support for **structured-log filtering**, **pretty-printing** of JSON/YAML/TOML/XML/HTML/CSV, **ANSI color passthrough** for piped tools that emit colored output, **wrap-aware scrolling** of long lines, **incremental search** (`--incsearch`), **variable tab stops** (`-x`/`--tabs`), a marked/match **status column** (`-J`), **clipboard integration** (`--from-clipboard` / `--to-clipboard` / `--clipboard`), **multi-file navigation** with `:n`/`:p`/`:e` colon commands, and **ctags/etags tag jumping** (`-t NAME`, `Ctrl-]`/`Ctrl-T`). Drop-in `less` compatibility flags (`-R`, `-#`/`--shift`, `--wheel-lines`) round out the surface. Escape to the shell with `!cmd`, preprocess inputs (`$LESSOPEN` / `--preprocess`), and remap keys via `~/.config/tess/keys.toml`. Written in Rust. macOS + Linux.

```sh
tess /var/log/syslog                                # like `less`, but newer keys
git log | tess                                      # pipe-friendly
tess -f --tail 1000 huge.log                        # tail -f the last 1000 lines
tess --format apache-combined --filter status~^5 access.log
tess --prettify config.json                         # auto-detected JSON layout
```

---

## Why?

`less` is great, but it doesn't know what your data _is_. `tess` adds a small amount of structure-awareness without trying to be a log explorer:

- **Parse-aware filtering.** Declare a log format once (built-in or your own regex), then keep only the lines whose parsed `status`, `ip`, `level`, etc. match a predicate. Hide them entirely or just dim them so context is preserved.
- **Multi-line records.** Set `record_start` in formats.toml (or pass `--record-start REGEX`) to group continuation lines under their leading record — search, filter, and `--grep` then operate on whole records like PHP stack traces or Java exception dumps.
- **Pretty-printing.** Open a JSON / YAML / TOML / XML / HTML / CSV file and see it laid out, not minified — without leaving the pager. Toggle on/off with `Shift-P`.
- **Live + follow modes.** `--follow` for tail-style appended logs (background reader thread, doesn't fight your tty); `--live` for whole-file rewrites (editor saves, AI agents, build outputs).
- **Cheap on huge files.** mmap'd file source, lazy line indexing, `--tail N` reverse-scans only as far as needed.
- **Long-line scrolling that actually works.** `j` walks wrap-rows so the last 1000 chars of a 5000-char line are reachable; `J` / `K` jump by whole logical lines when you don't want to scroll through every wrap.

It is **not** a log search/correlation tool, structured query engine, or replacement for `lnav`. It's a single-file pager.

---

## Install

### Homebrew (macOS / Linuxbrew)

```sh
brew install codedeviate/cli/tess
```

The formula lives in the [`codedeviate/homebrew-cli`](https://github.com/codedeviate/homebrew-cli) tap and builds from source on first install.

### crates.io

```sh
cargo install tess-cli
```

The crate is published as **`tess-cli`** because the bare `tess` name was already taken (an unrelated parked crate). The installed binary is still `tess`.

### From source

```sh
tar -xzf tess-<version>.tar.gz
cd <extracted-dir>
cargo build --release
install -m 755 target/release/tess ~/.local/bin/tess
```

Requires **Rust 1.85+**. See [INSTALL.md](INSTALL.md) for full instructions, including a macOS 26 (Tahoe) gotcha where `cp` into `/usr/local/bin` can get a binary SIGKILLed by the kernel — `install` avoids it.

---

## Quick start

| Goal                                  | Command                                                       |
| ------------------------------------- | ------------------------------------------------------------- |
| View a file                           | `tess Cargo.toml`                                             |
| View piped output                     | `git log \| tess`                                             |
| Watch a log live                      | `tess -f /var/log/syslog`                                     |
| Watch a file get rewritten            | `tess --live src/main.rs`                                     |
| Pretty-print JSON / YAML / etc.       | `tess --prettify config.json`                                 |
| Show line numbers                     | `tess -N script.sh`                                           |
| Don't wrap long lines                 | `tess -S /etc/hosts`                                          |
| Last 1000 lines (cheap on huge files) | `tess --tail 1000 huge.log`                                   |
| `tail -f` last 1000                   | `tess -f --tail 1000 huge.log`                                |
| First 50 lines                        | `tess --head 50 file.txt`                                     |
| Apache 5xx errors                     | `tess --format apache-combined --filter status~^5 access.log` |
| Navigate multiple files               | `tess foo.log bar.log baz.log` then `:n` / `:p`               |

Run `tess --examples` for a curated list, or `tess --manual` for the full manual paged through `tess` itself.

---

## Key bindings

Vim-ish + `less`-ish hybrid. The full table is in [MANUAL.md](MANUAL.md); these are the ones you'll use:

| Key(s)                        | Action                                                                 |
| ----------------------------- | ---------------------------------------------------------------------- |
| `↓` `j` `e` `Ctrl-E` `Return` | Scroll down one screen line (walks through wrap rows)                  |
| `↑` `k` `y` `Ctrl-Y`          | Scroll up one screen line                                              |
| `J` / `K`                     | Jump to next / previous _logical_ line (skips wrap rows of long lines) |
| `Space` `f` `Ctrl-F` `PgDn`   | Page down                                                              |
| `b` `Ctrl-B` `PgUp`           | Page up                                                                |
| `g` / `G`                     | Go to top / bottom                                                     |
| `/` _pattern_ `Enter`         | Forward regex search; `Esc` cancels                                    |
| `?` _pattern_ `Enter`         | Backward regex search                                                  |
| `n` / `N`                     | Repeat last search forward / backward                                  |
| `←` / `→`                     | Scroll left / right by half the screen width (chop mode and image view; no-op when wrapping) |
| `Shift-←` / `Shift-→`        | Scroll left / right by 8 columns; under `--mouse`, a horizontal swipe or `Shift`+scroll does the same **if your terminal reports it** (iTerm2/kitty/WezTerm do; Warp & Terminal.app don't — use the keys there) |
| `-N` / `-S` / `Shift-F`       | Toggle line numbers / chop / follow mode                               |
| `Shift-P`                     | Toggle pretty-print on/off                                             |
| `Shift-R`                     | Force reload from disk (with `--live`)                                 |
| `:n` / `:p` / `:e <path>`     | Next file / previous file / open new file                              |
| `Ctrl-]` / `Ctrl-T`           | Tag jump / pop tag stack                                               |
| `q` `Q` `Ctrl-C`              | Quit                                                                   |

Pressing `/<Enter>` (or `?<Enter>`) with an empty pattern repeats the last search in the typed direction, the way `less` does it.

---

## Plain-text grep

If you don't have (or don't need) a parsed format, `--grep PATTERN` filters
by regex against the raw line. Repeatable, AND-combined.

```sh
tess --grep error app.log
tess --grep error --grep '^\[' app.log    # both patterns must match
tess --grep error --dim app.log           # show all, dim non-matches
```

`--grep` composes with `--filter` when both are set (line must match both):

```sh
tess --format apache-combined --filter status=500 --grep timeout access.log
```

---

## Structured log filtering

Three formats ship built-in: `apache-common`, `apache-combined`, `nginx-combined`. Drop a TOML file at `~/.config/tess/formats.toml` to define your own:

```toml
[format.app]
regex = '^(?P<ts>\S+) (?P<level>\w+) (?P<msg>.+)$'
```

Then:

```sh
tess --format app --filter 'level!=DEBUG' app.log         # hide DEBUG lines
tess --format app --filter 'level=ERROR' --dim app.log    # keep DEBUG dimmed for context
```

Operators: `=` (exact), `!=` (exact ≠), `~` (regex), `!~` (regex ≠). Multiple `--filter` flags AND together. Quote arguments with `'…'` in interactive shells so `!` doesn't trigger history expansion.

Run `tess --list-formats` to see what's available, including your custom ones.

---

## Groups (shortcut bundles)

Define reusable flag bundles in the same `formats.toml`. `tess --<name>` expands the group's flags inline; bare positionals after it become `--filter`s, and any flag you pass after the group token overrides the group's value:

```toml
[group.errors]
format = "app"
filter = ["level=ERROR"]
display = "<ts> <level> <msg>"   # group's own --display template
follow = true
```

```sh
tess --errors app.log                       # format + filter + display + follow
tess --errors --display '<msg>' app.log     # override the group's display
```

Groups accept `format`, `file`, `follow`, `tail`, `head`, `dim`, `line_numbers`, `chop`, `tab_width`, `display`, `filter`, `grep`, `or_filter`, and `or_grep`. Named OR-groups are defined as `[group.NAME.or.<subname>]` sub-tables — each sub-table's conditions are OR'd internally, and every non-empty OR-group must have ≥ 1 hit (groups are AND'd together). See the [manual](MANUAL.md#groups-command-line-shortcuts) for the full field reference.

```sh
# Show ssh lines matching any brute-force signature:
tess --format ssh --filter service=ssh \
  --or-grep 'failed password' --or-grep 'invalid user' access.log
```

---

## Status line

```text
<source>  <top>-<bottom>/<total>  <pct>%  +<wrap>/<wraps>  [<format>]  [filter|dim]  [/<search>]  [pretty:<type>]  (L)  (F)
```

Each segment appears only when relevant. The `+12/50` indicator surfaces wrap-row position when scrolled inside a long wrapping line, so `j` is visibly making progress.

---

## Images

`tess` auto-detects image files by magic bytes (PNG, JPEG, GIF, BMP, WebP, TIFF, TGA, ICO, PNM) and renders them as colored ASCII art directly in the pager. No flags needed — just open the file:

```sh
tess photo.png                                # colored ASCII art in the pager
tess logo.gif                                 # animated GIFs play automatically
tess --blocks --image-width 100 photo.png     # higher-detail half-block render
tess photo.png -o art.txt                     # save the art (ANSI color) to a file
tess photo.png --no-color --stdout > art.txt  # plain, portable text
```

Flags:

| Flag | Behavior |
| ---- | -------- |
| `--blocks` | Unicode half-block (`▀`) mode — twice the vertical resolution per terminal row. |
| `--image-width N` | Scale to N columns (default: terminal width). |
| `--no-image` | View raw bytes instead of rendering; useful with `--hex`. |
| `--no-animate` | Open an animated image as its static first frame instead of playing it. |
| `--image-protocol auto\|kitty\|sixel\|ascii` | Render at true pixel fidelity via the Kitty graphics protocol or Sixel (default `auto`). |

For true-pixel rendering, `--image-protocol` draws the image with the Kitty graphics protocol (Kitty / iTerm2 / WezTerm / Ghostty) or Sixel (foot / xterm / mlterm / WezTerm). `auto` detects terminal support and falls back to ASCII art (Kitty preferred over Sixel); explicit `kitty` / `sixel` / `ascii` skip detection. In protocol mode the image fits the terminal width and scrolls vertically; `←`/`→` are no-ops. With an explicit `kitty`/`sixel` plus `-o`/`--stdout`, tess writes the raw escape bytes (e.g. save a `.sixel`). The status line shows `[kitty]` / `[sixel]`.

Animated images play automatically — animated GIF (and APNG / WebP where the decoder supports them), in every render mode (ASCII art on any terminal; Kitty / Sixel on graphics-capable ones). Playback honors the GIF loop count and rests on the last frame when it ends (loop count `0` / absent loops forever). Transport keys: `p` pause/resume, `.` / `,` step forward / back, `Backspace` restart (all remappable, no-op without an animation). The status line shows `[play i/n]` / `[pause i/n]` / `[done n/n]`. Pass `--no-animate` to open the static first frame, and note that `-o`/`--stdout` export is always a single static frame.

Color output uses 24-bit truecolor SGR; pass `--no-color` for a plain character-only render. Export the art with `-o FILE` or `--stdout` — ANSI-colored by default, plain text under `--no-color`.

When `--image-width N` produces a render wider than the terminal, use `←`/`→` (or `Shift-←`/`Shift-→` for fine steps) to scroll the image sideways.

The `image` feature is on by default. Build without it (`--no-default-features`) for a smaller binary that treats all inputs as text.

---

## Command-line flags

The full set, alphabetical by long name — the same order `tess --help` lists them. Run `tess --manual` for the long-form descriptions.

| Flag | Description |
| ---- | ----------- |
| `--blocks` | Render images with Unicode half-blocks (`▀`) for ~2× vertical detail. |
| `-S, --chop-long-lines` | Chop long lines instead of wrapping. |
| `--content-type TYPE` | Force the `--prettify` content type (`auto`/`raw`/`json`/`yaml`/`toml`/`xml`/`html`/`csv`). |
| `--dim` | With `--filter`/`--grep`, dim non-matching lines instead of hiding them. |
| `--display TEMPLATE` | Render each parsed line through a `<field>` template (requires `--format`). |
| `--examples` | Print a curated list of usage examples and exit. |
| `--exit-follow-on-close` | In follow mode with piped stdin, exit when the writer closes the pipe. |
| `--filter FIELD<op>VALUE` | Filter visible lines by parsed field (repeatable, AND'd; requires `--format`). |
| `-f, --follow` | Follow the source for new bytes (like `tail -f`); toggle with `Shift-F`. |
| `--follow-name` | Follow by path (compat no-op — tess already re-opens on rotation/truncation). |
| `--follow-suspend-on-motion` | Any user motion suspends following (`less +F` semantics). |
| `--format NAME` | Apply a named log format (built-in or from `formats.toml`). |
| `--grep PATTERN` | Filter visible lines by regex on the raw line (repeatable, AND'd; no `--format` needed). |
| `--head N` | Show only the first N lines. Mutually exclusive with `--tail`. |
| `--header L[,C]` | Pin the top L lines (and left C columns) at the top of the viewport. |
| `--hex` | Render the source as an xxd-style hex dump. |
| `--hex-group N` | Hex bytes per group in `--hex` mode (`2`/`4`/`8`/`16`/`32`; default `4`). |
| `-i, --ignore-case` | Smart-case search (insensitive unless the pattern has an uppercase char). |
| `-I, --IGNORE-CASE` | Force case-insensitive search regardless of pattern. |
| `--image-protocol MODE` | Image rendering path: `auto`/`kitty`/`sixel`/`ascii` (default `auto`, detects terminal graphics). |
| `--image-width N` | Target width in columns for image rendering. |
| `-N, --LINE-NUMBERS` | Show line numbers. |
| `--list-formats` | Print available log formats and their fields, then exit. |
| `--live` | Re-read the file when its on-disk content changes. Mutually exclusive with `--follow`. |
| `--manual` | Print the full user manual and exit. |
| `--mouse` | Enable mouse capture (click picker/help rows, scrollwheel scrolls the body). |
| `--no-animate` | Open an animated image as its static first frame instead of playing it. |
| `--no-color` | Show raw control bytes as `^X`; disable SGR/OSC interpretation. |
| `-G, --no-hilite-search` | Disable search-match highlighting by default (navigation still works). |
| `--no-image` | Treat a detected image file as raw text instead of rendering it. |
| `-X, --no-init` | Don't enter the alt-screen; leave content in terminal scrollback. |
| `--no-preprocess` | Ignore `$LESSOPEN` for this invocation. |
| `-o, --output FILE` | Batch mode: write filtered output to FILE (`-` = stdout) and exit. |
| `--preprocess CMD` | Pipe the source through CMD before rendering (`%s` = file path). |
| `--prettify` | Pretty-print structured content (JSON/YAML/TOML/XML/HTML/CSV). |
| `--prompt TEMPLATE` | Replace the status line with a templated string. |
| `--prompt-style SPEC` | Style for `--prompt` output (e.g. `bold,fg=cyan`). |
| `-e, --quit-at-eof` | Quit when scrolling forward past EOF a second time (`less -e`). |
| `-E, --QUIT-AT-EOF` | Quit the first time EOF is reached (`less -E`). |
| `-F, --quit-if-one-screen` | Exit without paging if the source fits on one screen. |
| `-K, --quit-on-intr` | `less` compat no-op (tess always quits on `Ctrl-C`). |
| `-r, --raw-control-chars` | Pass every byte to the terminal raw, including escapes (`less -r`). |
| `--record-start REGEX` | Treat lines matching REGEX as record boundaries; others join the prior record. |
| `--rscroll CHAR` | Right-edge marker for chopped lines (default `>`; empty disables). |
| `-s, --squeeze-blank-lines` | Collapse runs of blank lines into one (`less -s`). |
| `--status-style SPEC` | Style for the status row (e.g. `reverse`, `fg=COLOR`). |
| `--stdout` | Synonym for `--output -`: write batch output to stdout. |
| `--tab-width N` | Tab stop width (default `8`). |
| `-t, --tag NAME` | Jump to tag NAME at startup (requires a tags file). |
| `-T, --tag-file PATH` | Path to the tags file (default: walk up from CWD for `tags`). |
| `--tail N` | Show only the last N lines (cheap on huge files). Mutually exclusive with `--head`. |
| `--truecolor MODE` | 24-bit RGB handling (`auto`/`never`/`always`). |
| `-z, --window N` | PageDown / PageUp step size in lines (default: full screen). |
| `--wordwrap` | In wrap mode, break on whitespace boundaries instead of mid-character. |

---

## Project status

Pre-1.0; the CLI is settling but small breakage at minor-version boundaries is permitted (and called out in commit messages). Daily-driver-quality on macOS and Linux. **No Windows support** — the stdin / `/dev/tty` plumbing is Unix-specific.

Architecture notes and module layout: [CLAUDE.md](CLAUDE.md). Deferred features and intentional non-goals: [OUT-OF-SCOPE.md](OUT-OF-SCOPE.md).

---

## Building from source

```sh
cargo build --release                  # binary at target/release/tess
cargo test -- --test-threads=1         # unit + integration tests (PTY tests need serial)
cargo bench                            # criterion baselines (HTML in target/criterion/)
cargo run -- Cargo.toml                # quick interactive run
ls -la | cargo run --release           # piped stdin
```

---

## License

MIT — see [LICENSE](LICENSE).

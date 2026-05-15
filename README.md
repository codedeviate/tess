# `tess`

A `less`-style terminal pager for files, pipes, and live logs — with first-class support for **structured-log filtering**, **pretty-printing** of JSON/YAML/TOML/XML/HTML/CSV, and **wrap-aware scrolling** of long lines. Written in Rust. macOS + Linux.

```sh
tess /var/log/syslog                                # like `less`, but newer keys
git log | tess                                      # pipe-friendly
tess -f --tail 1000 huge.log                        # tail -f the last 1000 lines
tess --format apache-combined --filter status~^5 access.log
tess --prettify config.json                         # auto-detected JSON layout
```

---

## Why?

`less` is great, but it doesn't know what your data *is*. `tess` adds a small amount of structure-awareness without trying to be a log explorer:

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

| Goal | Command |
|---|---|
| View a file | `tess Cargo.toml` |
| View piped output | `git log \| tess` |
| Watch a log live | `tess -f /var/log/syslog` |
| Watch a file get rewritten | `tess --live src/main.rs` |
| Pretty-print JSON / YAML / etc. | `tess --prettify config.json` |
| Show line numbers | `tess -N script.sh` |
| Don't wrap long lines | `tess -S /etc/hosts` |
| Last 1000 lines (cheap on huge files) | `tess --tail 1000 huge.log` |
| `tail -f` last 1000 | `tess -f --tail 1000 huge.log` |
| First 50 lines | `tess --head 50 file.txt` |
| Apache 5xx errors | `tess --format apache-combined --filter status~^5 access.log` |

Run `tess --examples` for a curated list, or `tess --manual` for the full manual paged through `tess` itself.

---

## Key bindings

Vim-ish + `less`-ish hybrid. The full table is in [MANUAL.md](MANUAL.md); these are the ones you'll use:

| Key(s) | Action |
|---|---|
| `↓` `j` `e` `Ctrl-E` `Return` | Scroll down one screen line (walks through wrap rows) |
| `↑` `k` `y` `Ctrl-Y` | Scroll up one screen line |
| `J` / `K` | Jump to next / previous *logical* line (skips wrap rows of long lines) |
| `Space` `f` `Ctrl-F` `PgDn` | Page down |
| `b` `Ctrl-B` `PgUp` | Page up |
| `g` / `G` | Go to top / bottom |
| `/` *pattern* `Enter` | Forward regex search; `Esc` cancels |
| `?` *pattern* `Enter` | Backward regex search |
| `n` / `N` | Repeat last search forward / backward |
| `-N` / `-S` / `Shift-F` | Toggle line numbers / chop / follow mode |
| `Shift-P` | Toggle pretty-print on/off |
| `Shift-R` | Force reload from disk (with `--live`) |
| `q` `Q` `Ctrl-C` | Quit |

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
pattern = '^(?P<ts>\S+) (?P<level>\w+) (?P<msg>.+)$'
```

Then:

```sh
tess --format app --filter 'level!=DEBUG' app.log         # hide DEBUG lines
tess --format app --filter 'level=ERROR' --dim app.log    # keep DEBUG dimmed for context
```

Operators: `=` (exact), `!=` (exact ≠), `~` (regex), `!~` (regex ≠). Multiple `--filter` flags AND together. Quote arguments with `'…'` in interactive shells so `!` doesn't trigger history expansion.

Run `tess --list-formats` to see what's available, including your custom ones.

---

## Status line

```
<source>  <top>-<bottom>/<total>  <pct>%  +<wrap>/<wraps>  [<format>]  [filter|dim]  [/<search>]  [pretty:<type>]  (L)  (F)
```

Each segment appears only when relevant. The `+12/50` indicator surfaces wrap-row position when scrolled inside a long wrapping line, so `j` is visibly making progress.

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

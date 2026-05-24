# `tess` — Claude Code project notes

A `less`-style terminal pager written in Rust. macOS + Linux daily driver. Capabilities: follow mode (`-f` / `--follow`, interactive `Shift-F`) for `tail -f`-style log watching with rotation/truncation auto-reopen, `(F idle)` indicator after ~5s of silence, and optional `--follow-suspend-on-motion` for `less +F` semantics, `--head N` / `--tail N` for cheap views of huge files (`--tail` reverse-scans for the byte offset and only indexes from there forward), structured-log support (`--format <name>` parses lines via named regex captures; built-ins for apache-common, apache-combined, nginx-combined; user-defined formats in `~/.config/tess/formats.toml`), field-based filtering (`--filter FIELD<op>VALUE`, repeatable, AND'd) with optional `--dim` to keep non-matches visible but faded, raw-line regex filtering (`--grep PATTERN`, repeatable, AND'd, composable with `--filter`), user-defined CLI groups (`[group.NAME]` in formats.toml expands `--<groupname>` into a fixed flag bundle and turns positionals into filters; groups support `filter` and `grep` fields), multi-line records (`record_start` regex groups continuation lines; search/filter/grep operate on whole records), hex-dump display (`--hex`) for binary inputs, customizable status line (`--prompt TEMPLATE` or `prompt = '...'` per format), interactive regex search (`/`, `?`, `n`, `N`) with reverse-video row highlighting (smart-case via `-i`, force insensitive via `-I`, runtime cycle via `:case`; `-G` / `:hlsearch` / `:nohlsearch` toggle the visual highlight without changing navigation), `+CMD` startup commands (`+G` / `+NUM` / `+/pat` / `+?pat`), exit / startup control (`-X` skip alt-screen, `-F` quit-if-one-screen, `-e` / `-E` quit-at-eof, `-K` compat no-op), display tweaks (`-s` squeeze blanks, `--header=L,C` pin top rows + left cols, `--rscroll=c` chop-marker, `-z N` page-step size, `--wordwrap` whitespace-aware wrapping), `!cmd` shell escape (drops alt-screen, runs command, resumes on keypress), input preprocessing (`--preprocess '|cmd %s'` / `$LESSOPEN`) to pipe files through external tools before display, and user-remappable keybindings via `~/.config/tess/keys.toml` (including inline `!cmd` bindings), multi-file navigation (`:n` / `:p` / `:e` / `:d` / `:x` / `:t` colon commands; `file_set` module owns the working set; marks and previous-position slot are session-wide across files), and ctags/etags tag jumping (`-t NAME` startup jump, `-T PATH` explicit tags file, `:tag NAME` / `Ctrl-]` runtime prompt with `Tab` completion, `Ctrl-T` pop stack, `:tnext` / `:tprev` multi-match cycling, `:tselect` numbered picker overlay, chained `/foo/;/bar/` ctags addresses, graceful skip of unsupported address forms, auto-reload of the tags file when its mtime changes, `<tag-tag>` prompt placeholder), runtime ANSI mode toggle (`:color [strict|interpret|raw]` cycles or sets the SGR interpretation policy live), 24-bit→256 truecolor downsampling (`--truecolor=auto|never|always` with `$COLORTERM` detection), status/prompt theming (`--status-style`, `--prompt-style`, plus per-format `prompt_style` in formats.toml — accepts `bold,fg=cyan,bg=#ff0080`-style tokens), embedded SGR/escape sequences in `--display` / `--prompt` templates (`\e`, `\xHH`, `\NNN`, `\n`, `\t`, `\r`), and true `-r` raw passthrough that emits the source bytes verbatim past the cell pipeline (wrap math best-effort, matching `less -r`). See `OUT-OF-SCOPE.md` for the full deferred list.

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

**Tagging implies a GitHub release.** When you push a `vX.Y.Z` tag, immediately create the matching GitHub release:

```
gh release create vX.Y.Z --generate-notes
```

A tag without a release leaves the GitHub Releases page out of sync with tag history and hides the version from anyone browsing the repo's front page. Treat the release as part of the tag — same step, no follow-up commits needed.

## Build / packaging discipline

After every commit on this branch:

1. **Build the release profile** (the debug profile is skipped by default):
   ```
   cargo build --release
   ```
   Skip `cargo build` (debug). If the debug profile is actually needed (e.g. for a debug-only repro), build it on its own or wait for an explicit request — don't bundle it into the post-commit chore by default.
2. **Generate a source tarball** of everything needed to compile `tess` on another machine, named `tess-<version>.tar.gz` (where `<version>` matches `Cargo.toml`), placed in the repo root next to this `CLAUDE.md`. Contents: `Cargo.toml`, `Cargo.lock`, `src/`, `tests/`, `benches/`, `man/`, `README.md`, `MANUAL.md`, `CLAUDE.md`, `OUT-OF-SCOPE.md`, `INSTALL.md`, `LICENSE`, `.gitignore`. Excluded: `target/`, `.git/`, `.claude/`, any `.DS_Store`. The tarball is `.gitignore`d (see `tess-*.tar.gz`).
3. **Regenerate the man page** when CLI flags or behavior change:
   ```
   cargo run --release --bin gen-manpage
   ```
   Output: `man/tess.1`. Commit it alongside the change.

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

(If the release was created with `--generate-notes` per the Versioning section above, this is the follow-up step that adds the binaries to it.)

## Where to put new work

- New design specs → `~/Development/Starweb/superpowers/tess/specs/YYYY-MM-DD-<topic>-design.md`
- Implementation plans → `~/Development/Starweb/superpowers/tess/plans/YYYY-MM-DD-<feature>.md`
- Session reports (after a brainstorm → plan → implement cycle) → `~/Development/Starweb/superpowers/tess/reports/YYYY-MM-DD-<feature>.md`

The MVP design lives in `specs/2026-04-27-rust-less-clone-design.md`; the implementation plan in `plans/2026-04-27-rust-less-clone-plan.md`; session report in `reports/2026-04-27-rust-less-clone.md`.

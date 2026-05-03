# `tess` — Claude Code project notes

A `less`-style terminal pager written in Rust. macOS + Linux daily driver. Capabilities: follow mode (`-f` / `--follow`, interactive `Shift-F`) for `tail -f`-style log watching, `--head N` / `--tail N` for cheap views of huge files (`--tail` reverse-scans for the byte offset and only indexes from there forward), structured-log support (`--format <name>` parses lines via named regex captures; built-ins for apache-common, apache-combined, nginx-combined; user-defined formats in `~/.config/tess/formats.toml`), field-based filtering (`--filter FIELD<op>VALUE`, repeatable, AND'd) with optional `--dim` to keep non-matches visible but faded, user-defined CLI groups (`[group.NAME]` in formats.toml expands `--<groupname>` into a fixed flag bundle and turns positionals into filters), and interactive regex search (`/`, `?`, `n`, `N`) with reverse-video row highlighting. No multi-file nav yet — see `OUT-OF-SCOPE.md` for the full deferred list.

## Build, run, test

```
cargo build --release           # binary at target/release/tess
cargo test                      # 62 unit + integration tests
cargo run -- Cargo.toml         # quick interactive run
ls -la | cargo run --release    # piped stdin
```

## Module layout

The codebase splits into seven small, single-purpose units under `src/`. Dependencies flow downward — no upward edges.

```
cli (clap parsing) ─→ main ─→ app (event loop) ─→ viewport (scroll state, frame composition)
                                                    │
                                                    ├→ render (pure: bytes → display cells)
                                                    ├→ source (FileSource mmap, StdinSource buffer, MockSource)
                                                    └→ line_index (lazy line-start offsets)

terminal (RAII guard, panic hook, signal flag)        used by main + app
error (Error enum, exit codes)                        used everywhere
input (KeyEvent → Command translation)                used by app
```

- **`render` is the kernel.** Pure functions, no I/O, no terminal. The hard rules (UTF-8 cluster decode, tab stops, control-byte `^X` form, invalid-byte `<HH>` form, wrap vs chop with width-2 char boundaries) all live here so they get the densest unit-test coverage.
- **`source` abstracts byte sources.** `FileSource` mmaps the original content and keeps a separate file handle for streaming follow-mode reads (new bytes go into an appended `Vec`). `StdinSource` has two modes — synchronous `read_all` (no `-f`) or threaded `spawn_streaming` (with `-f`); the streaming variant dups stdin onto a private fd before main can `dup2` `/dev/tty` over fd 0. `MockSource` is for tests.
- **`line_index` lazily scans for newlines** and supports incremental growth via `notice_new_bytes` (intended for streaming sources later).
- **`viewport` owns scroll state** (`top_line`, `top_row` for wrap-aware scrolling) and composes `Frame { body, status }`. It uses `render::count_rows` to compute scroll math without allocating cells.
- **`app::run` is the event loop**: render-on-change → `poll(250ms)` → dispatch. On `poll()` error, sleep the timeout to avoid spinning.

## Key design decisions worth knowing

- **`crossterm` uses the `use-dev-tty` feature.** The default mio-based event source in 0.27 fails on macOS with piped stdin (`Failed to initialize input reader`). The `use-dev-tty` alternative uses `poll(2)` + `signal-hook` pipes and works in both file and pipe modes.
- **Stdin path uses `dup2` to redirect fd 0 to `/dev/tty`** *only when stdin was actually drained from a pipe*. In file mode we leave fd 0 alone — replacing the shell's healthy tty fd breaks crossterm's event source init.
- **Byte-faithful rendering, not lossy decode.** Real `less` shows `\x1b` as `^[` and stray `0xFF` as `<FF>`. We do too. UTF-8 grapheme clusters are decoded via `unicode-segmentation`; widths via `unicode-width`.
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

## Build / packaging discipline

After every commit on this branch:

1. **Build both profiles** to catch profile-specific issues early:
   ```
   cargo build
   cargo build --release
   ```
2. **Generate a source tarball** of everything needed to compile `tess` on another machine, named `tess-<version>.tar.gz` (where `<version>` matches `Cargo.toml`), placed in the repo root next to this `CLAUDE.md`. Contents: `Cargo.toml`, `Cargo.lock`, `src/`, `tests/`, `MANUAL.md`, `CLAUDE.md`, `OUT-OF-SCOPE.md`, `INSTALL.md`, `.gitignore`. Excluded: `target/`, `.git/`, `.claude/`, any `.DS_Store`. The tarball is `.gitignore`d (see `tess-*.tar.gz`).

If a commit only touches docs and doesn't change the version, the tarball can be skipped — the previous one is still current. If the version bumped, regenerate.

## Where to put new work

- New design specs → `~/Development/Starweb/superpowers/tess/specs/YYYY-MM-DD-<topic>-design.md`
- Implementation plans → `~/Development/Starweb/superpowers/tess/plans/YYYY-MM-DD-<feature>.md`
- Session reports (after a brainstorm → plan → implement cycle) → `~/Development/Starweb/superpowers/tess/reports/YYYY-MM-DD-<feature>.md`

The MVP design lives in `specs/2026-04-27-rust-less-clone-design.md`; the implementation plan in `plans/2026-04-27-rust-less-clone-plan.md`; session report in `reports/2026-04-27-rust-less-clone.md`.

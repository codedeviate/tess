# Installing `tess`

`tess` is distributed as a source tarball. Building it requires a working Rust
toolchain; the resulting binary is a single static-ish executable you can drop
anywhere on `$PATH`.

Supported platforms: **macOS** and **Linux**. Windows is not currently
supported (the stdin / `/dev/tty` plumbing is Unix-specific).

---

## 1. Prerequisites

- **Rust 1.85 or newer** (transitive dep `clap_lex` requires edition 2024). Install via
  [rustup](https://rustup.rs):

  ```sh
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```

  Then make sure `~/.cargo/bin` is on your `$PATH` (rustup's installer
  prints the snippet to add to your shell rc).

- A C linker (already present on macOS via Xcode CLI tools; on Debian/Ubuntu
  install `build-essential`, on Fedora `gcc`).

Verify:

```sh
rustc --version    # rustc 1.85.0 or newer
cargo --version
```

---

## 2. Unpack

```sh
tar -xzf tess-<version>.tar.gz
cd tess-<version>           # or wherever the archive extracted to
```

The archive contains everything needed to build offline once dependencies are
fetched: `Cargo.toml`, `Cargo.lock`, `src/`, `tests/`, `MANUAL.md`, and this
file.

> **Note:** the archive does not have a top-level `tess-<version>/` directory —
> it extracts straight into the current working directory. Run `tar` from a
> fresh directory if you don't want files mixed with existing ones:
>
> ```sh
> mkdir tess-<version> && cd tess-<version>
> tar -xzf ../tess-<version>.tar.gz
> ```

---

## 3. Build

Release build (optimised, what you want for daily use):

```sh
cargo build --release
```

The binary lands at `target/release/tess`. First build downloads dependencies
and takes 30-60 seconds; subsequent builds are incremental.

Run the test suite to confirm everything is healthy:

```sh
cargo test
```

You should see all tests pass (~140 across unit, integration, and doc tests).

---

## 4. Install

Pick wherever your `$PATH` points. Common choices:

```sh
# Per-user, no sudo needed (recommended):
mkdir -p ~/.local/bin
install -m 755 target/release/tess ~/.local/bin/tess

# Or system-wide:
sudo install -m 755 target/release/tess /usr/local/bin/tess
```

> **Why `install` instead of `cp`?** On macOS 14+ (and especially 26 /
> Tahoe), the kernel may attach a `com.apple.provenance` extended attribute
> to binaries copied into protected locations like `/usr/local/bin`. If
> XProtect doesn't recognise the provenance, the kernel SIGKILLs the
> process on exec and you'll see `zsh: killed tess` with no further
> diagnostic. `install` avoids attaching the attribute.
>
> **If you already used `cp` and hit this**, both steps below are required
> on macOS 26 — stripping the xattr alone isn't enough because the kernel
> caches a "suspicious" verdict; re-signing produces a fresh `cdhash` that
> gets re-evaluated:
>
> ```sh
> sudo xattr -c /usr/local/bin/tess
> sudo codesign --force --sign - /usr/local/bin/tess
> tess --version    # should print 'tess <version>'
> ```

Make sure your chosen directory is on `$PATH`. For `~/.local/bin`, add this to
`~/.zshrc` or `~/.bashrc` if it's not already:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

Verify:

```sh
which tess
tess --version          # tess 0.4.0
```

---

## 5. First run

```sh
tess --help             # flag list (alphabetical by long name)
tess --examples         # curated usage recipes
tess --manual           # the full manual, paged through tess itself
tess Cargo.toml         # open a file
git log | tess          # page piped output
```

`tess --manual` and `tess --examples` open inside the pager when stdout is a
terminal, and print plain text when piped (`tess --manual | grep foo`,
`tess --manual > MANUAL.txt`).

---

## 6. Optional: user config

`tess` reads optional log formats and command-line groups from
`~/.config/tess/formats.toml`. See the **Log formats** and **Groups** sections
of `MANUAL.md` (or run `tess --manual`) for the schema. The file is optional —
the built-in formats (`apache-common`, `apache-combined`, `nginx-combined`)
work without any config.

---

## Updating

To upgrade, repeat the unpack → build → copy steps with the newer tarball.
`cargo build --release` reuses cached dependencies, so updates are quick.

---

## Uninstall

```sh
rm -f ~/.local/bin/tess          # or wherever you installed it
rm -rf ~/.config/tess            # optional: drop user config too
```

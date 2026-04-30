# `tess` — User Manual

A `less`-style terminal pager with structured-log support. macOS + Linux.

---

## Synopsis

```
tess [OPTIONS] [FILE]...
cmd | tess [OPTIONS]
```

`tess` opens a file or piped stdin and lets you scroll, page, search, follow, and (with a log format declared) filter by parsed fields.

---

## Quick start

| Goal | Command |
|---|---|
| View a file | `tess Cargo.toml` |
| View piped output | `git log \| tess` |
| Watch a log live | `tess -f /var/log/syslog` |
| Show line numbers | `tess -N script.sh` |
| Don't wrap long lines | `tess -S /etc/hosts` |
| Show last 1000 lines | `tess --tail 1000 huge.log` |
| Tail-follow last 1000 | `tess -f --tail 1000 huge.log` |
| Show only first 50 | `tess --head 50 file.txt` |
| Apache 5xx errors | `tess --format apache-combined --filter status~^5 access.log` |

---

## Command-line flags

### Display

- **`-N`, `--LINE-NUMBERS`** — show line numbers in a left-side gutter.
- **`-S`, `--chop-long-lines`** — truncate long lines at the right edge instead of wrapping. Toggle interactively with `Shift-S`.
- **`--tab-width N`** — tab-stop width (default `8`).

### Source

- **`-f`, `--follow`** — keep watching the source for new bytes (`tail -f`-style). Jumps to the bottom on startup. Toggle interactively with `Shift-F`. With piped stdin, runs a background reader thread.
- **`--tail N`** — show only the last `N` logical lines. For files, reverse-scans for the byte offset and only indexes from there forward, so a 10 GB log stays cheap. Mutually exclusive with `--head`. Streaming stdin (`-f` without a file) is not supported.
- **`--head N`** — cap the visible content to the first `N` logical lines. Mutually exclusive with `--tail`.

### Structured logs

- **`--format NAME`** — parse each line via a named log format (built-in or user-defined). Required by `--filter`.
- **`--filter FIELD<op>VALUE`** — keep only lines whose parsed `FIELD` matches the predicate. Repeatable; multiple filters AND together. Operators:
  - `=` — exact match. `--filter status=500`
  - `!=` — exact non-match. `--filter status!=200`
  - `~` — regex match. `--filter ip~^10\.`
  - `!~` — regex non-match. `--filter agent!~bot`

  > **Shell quoting** (important): in interactive `bash` and `zsh`, the `!` in `!=` and `!~` triggers history expansion and you'll see `bash: !=200: event not found` or similar. Quote the filter argument with **single quotes** to disable expansion:
  >
  > ```sh
  > tess --format apache-combined --filter 'status!=200' access.log
  > tess --format app --filter 'level!~notice' app.log
  > ```
  >
  > Single quotes are sufficient and prevent any other shell metacharacter (`\`, `$`, etc.) from being interpreted in your regex too. Inside scripts, history expansion is off by default; quoting is only needed at an interactive prompt. Alternatively `set +H` in bash disables history expansion for the session.
- **`--dim`** — render non-matching lines visibly faded instead of hiding them. Requires `--filter`.
- **`--list-formats`** — print available formats and their named fields, then exit.

### Other

- **`-h`, `--help`** — print a flag list (sorted alphabetically by long name) and exit.
- **`--manual`** — print this manual to stdout and exit. Pipe to a pager if you want to scroll: `tess --manual | less`.
- **`--examples`** — print a short, curated list of practical usage recipes and exit. Lighter than `--manual`.
- **`-V`, `--version`** — print version.

---

## Interactive keys

| Key(s) | Action |
|---|---|
| `↓` `j` `e` `Ctrl-E` `Return` | Scroll down 1 line |
| `↑` `k` `y` `Ctrl-Y` | Scroll up 1 line |
| `Space` `f` `Ctrl-F` `PgDn` | Page down |
| `b` `Ctrl-B` `PgUp` | Page up |
| `d` `Ctrl-D` | Half-page down |
| `u` `Ctrl-U` | Half-page up |
| `g` `<` `Home` | Go to top |
| `G` `>` `End` | Go to bottom |
| `/` *pattern* `Enter` | Forward regex search; `Esc` cancels the prompt |
| `?` *pattern* `Enter` | Backward regex search |
| `n` | Repeat last search (same direction) |
| `N` | Repeat last search (opposite direction) |
| `-N` (dash, then N) | Toggle line numbers |
| `-S` (dash, then S) | Toggle chop / wrap |
| `Shift-F` | Toggle follow mode |
| `r` `Ctrl-L` | Force redraw |
| `q` `Q` `Ctrl-C` | Quit |

In hide-mode filtering, scroll/page/goto operate on visible (matching) lines — the viewport skips past hidden ones.

### Search

Pressing `/` opens a search prompt at the bottom of the screen. Type a regex (the same flavor as `--filter` uses), then `Enter` to execute or `Esc` to cancel. `?` does the same backward. The matched logical line scrolls to the top of the viewport, and within every visible row the matched **phrase** itself is rendered in reverse-video (not the whole row). `n` repeats the last search in its original direction; `N` repeats it the other way. Search wraps at the end of the source.

When a filter is active, search interacts with it predictably: in hide mode, only currently-visible (matching) lines are searched. In dim mode, lines stay dimmed but the matched phrase within each line is still highlighted so it pops out of the surrounding context.

The status line picks up `[/<pattern>]` (or `[?<pattern>]`) while a search is set.

### Option-toggle prefix (`-`)

Borrowed from real `less`: pressing `-` enters a one-shot option-prefix mode. The next keystroke selects which option to flip:

| `-` then… | Effect |
|---|---|
| `N` | Toggle line numbers |
| `S` | Toggle chop / wrap |
| `F` | Toggle follow (also available as `Shift-F` directly) |

Lowercase variants work too (`-n`, `-s`, `-f`). Any other key after `-` cancels the prefix harmlessly.

---

## Status line

The bottom row shows current state. Format:

```
<source>  <top>-<bottom>/<total>  <pct>%  [<format>]  [filter]/[dim]  [/<search>]  (F)
```

- **`<source>`** — file path or `(stdin)`.
- **`<top>-<bottom>/<total>`** — currently visible line range over total. With `--filter` (hide mode) this is `top-bottom/<matched>/<total>`.
- **`<pct>%`** — position percentage.
- **`[<format>]`** — present when `--format` is active (e.g. `[apache-combined]`).
- **`[filter]` / `[dim]`** — present when filtering, indicating mode.
- **`[/<search>]`** / **`[?<search>]`** — active search pattern (forward or backward). Cleared only when a new search is set or you exit.
- **`(F)`** — present when follow mode is on. New bytes auto-scroll into view if you're at the bottom.
- **`+`** suffix on `total` — the source may still grow (streaming stdin or follow mode).

While a search prompt is open, the entire status row is replaced with `/<typed-so-far>` (or `?…`). `Enter` commits, `Esc` cancels, `Backspace` edits.

---

## Log formats

`tess` ships with three built-in formats and reads user-defined formats from `~/.config/tess/formats.toml`. User entries with the same name as a built-in win.

### Built-in formats

| Name | Fields |
|---|---|
| `apache-common` | `ip`, `user`, `time`, `method`, `url`, `protocol`, `status`, `size` |
| `apache-combined` | apache-common + `referer`, `agent` |
| `nginx-combined` | same as apache-combined |

Run `tess --list-formats` for the live list.

### Defining your own

`~/.config/tess/formats.toml`:

```toml
# A simple level/message format.
[format.simple]
regex = '^(?P<level>\w+) (?P<msg>.*)$'

# A custom application log: timestamp, level, request id, message.
[format.app]
regex = '^(?P<ts>\S+ \S+) (?P<level>\w+) \[(?P<reqid>[0-9a-f]+)\] (?P<msg>.*)$'

# Override a built-in (here, a simplified apache-common variant).
[format.apache-common]
regex = '^(?P<ip>\S+) - - \[(?P<time>[^\]]+)\] "(?P<request>[^"]+)" (?P<status>\d+) (?P<size>\S+)$'
```

Each format is one regex with named capture groups (`(?P<name>…)`). Field names become filterable. The regex must be anchored / specific enough that it matches only valid lines — non-matching lines are treated as "not parsed" and behave like filter mismatches.

### How filtering works

`tess` runs the format's regex once per line. The named captures form a field map. Each `--filter` predicate looks up its field in that map and applies its operator. **All** predicates must match (AND). Lines that don't match the format regex at all are treated as non-matching.

In **hide mode** (default): non-matching lines are skipped entirely. The line counts, scroll, and `goto_bottom` operate on the matched-line numbering.

In **dim mode** (`--dim`): non-matching lines are still rendered, but with `Attribute::Dim` so they're visually faded; surrounding context stays readable. Useful for inspecting matches in their surroundings.

---

## Examples

### Plain file viewing

```sh
# Open a file
tess README.md

# Show line numbers and disable line wrapping
tess -N -S src/main.rs

# Custom tab width for code with mixed indentation
tess --tab-width 4 Makefile
```

### Piped input

```sh
# Page through git log
git log | tess

# Page a colored command's output (tess passes ANSI through faithfully —
# control bytes render as ^X, so use a tool that strips them if you want
# them gone)
ls --color=always | tess

# Build output, kept on screen for inspection
cargo build 2>&1 | tess
```

### Quick first/last N

```sh
# Last 100 lines of a 5 GB log — opens instantly
tess --tail 100 /var/log/access.log

# First 50 lines of a generated file
tess --head 50 schema.sql

# Last 1000 lines and follow new ones (the headline log-watching command)
tess -f --tail 1000 /var/log/access.log
```

### Following live output

```sh
# Watch a log file
tess -f /var/log/syslog

# Watch a finite producer (returns to shell when the producer ends)
( for i in $(seq 1 200); do date; sleep 0.5; done ) | tess -f

# In tess, press Shift-F to pause auto-scroll, scroll back to read context,
# press G to jump back to the live tail, Shift-F again to re-engage.
```

### Apache log analysis

Sample log line (apache-combined):

```
127.0.0.1 - alice [10/Oct/2023:13:55:36 +0000] "GET /index.html HTTP/1.1" 200 2326 "-" "Mozilla/5.0"
```

```sh
# Only 5xx errors
tess --format apache-combined --filter status~^5 access.log

# Only 5xx errors on /api/* paths (multi-filter AND)
tess --format apache-combined --filter status~^5 --filter url~^/api/ access.log

# Everything except 200s (single-quote because of bash's history expansion on `!`)
tess --format apache-combined --filter 'status!=200' access.log

# Errors from a specific subnet
tess --format apache-combined \
  --filter status~^[45] \
  --filter ip~^10\.0\. \
  access.log

# Exclude bot traffic (single-quoted to escape the `!`)
tess --format apache-combined --filter 'agent!~bot' access.log

# Show errors with surrounding context (dim non-matches instead of hiding)
tess --format apache-combined --filter status~^5 --dim access.log

# Tail-follow only the errors as they happen
tess -f --tail 100 --format apache-combined --filter status~^5 access.log
```

### Custom format from your own app

```sh
# 1) Define it in ~/.config/tess/formats.toml:
mkdir -p ~/.config/tess
cat > ~/.config/tess/formats.toml <<'EOF'
[format.app]
regex = '^(?P<ts>\S+ \S+) (?P<level>\w+) \[(?P<reqid>[0-9a-f]+)\] (?P<msg>.*)$'
EOF

# 2) Verify it shows up
tess --list-formats

# 3) Use it
tess --format app --filter level=ERROR app.log

# 4) ERRORs and WARNs (regex-OR via alternation)
tess --format app --filter level~^(ERROR|WARN)$ app.log

# 5) Errors for a specific request id
tess --format app --filter level=ERROR --filter reqid=deadbeefcafe app.log

# 6) Watch ERRORs land in real time
tess -f --tail 200 --format app --filter level=ERROR app.log
```

### Filtering piped output

```sh
# Synchronous stdin (no -f): tess buffers everything, then filters
journalctl --no-pager | tess --format apache-combined --filter status~^5
# (won't actually match — journald isn't apache, but the shape is the same:
#  pipe in, parse, filter)

# Streaming stdin with -f: tess reads as a background thread
tail -F /var/log/access.log | tess -f --format apache-combined --filter status~^5

# Note: --tail isn't supported with streaming stdin (no random access).
# Use the file form when you want both --tail and -f together.
```

### Combining everything

```sh
# Tail-follow the last 5000 lines, only 5xx errors, with line numbers,
# don't wrap long URLs:
tess -f --tail 5000 -N -S \
  --format apache-combined \
  --filter status~^5 \
  /var/log/access.log

# Same with context preserved (dim mode):
tess -f --tail 5000 -N -S \
  --format apache-combined \
  --filter status~^5 \
  --dim \
  /var/log/access.log
```

### `tess --list-formats`

```
$ tess --list-formats
apache-combined: ip, user, time, method, url, protocol, status, size, referer, agent
apache-common: ip, user, time, method, url, protocol, status, size
nginx-combined: ip, user, time, method, url, protocol, status, size, referer, agent
```

(User-defined formats appear in the same list.)

---

## Groups: command-line shortcuts

Repeating long invocations gets tiresome. A `[group.NAME]` entry in `~/.config/tess/formats.toml` defines a shortcut: when you pass `--NAME` on the command line, `tess` expands it into a fixed set of flags (format, file, follow, tail, head, dim, line numbers, chop, tab width, default filters). Bare positionals after the group token become **filters**.

### Example

```toml
# ~/.config/tess/formats.toml

[format.errorlog]
regex = '^(?P<ts>\S+ \S+) (?P<level>\w+) \[(?P<reqid>[0-9a-f]+)\] (?P<msg>.*)$'

[group.errorlog]
format = "errorlog"
file = "/var/log/apache2/SE.error"
follow = true
tail = 1000
filter = ["level=ERROR"]   # optional: pre-applied filters

[group.access5xx]
format = "apache-combined"
file = "/var/log/apache2/access.log"
follow = true
filter = ["status~^5"]
```

With this config:

```sh
# Watch ERRORs in the app log:
tess --errorlog
# Equivalent to:
tess --format errorlog --follow --tail 1000 --filter 'level=ERROR' /var/log/apache2/SE.error

# Add an extra filter on the fly — positionals become --filter args:
tess --errorlog 'msg~timeout'
# Equivalent to the above plus --filter 'msg~timeout' (ANDed).

# Multiple ad-hoc filters:
tess --errorlog 'msg~timeout' 'reqid=deadbeefcafe'

# Override a group flag with a CLI flag (the CLI value wins):
tess --errorlog --tail 50 'msg~timeout'
# Group has tail=1000 but you override to 50.
```

### Group fields

All optional. Anything left out simply isn't passed.

| Key | Type | Maps to CLI flag |
|---|---|---|
| `format` | string | `--format <name>` |
| `file` | string | positional `FILE` |
| `follow` | bool | `-f` / `--follow` |
| `tail` | integer | `--tail N` |
| `head` | integer | `--head N` |
| `dim` | bool | `--dim` |
| `line_numbers` | bool | `-N` |
| `chop` | bool | `-S` |
| `tab_width` | integer | `--tab-width N` |
| `filter` | array of strings | `--filter X` (one entry per element) |

### Override semantics

When the group is expanded, its flags appear in argv before any flags you typed *after* the group token. For repeatable flags (`--filter`), CLI values **add** to the group's. For single-value flags (`--tail`, `--head`, `--tab-width`, `--format`), the **last occurrence wins**, so a CLI flag after the group token overrides the group's value.

### Restrictions

- A group cannot be named the same as a built-in flag (`format`, `filter`, `dim`, `head`, `tail`, `follow`, `LINE-NUMBERS`, `chop-long-lines`, `tab-width`, `list-formats`, `help`, `version`). Trying to load such a group prints an error and exits.
- Once a group token is seen, every subsequent bare positional in argv (anything that doesn't start with `-`) becomes a `--filter` argument. To open a different file alongside an active group, edit the group or define a second one — there is no `--file` override flag yet.

## Files

- **`~/.config/tess/formats.toml`** — user-defined log formats and groups. See [Defining your own](#defining-your-own) and [Groups](#groups-command-line-shortcuts).

---

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Clean exit |
| `1` | Startup error (bad arguments, file not found, not a regular file, no input on a TTY) |
| `2` | Runtime error (e.g. invalid filter spec, terminal init failure) |

---

## Common pitfalls

- **`bash: !=200: event not found`** (or `!~notice` etc.) — the `!` in negating filter operators triggers shell history expansion. Single-quote the filter: `--filter 'status!=200'`. See the note under `--filter` in [Command-line flags](#structured-logs).
- **`tess --mygroup somefile.log` doesn't open `somefile.log`** — when a group is active, bare positionals are treated as filters, so `somefile.log` becomes `--filter somefile.log` and fails to parse (no operator). The group's `file = "..."` is the file. To view a different file, drop the group flag.
- **`--filter` without `--format`** — errors out with `tess: --filter requires --format`. Pick a format first; use `--list-formats` if unsure.
- **Filter field doesn't exist in the format** — errors out before entering the pager with the available field list, e.g. `field 'foo' is not in format 'apache-combined' (available: ip, user, time, method, url, protocol, status, size, referer, agent)`.
- **Lines that don't parse against the chosen format** — treated as non-matches. Hidden by default; visible-but-dimmed with `--dim`. If many lines aren't parsing, your regex is probably too strict.
- **`--tail` on streaming stdin (`-f` with no file)** — prints `tess: --tail is not supported on streaming stdin (-f); ignoring` and continues without it.
- **Pipeline doesn't return to shell after `q`** — you ran something like `(while true; do …; done) | tess -f`. The producer subshell is in an infinite loop; bash waits for it. Use a finite producer or open the file directly.
- **Big regex on huge files in hide mode is slow at startup** — hide-mode filtering does one full index pass before the first frame to find matches. For non-filtered or dim-mode viewing, indexing is lazy and 10 GB files open instantly.

---

## Glossary

- **Logical line** — one newline-bounded record. The line numbering used by `--head`, `--tail`, `goto`, scroll, etc.
- **Display row** — one row on the terminal. A long logical line wraps into several display rows when wrap is on.
- **Source** — a `tess` byte source: a file (mmap-backed with a streaming companion handle), synchronous stdin, or streaming stdin.
- **Hide mode / dim mode** — what `--filter` does to non-matching lines. Hide is the default.

---

## Versions

This manual targets `tess 0.4.0`. Run `tess --version` to confirm.

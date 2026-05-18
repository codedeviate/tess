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
| Watch a file get rewritten (e.g. by an editor) | `tess --live src/main.rs` |
| Pretty-print a JSON / YAML / etc. file | `tess --prettify config.json` |
| Force a content type when the extension is unhelpful | `tess --content-type=json data.bin` |
| Show line numbers | `tess -N script.sh` |
| Don't wrap long lines | `tess -S /etc/hosts` |
| Show last 1000 lines | `tess --tail 1000 huge.log` |
| Tail-follow last 1000 | `tess -f --tail 1000 huge.log` |
| Show only first 50 | `tess --head 50 file.txt` |
| Apache 5xx errors | `tess --format apache-combined --filter status~^5 access.log` |
| Filter to a file (non-interactive) | `tess --format apache-combined --filter status~^5 -o errors.log access.log` |
| Pretty-print a file to stdout | `tess --prettify --stdout config.json` |
| Reformat each line through a template | `tess --format apache-combined --display '[<status>] <method> <url>' access.log` |

---

## Command-line flags

### Display

- **`-N`, `--LINE-NUMBERS`** — show line numbers in a left-side gutter.
- **`-S`, `--chop-long-lines`** — truncate long lines at the right edge instead of wrapping. Toggle interactively with `Shift-S`.
- **`--tab-width N`** — tab-stop width (default `8`).
- **`--hex`** — render the source as an `xxd`-style hex dump. Mutually exclusive with `--filter`, `--grep`, `--prettify`, `--format`, `--display`, `--record-start`, and `--prompt`.

### Source

- **`-f`, `--follow`** — keep watching the source for new bytes (`tail -f`-style). Jumps to the bottom on startup. Toggle interactively with `Shift-F`. With piped stdin, runs a background reader thread. **For appending writers** (log files); see `--live` for files rewritten in place.
- **`--live`** — watch a file for *whole-file* rewrites: when the file's `(mtime, size, inode)` changes, re-read the entire file, rebuild the line index, and re-render. Use for source files being edited or saved by an AI agent. Different from `--follow` (the two are mutually exclusive). Polling is at the same 250 ms cadence as the rest of the event loop, so saves land within ~¼ second. Press `R` inside the pager to force an immediate reload. Status line shows `(L)` while active. Caveats:
  - Best for source-file-sized inputs — the whole file is re-read on each change.
  - Atomic-rename writers (`vim`, `code`, most editors) work cleanly. Truncate-and-stream writers may briefly flicker if you catch them mid-write; saves typically settle on the next tick.
  - Scroll position is preserved (clamped to the new total). If you were at the very bottom, the viewport snaps to the new bottom.
  - Requires a file path; not supported on stdin.
- **`--tail N`** — show only the last `N` logical lines. For files, reverse-scans for the byte offset and only indexes from there forward, so a 10 GB log stays cheap. Mutually exclusive with `--head`. Streaming stdin (`-f` without a file) is not supported. Re-applied on every reload under `--live`.
- **`--head N`** — cap the visible content to the first `N` logical lines. Mutually exclusive with `--tail`. Re-applied on every reload under `--live`.

### Structured logs

- **`--format NAME`** — parse each line via a named log format (built-in or user-defined). Required by `--filter`.
- **`--filter FIELD<op>VALUE`** — keep only lines whose parsed `FIELD` matches the predicate. Repeatable; multiple filters AND together. Operators:
  - `=` — exact match. `--filter status=500`
  - `!=` — exact non-match. `--filter status!=200`
  - `~` — regex match. `--filter ip~^10\.`
  - `!~` — regex non-match. `--filter agent!~bot`
  - `<`, `<=`, `>`, `>=` — comparison. Numeric if both the captured value and the predicate value parse as numbers (`f64`), otherwise lexicographic byte order. `--filter 'status>=500'`, `--filter 'hour>=10' --filter 'hour<=12'`, `--filter 'level>warn'` (lex). When the captured value is non-numeric (e.g. CLF's `-` for "missing size"), comparison falls back to lex and effectively rejects.

  > **Shell quoting** (important): in interactive `bash` and `zsh`, the `!` in `!=` / `!~` triggers history expansion (`bash: !=200: event not found`), and the `<` / `>` in `<`, `<=`, `>`, `>=` are treated as input/output redirection. Quote the filter argument with **single quotes** to disable both:
  >
  > ```sh
  > tess --format apache-combined --filter 'status!=200' access.log
  > tess --format apache-combined --filter 'status>=500' access.log
  > tess --format app --filter 'level!~notice' app.log
  > ```
  >
  > Single quotes are sufficient and prevent any other shell metacharacter (`\`, `$`, etc.) from being interpreted in your regex too. Inside scripts, history expansion is off by default; quoting is only needed at an interactive prompt. Alternatively `set +H` in bash disables history expansion for the session.
- **`--grep PATTERN`** — filter visible lines by regex against the raw line.
  Repeatable; multiple `--grep` arguments AND. Works on any input — no
  `--format` required. Composes with `--filter` (both must match) and with
  `--dim` (non-matches stay visible but faded).

  ````sh
  tess --grep error access.log
  tess --grep error --grep '^\[' access.log         # both must match
  tess --grep error --dim access.log                # dim non-matches
  tess --format apache-combined --filter status=500 --grep timeout access.log
  ````
- **`--dim`** — render non-matching lines visibly faded instead of hiding them.
  Works with `--filter`, `--grep`, or both. Keeps surrounding context visible.
- **`--display TEMPLATE`** — reformat each parsed line into a custom view. Placeholders `<fieldname>` are replaced with the captured value (empty if the regex didn't capture the field on this line). `\<` is a literal `<`, `\\` is a literal `\`; other `\X` is left as-is. Lines that don't parse against the format regex fall back to their raw form so no data is silently dropped. Requires `--format`. Overrides the format's `display` key (if set in `formats.toml`). Affects both the interactive view and `--output` / `--stdout`. Search runs against the rendered template (so what you see is what you can find); filtering still operates on the raw captures. Mutually exclusive with `--prettify`.
- **`--list-formats`** — print available formats and their named fields, then exit.

### Pretty-printing

- **`--prettify`** — reformat the file's content for human reading. Supports **JSON, YAML, TOML, XML, HTML, CSV**. Type is detected from the filename extension (`.json`, `.yaml`/`.yml`, `.toml`, `.xml`, `.html`/`.htm`, `.csv`) and falls back to a quick byte sniff for unextended files. **Static files only** — not allowed with `--follow`, `--live`, or `--filter` (which would all conflict with reshaping the byte stream). Layout only — no syntax highlighting, so search and `--filter` (when used separately) keep working byte-cleanly.
- **`--content-type NAME`** — override detection. Values: `auto` (default — same as not passing this flag), `raw` (force prettify off, even if `--prettify` is also given), `json`, `yaml` (alias `yml`), `toml`, `xml`, `html` (alias `htm`), `csv`. Setting this implies `--prettify` unless the value is `auto` or `raw`.

If a transform fails to parse, `tess` falls back to showing the raw content and the status line shows `[pretty:<type>:err]` so you know why nothing changed.

CSV cells are aligned into a fixed-width table; cells longer than 60 characters are truncated with an ellipsis (`…`) so a single runaway free-text column doesn't blow up the layout.

### Batch (non-interactive) output

- **`-o FILE`, `--output FILE`** — apply `--filter` / `--head` / `--tail` / `--prettify` to the source, write the surviving logical lines as **raw bytes** (one per line, separated by `\n`) to `FILE`, and exit. Use `FILE = -` to write to stdout. The terminal alt-screen and raw mode are not entered, so this is safe to run from scripts and CI.
- **`--stdout`** — synonym for `-o -`.
- With **`--follow`**, the run doesn't exit after the initial pass: it keeps polling the source and appending matching new lines as they arrive (`Ctrl-C` cleanly closes the file). Useful for `tess -f --filter status~^5 -o errors.log` to harvest only error lines from a live log.
- Incompatible with **`--live`** (which is a "watch a file rewrite, render the new view" feature — there's no view in batch).
- **`--dim`**, **`-N`** (line numbers), and **`-S`** (chop) are viewport-only concerns and are silently ignored in batch mode. The output is always the raw bytes of matching lines, exactly as they appear in the source (or in the prettified stream when `--prettify` is on), so the file stays grep-/awk-/diff-able.

### Other

- **`-h`, `--help`** — print a flag list (sorted alphabetically by long name) and exit.
- **`--manual`** — print this manual to stdout and exit. Pipe to a pager if you want to scroll: `tess --manual | less`.
- **`--examples`** — print a short, curated list of practical usage recipes and exit. Lighter than `--manual`.
- **`--prompt TEMPLATE`** — override the built-in status line with a custom template. Placeholders `<field>` expand to live values (see [Customizing the status line](#customizing-the-status-line)). CLI `--prompt` overrides any `prompt` key in the active format. Not allowed with `--hex`.
- **`-V`, `--version`** — print version.

---

## Interactive keys

| Key(s) | Action |
|---|---|
| `↓` `j` `e` `Ctrl-E` `Return` | Scroll down 1 screen line (walks through wrap rows of long lines) |
| `↑` `k` `y` `Ctrl-Y` | Scroll up 1 screen line |
| `J` | Jump to start of next *logical* line, skipping any remaining wrap rows |
| `K` | Jump to start of current/previous *logical* line |
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
| `Shift-P` | Toggle pretty-print on/off (only when `--prettify` was active at startup) |
| `r` `Ctrl-L` | Force redraw |
| `Shift-R` | Force-reload from disk (with `--live`; no-op otherwise) |
| `q` `Q` `Ctrl-C` | Quit |

In hide-mode filtering, scroll/page/goto operate on visible (matching) lines — the viewport skips past hidden ones.

### Search

Pressing `/` opens a search prompt at the bottom of the screen. Type a regex (the same flavor as `--filter` uses), then `Enter` to execute or `Esc` to cancel. `?` does the same backward. The matched logical line scrolls to the top of the viewport, and within every visible row the matched **phrase** itself is rendered in reverse-video (not the whole row). `n` repeats the last search in its original direction; `N` repeats it the other way. Pressing `/` (or `?`) followed by `Enter` with an empty pattern repeats the last search in the typed direction (just like `n` / `N`). Search wraps at the end of the source.

When a filter is active, search interacts with it predictably: in hide mode, only currently-visible (matching) lines are searched. In dim mode, lines stay dimmed but the matched phrase within each line is still highlighted so it pops out of the surrounding context.

The status line picks up `[/<pattern>]` (or `[?<pattern>]`) while a search is set.

### Option-toggle prefix (`-`)

Borrowed from real `less`: pressing `-` enters a one-shot option-prefix mode. The next keystroke selects which option to flip:

| `-` then… | Effect |
|---|---|
| `N` | Toggle line numbers |
| `S` | Toggle chop / wrap |
| `F` | Toggle follow (also available as `Shift-F` directly) |
| `P` | Enter the pretty-print sub-prefix (see below) |

Lowercase variants work too (`-n`, `-s`, `-f`, `-p`). Any other key after `-` cancels the prefix harmlessly.

After `-P`, one more keystroke sets the content type:

| `-P` then… | Effect |
|---|---|
| `j` | Force JSON |
| `y` | Force YAML |
| `t` | Force TOML |
| `x` | Force XML |
| `h` | Force HTML |
| `c` | Force CSV |
| `a` | Auto-detect from current bytes |
| `r` | Raw (turn prettify off) |

Both `-P` letters are case-insensitive. Any other key cancels the sub-prefix.

---

## Status line

The bottom row shows current state. Format:

```
<source>  <top>-<bottom>/<total>  <pct>%  +<wrap>/<wraps>  [<format>]  [grep]  [filter]/[dim]  [/<search>]  [pretty:<type>]  (L)  (F)
```

- **`<source>`** — file path or `(stdin)`.
- **`<top>-<bottom>/<total>`** — currently visible line range over total. With `--filter` (hide mode) this is `top-bottom/<matched>/<total>`.
- **`<pct>%`** — position percentage.
- **`+<wrap>/<wraps>`** — only shown when scrolled inside a wrapped line. Tells you which wrap row of the current logical line is at the top of the viewport (e.g. `+12/50` means wrap row 12 of a 50-row line). Lets you see that `j` is making progress through a long line; goes away when you reach the next logical line.
- **`[<format>]`** — present when `--format` is active (e.g. `[apache-combined]`).
- **`[filter]` / `[dim]`** — present when filtering, indicating mode. With
  `--grep` active, an additional `[grep]` token appears.
- **`[/<search>]`** / **`[?<search>]`** — active search pattern (forward or backward). Cleared only when a new search is set or you exit.
- **`[pretty:<type>]`** — present when `--prettify` is active. `<type>` is one of `json`/`yaml`/`toml`/`xml`/`html`/`csv`. Suffix `:err` indicates the last transform failed to parse; raw content is shown.
- **`(L)`** — present when `--live` is on. The file is being watched for whole-file rewrites; `R` forces an immediate reload.
- **`(F)`** — present when follow mode is on. New bytes auto-scroll into view if you're at the bottom.
- **`+`** suffix on `total` — the source may still grow (streaming stdin, follow mode, or live mode).

While a search prompt is open, the entire status row is replaced with `/<typed-so-far>` (or `?…`). `Enter` commits, `Esc` cancels, `Backspace` edits.

---

## Customizing the status line

Override the built-in status format with a templated string. Same
`<field>` syntax as `--display`:

```sh
tess --prompt '<label> <pct>%' file.log
tess --prompt '<label>  <rec-block>  <pct>%<grep-tag><hide-tag>' --format app file.log
```

Set a per-format default in `~/.config/tess/formats.toml`:

```toml
[format.app]
regex = '^(?P<ts>\S+) (?P<level>\w+) (?P<msg>.+)$'
prompt = '<label>  <top>-<bottom>/<total>  <pct>%<filter-tag><hide-tag>'
```

CLI `--prompt` overrides `format.prompt`, which overrides the built-in
default. The built-in default reproduces tess's standard status format.

Available placeholders:

| Placeholder | Resolves to |
|---|---|
| `<label>` | source label (filename / stdin) |
| `<top>` / `<bottom>` / `<total>` | visible line range and total |
| `<pct>` | percent through file (0–100) |
| `<rec-top>` / `<rec-bottom>` / `<rec-total>` | record range and total (records mode only) |
| `<rec-block>` | `L<top>-<bot>/<total>  R<rec-top>-<rec-bot>/<rec-total>` in records mode; `<top>-<bot>/<total>` in line mode |
| `<wrap-offset>` | `+N/M` indicator when inside a long-wrapped line |
| `<format-tag>` | `[<format-name>]` when --format is active |
| `<filter-tag>` | `[<format-name>]` when --filter is active |
| `<grep-tag>` | `[grep]` when --grep is active |
| `<hide-tag>` | `[hide]` or `[dim]` when a predicate is active |
| `<search-tag>` | `[/pattern]` or `[?pattern]` while searching |
| `<pretty-tag>` | `[pretty:<type>]` when prettify is on |
| `<live-tag>` / `<follow-tag>` | `(L)` / `(F)` markers |

Tag placeholders that aren't active resolve to empty strings (with no
surrounding whitespace), so a template like `<label><filter-tag>` cleanly
collapses when there's no filter.

Escape `\<` for a literal `<` and `\\` for a literal `\`. Unknown
placeholders cause a startup error pointing at the offending field name.

---

## Customizing key bindings

Override tess's default keybindings via `~/.config/tess/keys.toml`:

```toml
[bindings]
"j"       = "scroll-down"         # bind a single character
"shift-j" = "scroll-logical-down" # case-equivalent: "J" also works
"f1"      = "toggle-line-numbers" # function keys
"ctrl-r"  = "reload"              # modifiers stack: ctrl-shift-l works too
"f2"      = "!git status"         # `!` prefix runs a shell command
```

Key spec grammar:

- Single character: `"j"`, `"/"`, `"%"`, `"!"`.
- Named special: `esc`, `enter`, `tab`, `backspace`, `space`,
  `f1`-`f12`, `up`/`down`/`left`/`right`, `pgup`/`pgdn`, `home`/`end`.
- Modifiers (stackable): `ctrl-`, `alt-`, `shift-`.
- Bare uppercase letter (`"J"`) is equivalent to `"shift-j"`. When a
  modifier is present (`"ctrl-J"`), the letter is taken literally —
  no implicit shift.

Action:

- Existing command name in kebab-case: `scroll-down`, `page-up`,
  `goto-line`, `toggle-line-numbers`, `mark-set`, `search-forward`,
  `shell-escape`, etc. Unknown command names error at startup.
- `!`-prefixed string: an inline shell command, run via the same
  infrastructure as `!cmd`.

**Forbidden keys** (cannot be rebound; error at startup):
`m`, `'`, `-`, `Ctrl-X`, and digits `0`-`9`. These participate in
multi-key sequences (marks, option prefix, jump-previous chord,
numeric prefix accumulator) and rebinding them would break those
features.

User bindings win over the built-in defaults. Any key not in the
config keeps its default binding.

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
# Optional default display template. CLI --display overrides.
display = '[<ts>] <level> <msg>'

# Override a built-in (here, a simplified apache-common variant).
[format.apache-common]
regex = '^(?P<ip>\S+) - - \[(?P<time>[^\]]+)\] "(?P<request>[^"]+)" (?P<status>\d+) (?P<size>\S+)$'
```

Each format is one regex with named capture groups (`(?P<name>…)`). Field names become filterable. The regex must be anchored / specific enough that it matches only valid lines — non-matching lines are treated as "not parsed" and behave like filter mismatches.

#### Nested capture groups

Capture groups can be nested. The outer group captures the whole substring, the inner groups capture sub-parts, and **every named group becomes its own filterable field**. This is the cleanest way to expose a composite value (a timestamp, a URL, a version string) as both the full string *and* its parts:

```toml
# Apache CLF timestamp like "06/May/2026:16:13:44 +0200" — exposed both as
# `time` (the full bracketed string) and as discrete `year` / `month` / `day` /
# `hour` / `minute` / `second` / `tz` fields.
[format.apache-with-time-parts]
regex = '''^(?P<ip>\S+) \S+ \S+ \[(?P<time>(?P<day>\d{2})/(?P<month>[A-Za-z]{3})/(?P<year>\d{4}):(?P<hour>\d{2}):(?P<minute>\d{2}):(?P<second>\d{2})\s+(?P<tz>[+\-]\d{4}))\] "(?P<method>\S+) (?P<url>\S+) (?P<protocol>[^"]+)" (?P<status>\d+) (?P<size>\S+)$'''
```

Then any of the parts can drive a filter:

```sh
tess --format apache-with-time-parts --filter year=2026 --filter 'hour~^1[0-2]$' access.log
tess --format apache-with-time-parts --filter month=May access.log
```

Caveats:

- **Group names share one namespace** — every named group across the whole regex must be unique, even when nested.
- **The outer group still captures the literal substring**, including any delimiters inside it. Don't put characters outside the outer group that you want included in `time`.
- **No post-processing** — the captured value is exactly what the regex matched. If a format has `May` you can't filter on `05`; either filter on `May` or use a regex predicate (`--filter 'month~^(May|05)$'`).
- For optional sub-parts (e.g. fractional seconds that may or may not be present), wrap the optional segment in a non-capturing group with `?`: `(?:\.(?P<micro>\d+))?`. When the segment isn't there, the `micro` field simply won't appear in the field map and any `--filter micro=…` won't match — exactly what you want.

### Display templates

A format can specify a default `display` template that reformats each parsed line into a chosen subset and order of fields. `--display TEMPLATE` on the command line overrides the format's default. Either way, the syntax is the same:

| Syntax | Meaning |
|---|---|
| `<fieldname>` | Replaced with the field's captured value. Empty string if the regex didn't capture the field on this line. |
| `\<` | Literal `<`. |
| `\\` | Literal `\`. |
| `\X` (any other) | Left as `\X` (so you don't have to escape backslashes inside regex-like literals). |
| anything else | Literal. |

Examples:

```sh
tess --format apache-combined --display '[<status>] <method> <url>' access.log
tess --format app --display '[<ts>] <level> <msg>' --filter 'level>=WARN' app.log
tess --format apache-combined --display '<status>: <url>' --filter 'status>=500' -o errors.log access.log
```

Behavior notes:

- **Search runs against the rendered template**, so the highlight you see is the substring you typed. If you removed `<msg>` from the template, `/Renderer.php` won't find anything — that's intentional. Drop the template if you want raw-line search.
- **Filtering still operates on the raw captures**, not the rendered output. `--filter status>=500` works regardless of whether `<status>` is in the template.
- **Lines that don't parse** against the format regex fall back to the raw line (no data is silently dropped), both in the interactive view and in `--output`.
- **Wrap-row scrolling** measures the rendered line length, so `j` walks the rendered wrap rows, not the raw line's.
- **Mutually exclusive with `--prettify`** (which already reshapes the byte stream).

### How filtering works

`tess` runs the format's regex once per line. The named captures form a field map. Each `--filter` predicate looks up its field in that map and applies its operator. **All** predicates must match (AND). Lines that don't match the format regex at all are treated as non-matching.

In **hide mode** (default): non-matching lines are skipped entirely. The line counts, scroll, and `goto_bottom` operate on the matched-line numbering.

In **dim mode** (`--dim`): non-matching lines are still rendered, but with `Attribute::Dim` so they're visually faded; surrounding context stays readable. Useful for inspecting matches in their surroundings.

---

## Marks

Save and restore positions in the file. Marks are session-local: they
live only as long as the running tess process.

| Keys | Action |
|------|--------|
| `m<x>` | Set mark `<x>` to the current top line. |
| `'<x>` | Jump to mark `<x>`. Silent no-op if the mark is unset. |
| `Ctrl-X Ctrl-X` | Jump to the previous position. Swaps current with previous, so pressing it twice returns to where you were. |

`<x>` is any lowercase letter `a`-`z` or digit `0`-`9` (36 slots total).

The previous-position slot is updated automatically on every big jump:
search (`/`, `?`, `n`, `N`), goto (`Ng`, `NG`, `N%`, bare `g`, bare `G`),
mark jumps, and `Ctrl-X Ctrl-X` itself. Scrolling (`j`, `k`, `Space`,
arrows) does NOT update it — so `Ctrl-X Ctrl-X` after scrolling around
takes you back to wherever you last jumped from.

If a mark refers to a line that no longer exists (for example, after
the source file has shrunk in `--live` mode), the jump lands at the
last available line.

---

## Multi-file navigation

Pass multiple files on the command line:

```sh
tess foo.log bar.log baz.log
```

The first file opens immediately. Use `:`-prefixed commands to navigate:

| Command | Action |
|---------|--------|
| `:n` (or `:next`) | Next file. Shows `[no next file]` at the end. |
| `:p` (or `:prev`) | Previous file. Shows `[no previous file]` at the start. |
| `:e <path>` (or `:edit`) | Open `<path>`, append it to the file list, switch to it. `~/` expands to `$HOME`. |
| `:f` | Show the current filename and position briefly in the status line. |
| `:q` (or `:quit`) | Quit (alias for `q`). |
| `:d` (or `:delete`) | Remove the current file from the list and switch to the next (or previous if at the end). Errors if only one file remains. |
| `:x` (or `:first`) | Jump to the first file in the list. |
| `:t` (or `:last`) | Jump to the last file in the list. |

Press `:` to enter the colon prompt; type the command and press Enter,
or press Esc to cancel. Bare Enter on an empty prompt dismisses without
running anything.

When the file list has more than one entry, the status line shows
`<label>  [N/M]` next to the filename. Custom `--prompt` templates can
place this anywhere via the `<file-index-tag>` placeholder.

### State preserved across file switches

- Marks (`m a`, `'a`) are session-wide. Setting `m a` in `foo.log` and
  switching to `bar.log` doesn't lose the mark — `'a` returns to that
  position in `foo.log`.
- The previous-position slot (Ctrl-X Ctrl-X) tracks across files too.
  Jump from line 50 in `foo.log` to line 1 in `bar.log` (via `:n`),
  press Ctrl-X Ctrl-X, return to `foo.log:50`.
- The active search regex persists. After `:n`, pressing `n` searches
  the new file from the top with the same pattern.

### State reset on every switch

- Top-of-screen position resets to line 1. Each file starts fresh.
- The numeric-prefix accumulator, mark-pending, and Ctrl-X-pending
  states all clear.

### Stdin and multi-file

Piped stdin (`cat foo | tess bar.log baz.log`) still wins: stdin is
read as the only source, and file arguments are ignored with a warning.
Multi-file navigation requires file-path arguments.

### `--follow` and `--live` with multi-file

If you invoked with `--follow`, each file you switch to enters follow
mode. If you invoked with `--live`, each file is opened as a live
source.

---

## Running shell commands

Press `!` inside the pager to enter a shell command. The prompt shows
the command as you type it; `Enter` runs it, `Esc` cancels.

When the command runs, tess drops the alt-screen and raw mode so the
command sees a normal interactive terminal. Output streams directly to
your terminal. After the command exits, tess prompts:

```
[Press any key to continue]
```

After you press any key, tess restores the alt-screen and redraws.
Interactive commands like `!vim notes.txt` work normally.

The shell is taken from `$SHELL`, falling back to `/bin/sh`.

---

## Hex display

Render the source as an `xxd`-style hex dump. One row covers 16 bytes:
an 8-digit hex offset, the bytes themselves grouped in 8 × 2-byte words,
and an ASCII gutter where printable bytes appear and everything else
shows as `.`.

```sh
tess --hex /usr/bin/ls
tess -f --hex /var/log/binary-feed.bin   # follow mode works
```

`--hex` is mutually exclusive with `--filter`, `--grep`, `--prettify`,
`--format`, `--display`, `--record-start`, and `--prompt` — hex mode is
fundamentally byte-level and these features are line- or record-oriented.

Search (`/pattern`) inside hex mode operates on the rendered row text,
so you can find ASCII strings in the gutter or hex byte sequences:

```
/Hello       # find the ASCII string in the gutter
/4865 6c6c   # find the same bytes by their hex
```

---

## Preprocessing input

Pipe the source file through an external command before tess reads it.
Useful for viewing PDFs as text (`pdftotext`), tar archives as listings
(`tar -tzvf`), and so on.

Two ways to set the preprocessor:

```sh
# Per-invocation (CLI flag):
tess --preprocess '|pdftotext %s -' document.pdf

# Persistent (env var, less-compatible):
export LESSOPEN='|lesspipe.sh %s'
tess any-file.gz
```

CLI flag overrides env var. `--no-preprocess` ignores both for one
invocation.

Syntax: the command must start with `|` (pipe mode). `%s` is substituted
with the file path (shell-quoted to handle spaces). The command's stdout
becomes tess's input.

On failure (non-zero exit, missing executable, empty output), tess falls
back to the raw file and shows `[preprocess-failed: <stderr>]` in the
status line. Custom `--prompt` templates can include the failure tag
via `<preprocess-failed-tag>`.

Mutually exclusive with `--hex`, `--follow`, and `--live`.

Stdin pipes (`cat foo.txt | tess`) skip preprocessing entirely.

---

## Multi-line records

Some log formats emit records that span many physical newlines: PHP error
logs with stack traces, Java exception traces, multi-line debug payloads.
Set `record_start` to a regex that matches the first line of each record,
and tess treats every following non-matching line as a continuation:

```toml
[format.php-app]
record_start = '^\['
regex        = '^\[(?P<ts>[^\]]+)\] +(?P<level>\w+) +(?P<msg>[\s\S]+)$'
display      = '[<ts>] <level> <msg>'
```

Then run:

```
tess --format php-app /var/log/php/error.log
```

Or use the CLI flag directly without a format:

```
tess --record-start '^\[' /var/log/php/error.log
```

When records mode is active:

- Search (`/pattern`, `n`, `N`) matches against the full record bytes
  with embedded newlines. Use `(?s)` if you want `.` to match `\n`.
- `--filter FIELD<op>VALUE` matches against the parsed record (so
  `[\s\S]+` capture groups span lines).
- `--grep PATTERN` matches against the full record bytes.
- Hide mode hides every line of a non-matching record; dim mode dims
  them but keeps them visible.
- Status line shows `L<line>-<line>/<total>  R<rec>-<rec>/<total>`.
- `Ng` jumps to physical line N; `NG` jumps to record N; `N%` jumps to
  N percent through the file by bytes. `Esc` cancels a partially-typed
  numeric prefix.

Lines before the first `record_start` match are collected into a single
synthetic record numbered 0. If no line in the file matches the regex,
the entire content becomes one big record (this is usually a sign that
the regex is wrong).

`--head N` and `--tail N` continue to count physical lines, not records.

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
| `grep`   | array of strings | `--grep X` (one entry per element) |

### Override semantics

When the group is expanded, its flags appear in argv before any flags you typed *after* the group token. For repeatable flags (`--filter`, `--grep`), CLI values **add** to the group's. For single-value flags (`--tail`, `--head`, `--tab-width`, `--format`), the **last occurrence wins**, so a CLI flag after the group token overrides the group's value.

Groups also support a `grep` field that mirrors `filter`. Each entry
becomes a repeated `--grep <pattern>` after group expansion, and the
user's own `--grep` arguments accumulate on top:

```toml
[group.errorlog]
format = "errorlog"
follow = true
filter = ["level=ERROR"]
grep = ["timeout", "deadlock"]   # both patterns must match
```

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
- **Hide mode / dim mode** — what `--filter` / `--grep` does to non-matching lines. Hide is the default.

---

## Versions

This manual targets `tess 0.6.2`. Run `tess --version` to confirm.

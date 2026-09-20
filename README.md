![license](https://img.shields.io/badge/license-MIT-blue)

# rewind

A terminal browser for your Claude Code conversation history.

Claude Code keeps every conversation on disk, but the only way back into one is to remember
which repository you were in, `cd` there, and `--resume` your way through a picker. And if
the working copy is gone, so is the path back — even though the transcript is still there.

`rewind` opens all of it: every project Claude Code has ever seen, the sessions inside each,
and the conversation rendered as something worth reading. It makes no network requests and
never writes to `~/.claude`.

```
 rewind · vademecum · Theme loader fallback bug · code-review      ?  help
────────────────────────────────────────────────────────────────────────────
 Projects            Sessions                  Conversation
 ▸ rewind      2d  4 │   Rewind TUI: PRD…  2d  4 │ ▎ you
   vademecum   5d 36 │   fallout-vault…    5d 18 │ ▎ the fallback picks the wrong
   tr-ios      1w 95 │   Theme loader…     1w 61 │ ▎ theme when the file is partial
   .dotfiles   2w 27 │   minimap flicker…  3w  9 │
 ⊘ old-spike   6mo 3 │                          │ ▎ claude · opus-5
                     │                          │ ▎ Right — `merge` is folding the
                     │                          │ ▎ base in the wrong direction.
                     │                          │ ▎
                     │                          │ ▎ ▸ Read
                     │                          │ ▎   └ src/theme/loader.rs · 903 lines
                     │                          │ ▎ ▾ Edit
                     │                          │ ▎   └ src/theme/loader.rs · 1 hunk
                     │                          │ ▎   ┃ @@ -18,3 +18,3 @@
                     │                          │ ▎   ┃ -    base.or(user)
                     │                          │ ▎   ┃ +    user.or(base)
                     │                          │ ▎ ▸ Agent                                   ⏎
                     │                          │ ▎   └ code-review · check the fix
                     │                          │ ▎ ── 2 alternate branches · [b] ──
────────────────────────────────────────────────────────────────────────────
 vademecum · 61 msgs · main · 148.2k in / 22.9k out · 12/61 · 19%
```

The column with the keyboard names itself twice: its heading takes the accent colour, and
its selected row takes a solid band while the other columns keep a faint one. `Tab` moves
both.

The status line names the selected session, then the focused column's position and
percentage through its rows (nothing worth reading if it only has one), then `focus` when
`f` is on.

Below 50 columns there is no room for three side by side, so rewind shows **one pane at a
time** — whichever is focused, full width. `Enter` descends into it, `Esc` goes back, and a
pane with something behind it carries a `‹` before its name.

*Under construction — see the milestones for what works today.*

## Install

```
cargo install --locked --path .
```

or from source:

```
git clone https://github.com/otaviocc/rewind
cd rewind
cargo build --release
./target/release/rewind
```

Homebrew and prebuilt binaries for Linux, macOS and Windows arrive with the first tagged
release.

## Quick start

```
rewind                    open on every project Claude Code has seen
rewind vademecum           open one project directly
rewind --session <id>      open one conversation directly
```

`Tab` walks Projects → Sessions → Conversation, `Enter` descends into the selected row, and
`q` leaves. A project whose working copy is gone still reads — it shows as `⊘` in the
Projects column, because the transcript survives even when the directory does not. On a
narrow terminal `Enter` and `Esc` do the same walking, one pane at a time.

## Keys

| Key | |
| --- | --- |
| `h` `l` `Tab` `Shift-Tab` `←` `→` | move between columns |
| `j` `k` `↓` `↑` | move within a column |
| `g` `G` `Home` `End` | top · bottom of the column |
| `d` `u` `Ctrl-d` `Ctrl-u` | half page down · up |
| `Enter` | descend · expand the selected tool call · enter a subagent |
| `Esc` | leave a subagent · back out |
| `f` | focus the conversation full-width |
| `/` | filter the list |
| `s` | search everything |
| `n` `p` `N` | next · previous tool call, or step through a locked `/` filter (`N` is an alias for `p`) |
| `[` `]` | previous · next message |
| `Space` `t` | expand one tool call · all of them |
| `i` | reveal context injections |
| `b` | cycle the alternate branches at the marker |
| `y` `Y` | copy the message · the whole session |
| `c` | copy `claude --resume <id>` |
| `e` | export to a file |
| `D` | what could not be read · `Esc` or `q` closes it |
| `?` `F1` | every key and what it does · `Esc` or `q` closes it |
| `r` | rescan `~/.claude` for new and changed projects |
| `q` `Ctrl-C` | quit — with a window open, `q` closes the window and `Ctrl-C` still quits |

The `/` filter and the `e` export prompt take typing: letters type instead of acting,
`Backspace` deletes, `↑`/`↓` still move, `Enter` accepts, and `Esc` cancels. In the export
prompt, `Tab` cycles the format between Markdown and JSONL.

Mouse wheel scrolls whichever column is under the pointer, not whichever has focus. Clicking
a row selects it, clicking a collapsed tool call expands it, and clicking a subagent call
enters it. Dragging in the conversation selects its lines and copies them to the clipboard
on release. `--no-mouse` turns capture off.

## Searching

`s` opens a search over every project, ranking hits as you type; `Enter` opens the
selected one, switching to the branch the hit is on, stopping on the line that carries the
term and marking it there. The mark stays until your next keypress, so a hit in a long
message is a glance rather than a hunt.

| | |
| --- | --- |
| `bare terms` | AND together |
| `"a phrase"` | matched whole |
| `-term` | excludes it |
| `is:user` `is:assistant` `is:tool` `is:thinking` `is:any` | restrict to one kind of text |
| `project:name` | restrict to one project |

Anything else is a literal term.

A search with no `is:` looks only at what a human or the assistant wrote. Tool parameters,
tool output and the model's thinking are indexed too, but they are bulky and would otherwise
crowd out the conversation, so you reach them by asking: `is:tool`, `is:thinking`, or
`is:any` for everything at once.

Search is backed by a corpus rewind keeps under `$XDG_CACHE_HOME/rewind`, or
`~/.cache/rewind` if that variable is unset. It builds in the background and is searchable
while it is still filling — the overlay shows `indexing…`, then marks results `(partial)`
until the corpus is complete. The project and session lists never wait on it.
`--no-cache` ignores the cache and does not write it; `--rebuild-cache` discards it and
rebuilds from scratch, then exits.

## Themes

Fourteen themes are built in. `spool` is the default — written for `rewind` rather than
borrowed from an editor, and it leaves the background and foreground alone so a transcript
sits on your own terminal ground. `ansi` is the other end: it asserts no colour at all and
takes everything from your terminal's scheme.

```
spool             ansi              catppuccin-latte
catppuccin-mocha  default-plus      gruvbox-dark
gruvbox-light     kanagawa-dragon   nord
solarized-dark    solarized-light   tokyo-night
tokyo-night-day   vesper
```

`--theme <name>` picks one, `--list-themes` prints them.

To change anything, `rewind` reads one file. On every platform, including macOS, it lives at
`$XDG_CONFIG_HOME/rewind/theme.toml`, or `~/.config/rewind/theme.toml` if that variable is
unset — on Windows, `%APPDATA%\rewind\theme.toml` if neither is set. Named themes go beside
it in `themes/<name>.toml` and are selected with `--theme`.

Everything is optional. A theme states what it wants moved and inherits the rest, so this is
a complete and valid file:

```toml
[palette]
accent = "#89b4fa"
```

A theme of your own in `themes/` shadows a built-in of the same name — including `spool`,
which replaces the default without a flag, and `ansi`, which every other theme inherits from.

### Colors

A color is one of the sixteen ANSI names (`red`, `light_blue`, `dark_gray`, …), `#rrggbb`, a
bare number for a 256-color index, or `reset` for whatever the terminal already uses.

```toml
[palette]
accent     = "#89b4fa"   # true color
notice     = 208         # a 256-color index
background = "reset"     # the terminal's own
```

The fifteen slots, and what each drives when nothing more specific overrides it:

| Slot | Drives |
| --- | --- |
| `background` | the help window's ground |
| `foreground` | body text |
| `muted` | thinking blocks, context injections, the branch marker, the compact-boundary divider, tool summaries and their ok mark, a missing project, a code fence's language tag, quote gutters, list bullets, table borders, rules, raw HTML, hints, an unfocused column's title |
| `muted_text` | block quotes and the status line |
| `subtle` | nothing on its own — `cursor` falls back to it when a theme sets neither |
| `cursor` | the cursor-line band |
| `selection_background` `selection_foreground` | a dragged mouse selection |
| `error` | tool errors, removed diff lines, status errors |
| `success` | added diff lines, the live-session badge |
| `warning` | status notices, the marked terms in a message a search opened |
| `accent` | tool names, the human gutter, a focused column's title, the header title, inline code, the scroll percentage |
| `chrome` | nothing by default — free for a theme to point an element at |
| `highlight` | links |
| `notice` | the assistant gutter |

A slot left unsaid keeps its default, and `cursor` follows `subtle` unless you say
otherwise.

### Elements

Elements are the surfaces those colors are used on. Each takes `fg`, `bg` and `modifiers`,
and `fg` and `bg` may name a palette slot instead of a color:

```toml
[elements.heading]
fg        = "accent"
modifiers = ["bold", "underline"]

[elements.cursor_line]
bg = "none"
```

`modifiers` replaces rather than adds, so `modifiers = []` removes the bold an element
started with. The six are `bold`, `italic`, `underline`, `dim`, `reversed` and `crossed_out`.
`bg = "none"` removes a background; a foreground cannot be removed, only changed.

The element keys are `body`, `muted`, `label`, `human_gutter`, `assistant_gutter`,
`tool_name`, `tool_summary`, `tool_ok`, `tool_error`, `thinking`, `diff_added`,
`diff_removed`, `diff_context`, `subagent`, `injection`, `branch_marker`, `compact_divider`,
`project_missing`, `session_live`, `heading`, `strong`, `emphasis`, `strikethrough`,
`inline_code`, `code_block`, `code_block_lang`, `link`, `quote`, `quote_gutter`,
`list_bullet`, `table_header`, `table_border`, `rule`, `html`, `header_title`, `status`,
`status_notice`, `status_error`, `cursor_line`, `search_match`, `search_current`, `selection`,
`help_window`, `scroll_progress`, `hint`, `column_title` and `column_title_active`.

### Inheriting

`base` takes the name of another theme, which is merged underneath field by field:

```toml
base = "nord"

[palette]
accent = "#89b4fa"
```

A theme may also name the syntax highlighting its code blocks use, with
`syntax_theme = "..."`. The bundled ones are `default-plus`, `base16-ocean.dark`,
`base16-eighties.dark`, `base16-mocha.dark`, `base16-ocean.light`, `InspiredGitHub`,
`Solarized (dark)` and `Solarized (light)`.

### A whole theme

Everything above, together — `base`, a palette, and a few elements:

```toml
base          = "nord"
syntax_theme  = "base16-mocha.dark"

[palette]
accent = "#89b4fa"
notice = "#d9a05b"

[elements.heading]
fg        = "accent"
modifiers = ["bold", "underline"]

[elements.tool_error]
fg        = "error"
modifiers = ["bold"]

[elements.cursor_line]
bg = "none"
```

### When you get it wrong

A key you misspell — a palette slot, an element name, or a top-level field like `base` — is
reported on stderr and otherwise ignored, so one typo costs you that one line and nothing
else. A color or modifier that is not one fails the run outright, naming the key that
carried it. A `base` chain that loops on itself is also an error.

## Flags

| Flag | |
| --- | --- |
| `[PROJECT]` | open this project first |
| `--session <ID>` | open a session directly |
| `--theme <NAME>` | theme by name |
| `--config <FILE>` | explicit theme file |
| `--list-themes` | list built-in and user themes, then exit |
| `--claude-dir <DIR>` | override `~/.claude` |
| `--no-mouse` | disable mouse capture |
| `--no-cache` | ignore and do not write the cache |
| `--rebuild-cache` | discard the cache and rebuild, then exit |
| `--color <WHEN>` | `auto`, `always` or `never` |
| `--help` `--version` | usage and version, then exit |

## Privacy

`rewind` makes no network requests, ever, and never writes to `~/.claude`, ever. The only
things it writes anywhere are its own search cache under `~/.cache/rewind` and whatever `e`
exports to the file you name.

## License

MIT.

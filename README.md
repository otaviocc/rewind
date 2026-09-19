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
                     │                          │ ▎ ▸ Read  src/theme/loader.rs · 903 lines   ok
                     │                          │ ▎ ▾ Edit  src/theme/loader.rs · 1 hunk       ok
                     │                          │ ▎   ┃ @@ -18,3 +18,3 @@
                     │                          │ ▎   ┃ -    base.or(user)
                     │                          │ ▎   ┃ +    user.or(base)
                     │                          │ ▎ ▸ Agent  code-review · check the fix  ⏎   ok
                     │                          │ ▎ ── 2 alternate branches · [b] ──
────────────────────────────────────────────────────────────────────────────
 vademecum · 61 msgs · main · 148.2k in / 22.9k out · 12/61 · 19%
```

The column with the keyboard names itself twice: its heading takes the accent colour, and
its selected row takes a solid band while the other columns keep a faint one. `Tab` moves
both.

*Under construction — see the milestones for what works today.*

## Install

```
cargo install --locked --path .
```

## Keys

| Key | |
| --- | --- |
| `h` `l` `Tab` | move between columns |
| `j` `k` | move within a column |
| `g` `G` | top · bottom of the column |
| `Ctrl-d` `Ctrl-u` | half page down · up |
| `Enter` | descend · expand the selected tool call · enter a subagent |
| `Esc` | leave a subagent · back out |
| `f` | focus the conversation full-width |
| `/` | filter the list |
| `?` | search everything |
| `n` `N` | next · previous tool call |
| `Space` `t` | expand one tool call · all of them |
| `i` | reveal context injections |
| `b` | cycle the alternate branches at the marker |
| `y` `Y` | copy the message · the whole session |
| `c` | copy `claude --resume <id>` |
| `e` | export to a file |
| `d` | what could not be read · `Esc` closes it |
| `q` | quit |

Mouse wheel scrolls whichever column is under the pointer, not whichever has focus. Clicking
a row selects it, clicking a collapsed tool call expands it, and clicking a subagent call
enters it. `--no-mouse` turns capture off.

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
unset. Named themes go beside it in `themes/<name>.toml` and are selected with `--theme`.

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

The fifteen slots are `background`, `foreground`, `muted`, `muted_text`, `subtle`, `cursor`,
`selection_background`, `selection_foreground`, `error`, `success`, `warning`, `accent`,
`chrome`, `highlight` and `notice`. A slot left unsaid keeps its default, and `cursor`
follows `subtle` unless you say otherwise.

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
`syntax_theme = "..."`. The bundled ones are `base16-ocean.dark`, `base16-eighties.dark`,
`base16-mocha.dark`, `base16-ocean.light`, `InspiredGitHub`, `Solarized (dark)` and
`Solarized (light)`.

A key you misspell is reported on stderr and otherwise ignored, so one typo does not cost you
the rest of the file. A color or modifier that is not one fails the run and names the key
that carried it.

## Flags

| Flag | |
| --- | --- |
| `--session <ID>` | open a session directly |
| `--theme <NAME>` | theme by name |
| `--config <FILE>` | explicit theme file |
| `--list-themes` | list themes and exit |
| `--claude-dir <DIR>` | override `~/.claude` |
| `--no-mouse` | disable mouse capture |
| `--no-cache` | ignore and do not write the cache |
| `--rebuild-cache` | discard the cache and rebuild, then exit |
| `--color <WHEN>` | `auto`, `always` or `never` |

## License

MIT.

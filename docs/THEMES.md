# Theming rewind

`rewind` ships fourteen themes and reads one file if you want something else. This page is
the whole of it: the file, the colors, the surfaces they land on, and what happens when you
get it wrong. [`README.md`](../README.md#themes) has the short version: the names, the
flags, and where the file lives.

## The file

On every platform, including macOS:

```
$XDG_CONFIG_HOME/rewind/theme.toml
~/.config/rewind/theme.toml          if XDG_CONFIG_HOME is unset
```

Named themes go beside it in `themes/<name>.toml` and are selected with `--theme <name>`.
`--config <file>` reads one file explicitly, wherever it is.

Everything is optional. A theme states what it wants moved and inherits the rest, so this is
a complete and valid file:

```toml
[palette]
accent = "#89b4fa"
```

A theme of your own in `themes/` shadows a built-in of the same name, including `spool`,
which replaces the default without a flag, and `ansi`, which every other theme inherits from.

## Colors

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
| `subtle` | nothing on its own; `cursor` falls back to it when a theme sets neither |
| `cursor` | the cursor-line band |
| `selection_background` `selection_foreground` | a dragged mouse selection |
| `error` | tool errors, removed diff lines, status errors |
| `success` | added diff lines, the live-session badge |
| `warning` | status notices, the marked terms in a message a search opened |
| `accent` | tool names, the human gutter, a focused column's title, the header title, inline code, the scroll percentage |
| `chrome` | nothing by default; free for a theme to point an element at |
| `highlight` | links |
| `notice` | the assistant gutter |

A slot left unsaid keeps its default, and `cursor` follows `subtle` unless you say
otherwise.

## Elements

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
`status_notice`, `status_error`, `cursor_line`, `search_match`, `selection`,
`help_window`, `scroll_progress`, `hint`, `column_title` and `column_title_active`.

## Inheriting

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

## A whole theme

Everything above, together: `base`, a palette, and a few elements:

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

## When you get it wrong

A key you misspell, whether a palette slot, an element name, or a top-level field like
`base`, is reported on stderr and otherwise ignored, so one typo costs you that one line and
nothing else. A color or modifier that is not one fails the run outright, naming the key that
carried it. A `base` chain that loops on itself is also an error.

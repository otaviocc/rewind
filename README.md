# rewind

A terminal browser for your Claude Code conversation history.

Claude Code keeps every conversation on disk, but the only way back into one is to remember
which repository you were in, `cd` there, and `--resume` your way through a picker. And if
the working copy is gone, so is the path back — even though the transcript is still there.

`rewind` opens all of it: every project Claude Code has ever seen, the sessions inside each,
and the conversation rendered as something worth reading. It makes no network requests and
never writes to `~/.claude`.

```
 rewind · vademecum · Theme loader fallback bug                    ?  help
────────────────────────────────────────────────────────────────────────────
 Projects            Sessions                  Conversation
 ▸ rewind      2d  4 │ ▸ Rewind TUI: PRD…  2d  4 │ ▎you
   vademecum   5d 36 │   fallout-vault…    5d 18 │ the fallback picks the wrong
   tr-ios      1w 95 │   Theme loader…     1w 61 │ theme when the file is partial
   .dotfiles   2w 27 │   minimap flicker…  3w  9 │
 ⊘ old-spike   6mo 3 │                          │ ▎claude  opus-5
                     │                          │ Right — `merge` is folding the
                     │                          │ base in the wrong direction.
                     │                          │
                     │                          │ ▸ Read  src/theme/loader.rs  903 lines
                     │                          │ ▾ Edit  src/theme/loader.rs
                     │                          │     - base.or(user)
                     │                          │     + user.or(base)
────────────────────────────────────────────────────────────────────────────
 vademecum · 61 msgs · main · 148.2k in / 22.9k out · 12/61 · 19%
```

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
| `Enter` | descend · expand a tool call · enter a subagent |
| `Esc` | back out |
| `f` | focus the conversation full-width |
| `/` | filter the list |
| `?` | search everything |
| `Space` `t` | expand one tool call · all of them |
| `i` | reveal context injections |
| `b` | cycle alternate branches |
| `y` `Y` | copy the message · the whole session |
| `c` | copy `claude --resume <id>` |
| `e` | export to a file |
| `d` | diagnostics |
| `q` | quit |

Mouse wheel scrolls whichever column is under the pointer. `--no-mouse` turns capture off.

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

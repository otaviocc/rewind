# rewind

A read-only terminal browser for Claude Code conversation history: the projects Claude Code
has seen, the sessions inside them, and the conversation itself rendered as something worth
reading. No network requests, ever. No writes to `~/.claude`, ever.

## Commands

```
make check                                   THE GATE — fmt, clippy, test, audit
make build                                   cargo build --release
make test                                    cargo test
make fmt                                     cargo fmt
make lint                                    cargo clippy --all-targets -- -D warnings
make audit                                   cargo audit --deny warnings
cargo run -- --claude-dir tests/data/claude  run against the fixtures
```

**CI is disabled and verification is local.** The repository is private until there is
something people can use, and GitHub Actions does not run on it. `.github/workflows/ci.yml`
is committed and known-good but disabled at the repository level; re-enabling it is part of
#26, alongside going public.

So `make check` is the gate, not a convenience: **run it before every commit that closes an
issue, and say in the closing comment that it passed.** Nothing else will catch a regression.

It reproduces four of the five CI jobs. The two it cannot are worth knowing about:

- **MSRV.** This machine has Homebrew rust and no rustup, so the 1.88 toolchain cannot be
  installed to check against. Do not use a `std` API stabilised after 1.88 — the compiler
  here will happily accept it and nothing will complain until CI comes back.
- **The Linux and Windows test matrix.** Anything path-shaped, line-ending-shaped or
  terminal-shaped is unverified off macOS. Prefer `Path::join` over string concatenation and
  keep platform assumptions behind `cfg`.

`cargo build` can report `Fresh` while the binary on disk is stale. If a change does not
appear to take effect, `rm -rf target/debug/.fingerprint/rewind-*` and build again.

## Architecture

```
  ~/.claude/*.jsonl
        │
   domain/          parse. Knows nothing about drawing.
        │           scan → record → session → thread → Conversation
        ├── cache/  search corpus shards + meta.json
        │   search/ query → matcher → engine
        ▼
   render/          pure. Conversation → Vec<RenderedLine> of styled spans.
        │
        ▼
     ui/            reducer + view. Owns the terminal, never the filesystem.
```

**The browser is a reducer plus a view.** `ui::input::action(&Event, Mode) -> Option<Action>`
is the only place an event becomes an intent; `App::apply(Action)` is the only place state
mutates; everything in `ui` that draws is a `Widget` impl that only reads. `Action` derives
`Copy`, so it carries no `String` payloads — `Focus { forward: bool }`, never `Focus(String)`.

**`render` is pure.** It takes a `Conversation` and a `Ctx` and returns styled lines. It does
no I/O, holds no terminal, and is testable without a backend.

**The domain layer never draws.** Nothing under `domain/`, `cache/` or `search/` may depend
on `ratatui`, `crossterm` or `theme`.

**The UI thread never touches the filesystem.** Scanning, session parsing and search all run
on workers and report home over one `Sender<Wake>`. Superseded work is cancelled by a
generation counter, never by killing a thread: the sender bumps the counter, the worker
checks it periodically and bails, and `App::apply` drops any result whose generation is not
current. One mechanism, used for both session loads and search queries.

## The on-disk schema

These rules are invisible in the code and expensive to rediscover. Violating one produces a
transcript that looks plausible and is wrong.

- **Never decode the project directory name.** `/`, `.` and space all encode to `-`, so
  `-Users-otaviocc-Developer-tr-ios` is not invertible. Real paths come from re-encoding the
  keys of `~/.claude.json`'s `.projects` map and matching, falling back to the `cwd` field on
  a transcript record.
- **A session is a forest, not a list.** `parentUuid: null` appears at session start, after
  `/clear`, and after compaction. User rewinds and edits create genuine sibling branches.
- **`compact_boundary` records stitch through `logicalParentUuid`**, not `parentUuid`
  (which is null). Miss this and a compacted session renders as two disconnected halves.
- **One assistant message is split across several records** sharing a `message.id`, ordered
  by `apiBlockIndex`. Coalesce on `(message.id, requestId)` — a retry can reuse `message.id`
  on another branch — and **register every fragment's `uuid` in the id map, all pointing at
  the coalesced node**, because later records commonly parent off the *last* fragment.
- **`usage` is cumulative across fragments, not additive.** Take the fragment with the
  highest `apiBlockIndex`. Never sum; you will over-count by the fragment count.
- **A `user` record is not a human turn** if it has `toolUseResult`, or `isMeta: true`, or an
  `origin.kind` other than `"human"`.
- **Titles: take the last matching record in the file.** `custom-title` → `custom-title.json`
  → `ai-title` → `agent-name` → `last-prompt` → first human message. These are re-appended on
  every change, so the last one wins.
- **Subagents live in their own files** at `<sessionId>/subagents/agent-<hex>.jsonl`, linked
  to the spawning `tool_use` block by `toolUseId` in the adjacent `.meta.json`. Load the
  `.meta.json` files eagerly (they are ~200 bytes) so every `Agent` call renders with its
  real type and description; load the transcripts only when expanded.
- **Never deserialize base64.** The longest line in the corpus is 1.96 MB of inline image.
  Redact `"data":"…"` before `serde_json` sees the line.
- **Support the legacy shapes.** `type: "summary"` records, inline sidechains marked
  `isSidechain: true`, and the old tool names `Task`, `Grep`, `Glob`, `TodoWrite`.

Schema drift must never be silent. An unparseable line, an unknown record type or an unknown
content block is counted into `diagnostics`, surfaced in the status line as `· 3 unreadable`,
and listed by `d`.

## Performance

Two parse tiers, and knowing which one you are in is the whole story.

- **Metadata tier** — project lists, session lists, message counts, corpus extraction. Uses
  `domain::scan::top_level_str` and `memchr` only. **Never calls `serde_json`.**
- **Load tier** — opening a conversation. Dispatches on the scanned `type` to a concrete
  struct.

`type` is not the first key in a record, so `#[serde(tag = "type")]` at the top level would
buffer the entire `message` value — base64 included — just to read the tag. That is why the
hand-rolled scanner exists. Do not replace it with a serde-tagged enum.

Read with `BufReader::read_until(b'\n')` into one reused `Vec<u8>`, never `.lines()`.

The project list is derived from `read_dir` + `stat` + one parse of `~/.claude.json` in 3–5 ms
and **must never block on the cache**. The cache is a search corpus that happens to carry
session summaries; its absence should be invisible except for a progress line.

## Tests

`#[cfg(test)] mod tests` at the bottom of every source file, with full-sentence test names
(`fn a_compacted_session_stays_one_thread`). Integration suites in `tests/`, using `insta`
snapshots for rendered output and `ratatui::backend::TestBackend` for frames.

Tests read the fixture tree in `tests/data/claude`, never the developer's real `~/.claude`.
`XDG_CONFIG_HOME` is pinned at a nonexistent path in `tests/common/mod.rs` so a local theme
cannot repaint snapshots. `cargo-insta` is deliberately not used — new snapshots are reviewed
and moved by hand.

## Code conventions

- **No comments in Rust.** A file may carry a single `//!` line saying what it is, for
  navigation. Nothing else: no `///`, no `//`. A comment is a claim nobody checks, and it
  lends authority to whatever it sits above. Put the explanation in the commit message and
  the PR body, which are dated and tied to a diff. If code needs a paragraph to be
  understood, prefer a name, a smaller function, or a test. TOML and YAML in this repo *are*
  commented; the rule is about code.
- No `unsafe`. No `unwrap` outside tests; `expect` only with an invariant message.
- `anyhow` at the binary boundary, `thiserror` inside modules the UI matches on.
- `indexing_slicing`, `arithmetic_side_effects` and `as_conversions` are denied. Index with
  `.get()`, convert with `TryFrom`, and slice with `split_at_checked` / `first_chunk`. This
  is not friction to route around — it is what makes a corrupt cache file a rejected shard
  instead of a panic.
- `clippy -D warnings` makes `dead_code` and an unconstructed enum variant build failures.
  **That decides how a large feature splits: slice it by feature, not by module, so every PR
  constructs what it adds.**
- New inputs go into the existing context structs (`Ctx`, `Options`) rather than into
  parameter lists that ripple through every signature and every test.
- Add a dependency only when it earns its place, and say why in the commit body.
- `README.md` is a **product page** for someone using the program, with no architecture
  section. A change to a flag, key, theme key, or default updates it in the same PR (own
  commit, `docs:` prefix); a change to how the code works does not touch it.

## Workflow

Work is tracked as GitHub issues under a milestone. An issue's **Scope** checklist and
**Exit criteria** are the spec — read them first, and treat them as the definition of done.

```
gh issue list --milestone "M1 · Walking skeleton"
gh issue view 7
```

Branch as `t<issue>-<slug>` (e.g. `t7-top-level-scanner`). Conventional Commits. PR title
`<type>: <summary>`. `Closes #N` only on the PR that actually meets the exit criteria.

**Comment on the issue as you go.** Before starting, comment with the approach and anything
the issue did not anticipate. On finishing, comment with what was built, what changed from
the plan and why, the files touched, and how it was verified. A future session reads the
issue thread, not the diff, to understand why the code looks the way it does.

```
gh issue comment 7 --body "..."
```

If the work reveals something a later milestone needs to know, open or update that issue
rather than leaving it in a commit message. Never close an issue without a closing comment.

## Smoke-testing the TUI

ratatui paints runs and repaints only changed cells, so grepping stripped escape sequences
lies about what is on screen. Drive it through a pty at a fixed size:

```
script -q /dev/null sh -c 'stty rows 24 cols 80; ./target/debug/rewind --claude-dir tests/data/claude'
```

and reconstruct a frame from the capture with `tools/replay-frame.py`.

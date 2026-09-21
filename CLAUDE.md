# rewind

A read-only terminal browser for Claude Code conversation history: the projects Claude Code
has seen, the sessions inside them, and the conversation itself rendered as something worth
reading. No network requests, ever. No writes to `~/.claude`, ever.

## Commands

```
make check                                   THE GATE — fmt, clippy, test, msrv, audit
make build                                   cargo build --release
make test                                    cargo test
make fmt                                     cargo fmt
make lint                                    cargo clippy --all-targets -- -D warnings
make msrv                                    cargo +<rust-version> check --locked
make audit                                   cargo audit --deny warnings
cargo run -- --claude-dir tests/data/claude  run against the fixtures
```

**CI is disabled and verification is local.** The repository is private, and GitHub Actions
does not run on it. `.github/workflows/ci.yml` is committed and known-good but disabled at
the repository level.

So `make check` is the gate, not a convenience: **run it before every commit, and say in the
commit body that it passed.** Nothing else will catch a regression.

### Two machines

Development happens on both macOS and Fedora, and `make check` is the same command on
either — but they do not check the same things.

| | macOS | Fedora |
| --- | --- | --- |
| toolchain | Homebrew rust, no rustup | rustup (dnf) |
| `msrv` | skipped, with a notice | **runs** — this is where the MSRV is enforced |
| test matrix leg | macOS | Linux |

So between the two, four of the five CI jobs are covered on every platform CI would build
for except one. What is left:

- **Windows is unverified** while CI is off. Prefer `Path::join` over string concatenation,
  keep platform assumptions behind `cfg`, and do not assume `\n` line endings or a particular
  terminal.
- **The MSRV is only enforced on Fedora.** Work done solely on the macOS box can introduce a
  `std` API stabilised after `rust-version` and nothing will say so. Run `make check` on
  Fedora before calling macOS work finished, or expect to find it when CI returns.

`make msrv` reads `rust-version` straight out of `Cargo.toml`, the way the CI job does, so
the MSRV is stated in exactly one place. It skips where rustup is absent and **fails** where
rustup is present but the toolchain is not installed — that machine is expected to enforce
it, so silence there would be a lie. It tells you the `rustup toolchain install` command to
run.

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
transcript that looks plausible and is wrong. `tests/data/README.md` says which fixture
reproduces each of them.

```
~/.claude.json                               the projects map — a SIBLING, not inside
~/.claude/
  projects/<encoded>/<uuid>.jsonl            the transcript
  projects/<encoded>/<uuid>/
    custom-title.json
    subagents/agent-<hex17>.jsonl + .meta.json + .forked-skill{,.marker}.json
    tool-results/<base36>.txt                tool output too large for the transcript
  history.jsonl
  sessions/<pid>.json                        RUNNING processes, not session storage
```

- **A transcript file holds two disjoint record schemas, not one envelope with optional
  fields.** *Transcript* records — `assistant`, `user`, `attachment`, `system` — always carry
  `uuid`, `parentUuid`, `timestamp`, `isSidechain`, `cwd`, `sessionId`, `version`,
  `gitBranch`, `userType` and `entrypoint`. *Latch and event* records — `mode`, `ai-title`,
  `custom-title`, `agent-name`, `agent-color`, `last-prompt`, `atis-latch`,
  `permission-mode`, `cost-state`, `pr-link`, `frame-link`, `queue-operation`,
  `continued-in`, `fork-context-ref`, `artifact-*` — carry **only** `sessionId`, and
  sometimes `timestamp`. `file-history-snapshot` and `file-history-delta` carry no
  `sessionId` at all; they key off `messageId`. Do not model this as one struct.
- **Never decode the project directory name.** Each of `/`, `.` and space encodes to `-`
  and everything else survives verbatim — case, `_`, `+`, `(`, `)`, digits — so the encoding
  is not invertible. Real paths come from re-encoding the keys of `~/.claude.json`'s
  `.projects` map and matching, falling back to the `cwd` field on a transcript record when
  no key matches or several do.
- **The `.projects` map is not the enumeration.** Two thirds of its keys have no directory
  under `projects/`. Discovery reads `projects/` and the map only ever resolves a directory
  name back to a path. `projects/` also contains entries that are not project directories —
  a `.DS_Store` among them — so skip anything that is not a directory.
- **A session is a forest, not a list.** `parentUuid: null` appears at session start, after
  `/clear`, and after compaction. User rewinds and edits create genuine sibling branches.
- **A compaction boundary is `type: "system"` with `subtype: "compact_boundary"`**, not a
  top-level type of its own, so a scanner dispatching on `type` sees `system`. It stitches
  through `logicalParentUuid`, not `parentUuid` (which is null), and carries
  `compactMetadata`. Miss this and a compacted session renders as two disconnected halves.
- **One assistant message is split across several records** sharing a `message.id`, ordered
  by `apiBlockIndex`. Coalesce on `(message.id, requestId)` — a retry can reuse `message.id`
  on another branch — and **register every fragment's `uuid` in the id map, all pointing at
  the coalesced node**, because later records commonly parent off the *last* fragment.
- **`usage` is cumulative across fragments, not additive.** Take the fragment with the
  highest `apiBlockIndex`. Never sum; you will over-count by the fragment count.
- **A `user` record is not a human turn** if it has `toolUseResult`, or `isMeta: true`, or an
  `origin.kind` other than `"human"`.
- **`attachment` records are the context injections**, and there are nearly as many of them
  as there are `assistant` records. The payload is discriminated by `attachment.type` across
  some twenty shapes — `date`, `instructions`, `environment`, `queued_command`,
  `skill_listing`, `plan_mode`, `edited_text_file` and the rest. There is no
  `queued-command` record type; a queued command is an `attachment`.
- **Tool output too large for the transcript is persisted beside it.** The `tool_result`
  block keeps a `<persisted-output>` preamble and a preview, and `toolUseResult` carries
  `persistedOutputPath` plus `persistedOutputSize`. That path is **absolute**, so it does not
  resolve under `--claude-dir`: locate the file relative to the session directory instead.
  Overflow files with no record pointing at them accumulate, so do not treat one as a link.
- **Titles: take the last matching record in the file.** `custom-title` → `custom-title.json`
  → `ai-title` → `agent-name` → `last-prompt` → first human message. These are re-appended on
  every change, so the last one wins.
- **Subagents live in their own files** at `<sessionId>/subagents/agent-<hex>.jsonl`, linked
  to the spawning `tool_use` block by `toolUseId` in the adjacent `.meta.json`. Load the
  `.meta.json` files eagerly (they are ~200 bytes) so every `Agent` call renders with its
  real type and description; load the transcripts only when expanded. `toolUseId` is
  **optional**: a forked skill has none, so nothing in the parent points at it. A `.meta.json`
  can also have no transcript beside it at all. Only `agentType` and `spawnDepth` are always
  there. Sidechain records add `agentId` — the bare hex, no `agent-` prefix — and carry the
  *parent* session's `sessionId`.
- **Never deserialize base64.** The longest line in the corpus is 1.96 MB of inline image.
  Redact `"data":"…"` before `serde_json` sees the line.
- **Support the legacy shapes.** `type: "summary"` records, inline sidechains marked
  `isSidechain: true`, and the old tool names `Task`, `Grep`, `Glob`, `TodoWrite`. The first
  two no longer appear in a current store at all — `isSidechain: true` now occurs only inside
  `subagents/`, and `summary` nowhere — so their shapes in `tests/data` are reconstructed
  rather than observed. Confirm before relying on a field name.

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

Read with `fill_buf` + `memchr` + `consume` into one reused `Vec<u8>`, never `.lines()` and
never `read_until`. `read_until` appends the whole line before anyone can see how long it is,
so the 8 MB `MAX_LINE` cap could only reject a 100 MB line after it was already resident.
`fill_buf` stops appending at the cap and keeps consuming to the newline, so an oversized line
costs the cap and not the line.

The project list is derived from `read_dir` + `stat` + one parse of `~/.claude.json` in 3–5 ms
and **must never block on the cache**. The cache is a search corpus that happens to carry
session summaries; its absence should be invisible except for a progress line.

## Tests

`#[cfg(test)] mod tests` at the bottom of every source file, with full-sentence test names
(`fn a_compacted_session_stays_one_thread`). Integration suites in `tests/`, using `insta`
snapshots for rendered output and `ratatui::backend::TestBackend` for frames.

Tests read the fixture tree in `tests/data/`, never the developer's real `~/.claude`.
`tests/common/mod.rs` pins `HOME`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` and the Windows
equivalents at nonexistent paths, so a local theme cannot repaint a snapshot and a bug that
falls back to the real `~/.claude` fails loudly instead of quietly reading hundreds of
megabytes of personal history.

`tests/data/` is a fake `$HOME`, because `.claude.json` is a sibling of `.claude` rather than
a member of it. Every path inside it is under `/Users/fixture`, which exists nowhere, so the
tree reads with no setup and every project reads as gone. `common::fixture_tree` is what
makes a *present* working copy and a deterministic ordering testable: it copies the tree to a
tempdir, rewrites `/Users/fixture` to the tempdir root, re-encodes the project directory names
so the copy is a store that could exist, creates the working copies that are meant to exist,
and stamps mtimes derived from the timestamps in the data. Git preserves
neither mtimes nor the deliberately truncated line's bytes, so use the helper rather than
statting the checkout, and see `tests/data/README.md` before changing a fixture.

`cargo-insta` is deliberately not used — new snapshots are reviewed and moved by hand.

## Code conventions

- **No comments in Rust.** A file may carry a single `//!` line saying what it is, for
  navigation. Nothing else: no `///`, no `//`. A comment is a claim nobody checks, and it
  lends authority to whatever it sits above. Put the explanation in the commit message, which
  is dated and tied to a diff. If code needs a paragraph to be understood, prefer a name, a
  smaller function, or a test. TOML and YAML in this repo *are* commented; the rule is about
  code.
- No `unsafe`. No `unwrap` outside tests; `expect` only with an invariant message.
- `anyhow` at the binary boundary, `thiserror` inside modules the UI matches on.
- `indexing_slicing`, `arithmetic_side_effects` and `as_conversions` are denied. Index with
  `.get()`, convert with `TryFrom`, and slice with `split_at_checked` / `first_chunk`. This
  is not friction to route around — it is what makes a corrupt cache file a rejected shard
  instead of a panic.
- `clippy -D warnings` makes `dead_code` and an unconstructed enum variant build failures.
  **That decides how a large feature splits: slice it by feature, not by module, so every
  commit constructs what it adds.**
- New inputs go into the existing context structs (`Ctx`, `Options`) rather than into
  parameter lists that ripple through every signature and every test.
- Add a dependency only when it earns its place, and say why in the commit body.
- `README.md` is a **product page** for someone using the program, with no architecture
  section. A change to a flag, key, theme key, or default updates it on the same branch (own
  commit, `docs:` prefix); a change to how the code works does not touch it.

## Workflow

Branch off `main`, one branch per change, named for the change rather than for a ticket
(`top-level-scanner`, `narrow-threshold`). Conventional Commits. Rebase rather than merge, so
the history stays linear.

**The commit message is the only record.** There is no tracker and no thread: a future session
reads the log to understand why the code looks the way it does, so the body has to carry what
a thread used to. Say what the change is, what was wrong before, what was rejected and why,
and how it was verified. Put in the reasoning you would otherwise have to rediscover — the
constraint that made the obvious approach wrong, the measurement behind a threshold, the case
a test exists to pin.

**Write it for the diff, not for the moment.** The message describes the change itself, in
terms that are still true a year later:

- No ticket, issue, PR or milestone references, and no platform-specific links. Git outlives
  whatever is hosting it, and a `#7` either dangles or, worse, silently points at something
  unrelated once numbering restarts somewhere else.
- No "part 1 of", "follow-up to", "deferred from", "ticks the scope item". Work that only
  makes sense relative to other work should say what it *is* instead.
- Name the thing, not the number: "the append fast path", not "#20".
- Upstream references are fine — `rust-lang/rust#51114`, a vendored project's URL — because
  they point outward at something that exists independently.

A decision you made *against* belongs in the body as much as one you made for. Leaving it out
is what makes a future session re-litigate it.

Run `make check` before every commit, and say in the body that it passed and on which machine
— the MSRV is only enforced on Fedora, so which box ran it is part of the claim.

## Smoke-testing the TUI

ratatui paints runs and repaints only changed cells, so grepping stripped escape sequences
lies about what is on screen. Drive it through a pty at a fixed size, with a `q` piped in to
end the run — without it the session hangs — and `sleep 0.8` first to give the TUI time to
paint before the quit key lands:

```
# BSD/macOS script(1): [file [command ...]]
{ sleep 0.8; printf 'q'; } | script -q /dev/null sh -c 'stty rows 24 cols 80; ./target/debug/rewind --claude-dir tests/data/claude' > frame.bin
# util-linux script(1) needs -c instead
{ sleep 0.8; printf 'q'; } | script -q -c 'stty rows 24 cols 80; ./target/debug/rewind --claude-dir tests/data/claude' /dev/null > frame.bin
```

and reconstruct a frame from the capture with `tools/replay-frame.py <rows> <cols> <capture>`:

```
python3 tools/replay-frame.py 24 80 frame.bin
```

Repaints touch changed cells only, so when checking the capture for a change, grep for a
fragment (`Holodeck`), never a phrase — a phrase can straddle a repaint boundary and never
appear as one run.

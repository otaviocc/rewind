# The fixture tree

`tests/data/` is a fake `$HOME`. Tests read it and never the developer's real `~/.claude`,
which is hundreds of megabytes, changes under you, and is personal.

Every name in here is invented — a Star Trek theme, deliberately, so nothing can be mistaken
for a real project.

```
tests/data/                 the fake $HOME
  .claude.json              the projects map — a SIBLING of claude/, as it is on disk
  claude/                   the fake ~/.claude
```

`.claude.json` is not inside `claude/`. On a real machine the file is `~/.claude.json`, next
to `~/.claude`, so `tests/data/` has to be the root for the layout to be faithful. Resolve it
as `<claude_dir>/../.claude.json` and nothing else.

Every path inside the fixtures is under `/Users/fixture`, which exists on no machine, so
`cargo run -- --claude-dir tests/data/claude` works with no setup and every project reads as
gone. `tests::common::fixture_tree` is what makes a *present* working copy testable: it
copies the tree to a tempdir, rewrites `/Users/fixture` to the tempdir root, creates the
working copies that are meant to exist, and stamps deterministic mtimes.

## Two rules that are easy to break

**Byte-exactness.** `44444444-….jsonl` ends in a line cut mid-JSON with no trailing newline.
`.gitattributes` pins `tests/data/** -text` so EOL normalisation on a Windows checkout cannot
repair it. If it gets repaired, the fixture stops testing the unparseable-line path and
nothing says so.

**`projects/.DS_Store`** is a deliberate non-directory entry that project discovery has to
skip. Most developers ignore `.DS_Store` globally, so the repository `.gitignore` carries a
negation for this one path. Without it the file vanishes on someone else's clone.

## Projects

Discovery reads `projects/`. `.claude.json` only ever resolves a directory name back to a
real path — it never enumerates. Five of its eleven distinct names have no directory here,
mirroring the ratio in a real map.

| Directory | Real path | What it is for |
| --- | --- | --- |
| `-Users-fixture-Developer-holodeck` | `/Users/fixture/Developer/holodeck` | the four interesting sessions |
| `-Users-fixture-Developer-warp-core` | three colliding keys | **ambiguity.** `warp-core`, `warp.core` and `warp core` all encode to this one name. Only the `cwd` on a transcript record says which owns it |
| `-Users-fixture--tricorder` | `/Users/fixture/.tricorder` | a leading dot in the path |
| `-Users-fixture-Music-Red-Alert---Live-(2019)` | `/Users/fixture/Music/Red Alert - Live (2019)` | ` - ` collapsing to `---`, and parentheses surviving verbatim |
| `-Users-fixture-Developer-nomad` | `/Users/fixture/Developer/nomad` | listed in `.claude.json`, working copy absent |
| `-Users-fixture-Developer-jeffries-tube` | `/Users/fixture/Developer/jeffries-tube` | **absent from `.claude.json`.** Only the `cwd` fallback can name it, and it has no title latch, so the title must fall back to the first human message |
| `-Users-fixture-Developer-shuttlebay` | `/Users/fixture/Developer/shuttlebay` | no `.jsonl` at all, only a `.gitkeep` |

The encoding rule, confirmed against 20 of 20 real directories: replace each of `/`, `.` and
space with `-`, and leave everything else alone — case, `_`, `+`, `(`, `)`, digits. It is not
invertible, which is the whole reason `.claude.json` exists.

## Sessions

### `11111111-….jsonl` — the readable baseline

A human prompt turn, assistant prose with a `thinking` block, `Bash` / `Agent` / `Read` tool
calls with their results, three `attachment` subtypes, and two `system` subtypes.

Its tail is the **latch family**: records carrying only `sessionId` and no envelope at all.
They are ordered so both title rules are provable from one file — `custom-title` wins on
precedence even though an `ai-title` is appended after it, and the *second* `ai-title` wins
over the first on recency. Delete the `custom-title` record and the answer must become
`ai-title second, wins on recency`.

`custom-title.json` says something different from the `custom-title` record on purpose: the
record outranks the file, and the values differ so you can see which one was used.

`tool-results/b7k2m9x4q.txt` is **overflowed tool output**. When a result is too large the
transcript keeps a preview and writes the whole thing here, pointing at it with
`toolUseResult.persistedOutputPath` and a `<persisted-output>` preamble inside the
`tool_result` block. That path is **absolute**, so it does not resolve under `--claude-dir`;
the file sits where the session directory implies, which makes resolving it relative to the
session the only thing that works. `b0orphan1.txt` is an overflow file no record points at,
which real sessions accumulate.

`subagents/` holds three `.meta.json` files, and the differences are the point:

| File | |
| --- | --- |
| `agent-a1b2c3d4e5f607182` | meta + transcript, `toolUseId` joining it to a real `tool_use` block in the parent |
| `agent-b2c3d4e5f60718293` | meta + transcript, but **no `toolUseId`** — a forked skill, so nothing in the parent points at it. Carries the `forked-skill.json` + `.marker.json` sidecars |
| `agent-c3d4e5f607182934a` | **meta with no transcript**, `stoppedByUser`. Eager meta loading has to survive this |

Sidechain records set `isSidechain: true`, add `agentId` (the bare hex, no `agent-` prefix),
and carry the *parent* session's `sessionId`.

### `22222222-….jsonl` — compaction, then `/clear`

A first `parentUuid: null` root, then a compaction, then the same thread continuing, then a
second `parentUuid: null` root from a `/clear`. One session, two roots, and the first root
must not come out as two disconnected halves.

The boundary is `type: "system"` with `subtype: "compact_boundary"` — **not** a top-level
type. Its `parentUuid` is `null`, and `logicalParentUuid` is the only thing joining it to the
tail above it.

### `33333333-….jsonl` — fragmentation, retry, fork, base64

- **One** assistant message across three records sharing `message.id` *and* `requestId`,
  ordered by `apiBlockIndex` 0, 1, 2. `usage` is **cumulative**: the highest `apiBlockIndex`
  carries the true total, and summing the three over-counts threefold.
- The next record parents off **fragment three**, not off the first fragment, so every
  fragment's `uuid` has to be registered in the id map pointing at the coalesced node.
- A **retry** reusing the same `message.id` under a *different* `requestId`, on its own
  branch. Coalescing on `message.id` alone fuses it into the message above; the key is the
  pair.
- A **fork**: one `parentUuid` with two `user` children, as a rewind produces.
- An assistant record with **no `apiBlockIndex`**, which must read as a single fragment
  rather than as block zero.
- An inline base64 `image` of ~5 KB, enough that redacting before `serde_json` sees the line
  actually matters, plus a 92-byte one for the small case.

### `44444444-….jsonl` — legacy shapes, drift, and a truncated tail

Legacy shapes, and the old tool names `Task`, `Grep`, `Glob` and `TodoWrite`. The `summary`
record and the inline `isSidechain: true` pair are **reconstructed, not observed** — neither
appears anywhere in a current store, so this file is their only specification. Treat their
field names as a best guess and confirm before relying on them.

Also the three `user` shapes that are not human turns: `origin.kind` of `task-notification`,
`origin.kind` of `peer`, and `isMeta: true`.

This is the **diagnostics** fixture, and it carries exactly three defects:

1. an unknown content block — `server_tool_use`, a real API block type that no current store
   contains, so it is honest forward drift rather than an invention
2. an unknown top-level record type — `telemetry-latch`
3. the final line, cut mid-JSON with no trailing newline

That count is exact. If it changes, either the fixture or the diagnostics accounting moved.

### The rest

`55555555-…` through `99999999-…` are one exchange each, carrying only enough to be named,
counted, and to supply the `cwd` their project directory needs.

## The other files

- **`history.jsonl`** — one uniform shape, `timestamp` in epoch milliseconds. One line names
  a project with no directory under `projects/`: history outlives the transcript.
- **`sessions/`** — a registry of *running* Claude Code processes, not session storage. It is
  the source for the live badges: pid → `sessionId` → "running now". `4101.json` is `busy`
  and points at the baseline session; `999999.json` has a pid above any platform's `pid_max`,
  so it is reliably dead and its badge must not appear. The `.key` blob beside it is opaque
  and must be ignored rather than parsed.

# Bundled syntax definitions

The `.sublime-syntax` files in this directory fill the gaps in the set syntect
bundles, so that a rewind binary highlights a fenced code block in a transcript
without a separate install. They are compiled into the syntax set at build time
by `build.rs`.

Most are third-party work. Every one of those is redistributed under a licence
that permits it, and every licence here is compatible with rewind's own MIT
licence; each entry names the upstream commit the file was taken at, so it can
be checked or updated.

| File | Upstream | Commit | Licence |
| --- | --- | --- | --- |
| `Swift.sublime-syntax` | [aerobounce/Swift-Next](https://github.com/aerobounce/Swift-Next) | `258b6249d8c8` | MIT |
| `Kotlin.sublime-syntax` | [guille/sublime-kotlin](https://github.com/guille/sublime-kotlin) | `c353694169c0` | Unlicense (public domain) |
| `TOML.sublime-syntax` | [sublimehq/Packages](https://github.com/sublimehq/Packages) | `f29821e2f98f` | Sublime HQ Packages licence (below) |
| `TypeScript.sublime-syntax` | [sharkdp/bat](https://github.com/sharkdp/bat) | `d7b651942287` | Apache-2.0 |
| `Mermaid.sublime-syntax` | [otaviocc/vademecum](https://github.com/otaviocc/vademecum) | — | MIT |

All five were selected in `vademecum`, which bundles the same set for the same
reason and against the same syntect version and feature set. The notes below are
the part of that selection worth carrying, because they are the part that
constrains a replacement.

## Notes on individual files

**The rule for a replacement is both.** It has to load under `fancy-regex` —
rewind builds syntect with `default-fancy` and no Oniguruma, deliberately, to
avoid a C dependency — *and* it has to colour a sample containing nothing but
keywords. A sample with a string or a number in it passes on the strength of the
literal alone and says nothing about whether the grammar has keyword scopes.

**Swift.** Two others fail that rule.
[colinta/decent-swift-syntax](https://github.com/colinta/decent-swift-syntax)
cannot be used at all: it matches float literals with regex *subroutine calls*
(`\g<1>`), which `fancy-regex` does not implement, so it fails to load outright
with `FeatureNotYetSupported("Subroutine Call")`.
[wbond/swift-for-sublime](https://github.com/wbond/swift-for-sublime) loads but
covers literals only — its `expression` context never reaches the `identifier`
context it defines, and the file contains no `keyword` scopes at all, so a fence
of `import`/`class`/`func`/`if`/`return` comes out in a single colour.

**TypeScript.** Taken from `bat`, which converted it by hand from
[Microsoft/TypeScript-Sublime-Plugin](https://github.com/Microsoft/TypeScript-Sublime-Plugin)
(also Apache-2.0) — that upstream ships only `.tmLanguage`, and syntect 5.3 can
load plists for *themes* but not for syntaxes. The definition Sublime Text ships
today cannot be used either: it is `extends:`-based, chaining through
`TypeScript (Plain)` to a modern `JavaScript (Plain)`, so vendoring it would mean
replacing syntect's bundled JavaScript wholesale. This file is self-contained.

**Mermaid.** Written for vademecum rather than vendored from anywhere, and taken
from there under the same author's MIT licence. Edges (`-->`, `-.->`, `==>`) are
scoped `keyword.operator` and node identifiers `variable.other`, so a diagram
reads as declaration, direction, node, label and comment with the arrows as plain
connectors between them.

## Sublime HQ Packages licence

Applies to `TOML.sublime-syntax`, quoted from
<https://github.com/sublimehq/Packages/blob/master/LICENSE>:

> If not otherwise specified (see below), files in this repository fall under
> the following license:
>
>     Permission to copy, use, modify, sell and distribute this
>     software is granted. This software is provided "as is" without
>     express or implied warranty, and with no claim as to its
>     suitability for any purpose.
>
> An exception is made for files in readable text which contain their own
> license information, or files where an accompanying file exists (in the same
> directory) with a "-license" suffix added to the base-name name of the
> original file, and an extension of txt, html, or similar.

`TOML.sublime-syntax` carries no licence header of its own and has no
accompanying `-license` file upstream, so the terms above are the ones that
apply to it.

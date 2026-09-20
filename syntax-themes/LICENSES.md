# Bundled syntax themes

The `.tmTheme` files in this directory are compiled into the theme set by
`src/render/code.rs`, alongside the seven syntect ships, so that a theme naming
one works from a rewind binary with nothing installed beside it.

A syntect theme set is cheap to build, so these are `include_bytes!`ed directly
rather than baked by `build.rs` the way `syntaxes/` is.

| File | Upstream | Commit | Licence |
| --- | --- | --- | --- |
| `default-plus.tmTheme` | [otaviocc/snapcode](https://github.com/otaviocc/snapcode) | `af118d28c014` | MIT |

## Notes on individual files

**Default+.** The colours come from
[otaviocc/default-plus](https://github.com/otaviocc/default-plus), an Xcode Font
& Color theme ported to a couple of dozen applications, whose canonical palette
is that repository's `palette.yaml`. The TextMate expression of it — the scope
map this file holds — was written for
[snapcode](https://github.com/otaviocc/snapcode), and is taken verbatim rather
than re-derived from the palette, so that the two ports cannot drift apart. The
file is byte-for-byte the one `vademecum` bundles, for the same reason.

Default+ inverts two roles against the usual terminal convention: **comments are
green** and **strings are red**. That is the theme's signature, not a mistake; a
port that swaps them back stops looking like Default+. The unit test in
`src/render/code.rs` pins both colours for that reason.

The palette's `url` blue is the one colour the scope map spends nowhere, which
is why `themes/default-plus.toml` is free to give it to `highlight`.

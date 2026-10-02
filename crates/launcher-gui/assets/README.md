# Bundled icons

Lightforge's own chrome, compiled into the binary with `include_bytes!` so it
renders before any asset pipeline has answered. Product art is *not* here — it
comes from the manifest at runtime.

| File | Source | Licence |
| --- | --- | --- |
| `gear.png` | — | — |
| `chevron-down.png` | [Lucide](https://lucide.dev) `chevron-down`, stroked white and rendered to 64x64 | ISC, which GPL-3.0 may redistribute |
| `plus.png` | [Lucide](https://lucide.dev) `plus`, same treatment | ISC |
| `arrow-left.png`, `arrow-right.png` | Lucide `arrow-left` / `arrow-right`, same treatment | ISC |
| `file-text.png` | Lucide `file-text`, same treatment | ISC |
| `bug.png` | Lucide `bug`, same treatment | ISC |
| `message-square-quote.png` | Lucide `message-square-quote`, same treatment | ISC |
| `power.png` | Lucide `power`, same treatment | ISC |
| `discord.png` | [Simple Icons](https://simpleicons.org) `discord`, filled white, 64x64 | CC0 |
| `x.png` | Simple Icons `x`, same treatment | CC0 |

All are white with an alpha channel, 64x64, and drawn small (18pt for the cog,
16pt for the band's two marks): one source at a size no display asks to
upscale.

The two band marks — `chevron-down` and `plus` — were trimmed to their own ink
and then scaled so their strokes land on the same width: Lucide draws a chevron
narrower than a plus inside the same 24pt box, and side by side in the band that
reads as two different weights rather than two members of a set. The menu icons
are **not** trimmed, deliberately. They are read down a column against their
labels, where a shared box keeps the labels aligned and the marks optically the
same size; trimming each to its ink would make a wide one loom over a narrow one.

`discord.png` and `x.png` are brand marks, so the CC0 licence covers the files
rather than the trademarks: they are fine as links to those services, and must
not be restyled or used to imply either company endorses Lightforge. They are
filled where the Lucide set is stroked, which is why they sit in squares of
their own rather than in the menu's icon column.

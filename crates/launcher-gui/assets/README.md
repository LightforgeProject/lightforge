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
| `arrow-up-right.png` | Lucide `arrow-up-right`, stroked white and rendered to 64x64 from a viewBox cropped to its ink (`6 6 12 12`) | ISC |
| `file-text.png` | Lucide `file-text`, same treatment | ISC |
| `bug.png` | Lucide `bug`, same treatment | ISC |
| `message-square-quote.png` | Lucide `message-square-quote`, same treatment | ISC |
| `power.png` | Lucide `power`, same treatment | ISC |
| `ellipsis.png` | Lucide `ellipsis`, same treatment | ISC |
| `discord.png` | [Simple Icons](https://simpleicons.org) `discord`, filled white, 64x64 | CC0 |
| `x.png` | Simple Icons `x`, same treatment | CC0 |
| `news-background.jpg` | Supplied by Gabriel; resized, see below | — |

`news-background.jpg` is the odd one out: artwork, not a mark. Gabriel
supplied it at 6715x3719 and 19 MB; it ships at **2560x1418, JPEG q85,
progressive, 4:2:2** — about 500 KB. 2560 is `art::BACKGROUND_MAX_WIDTH`, the
same ceiling a product background gets, so nothing downscales it twice. Its
native 1.806 ratio is kept rather than cropped to 16:9: the page draws it
`ContentFit::Cover` at whatever shape the window is, so cropping here would
only throw away pixels the window might have wanted.

It is bundled rather than published through the asset manifest because that
manifest is keyed by product and this is chrome for a page that belongs to no
game. The cost is ~500 KB of binary; moving it to the asset repo is the
alternative if that ever matters.

The icons below are white with an alpha channel, 64x64, and drawn small (18pt for the cog,
16pt for the band's two marks): one source at a size no display asks to
upscale.

`arrow-up-right.png` is trimmed, like the band marks and unlike the menu
column: it is drawn beside a word in the navigation row, where a box of empty
margin would push it away from its label and make `OUTBOUND_GLYPH` mean
something other than the size it renders. Cropping the viewBox rather than the
rendered PNG keeps all 64 pixels on the mark itself.

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

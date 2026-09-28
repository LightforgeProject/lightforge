# Bundled icons

Lightforge's own chrome, compiled into the binary with `include_bytes!` so it
renders before any asset pipeline has answered. Product art is *not* here — it
comes from the manifest at runtime.

| File | Source | Licence |
| --- | --- | --- |
| `gear.png` | — | — |
| `chevron-down.png` | [Lucide](https://lucide.dev) `chevron-down`, stroked white and rendered to 64x64 | ISC, which GPL-3.0 may redistribute |
| `plus.png` | [Lucide](https://lucide.dev) `plus`, same treatment | ISC |

All are white with an alpha channel, 64x64, and drawn small (18pt for the cog,
16pt for the band's two marks): one source at a size no display asks to
upscale.

The two Lucide marks were trimmed to their own ink and then scaled so their
strokes land on the same width — Lucide draws a chevron narrower than a plus
inside the same 24pt box, and side by side in the band that reads as two
different weights rather than two members of a set.

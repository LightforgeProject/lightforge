# Bundled icons

Lightforge's own chrome, compiled into the binary with `include_bytes!` so it
renders before any asset pipeline has answered. Product art is *not* here — it
comes from the manifest at runtime.

| File | Source | Licence |
| --- | --- | --- |
| `gear.png` | — | — |
| `chevron-down.png` | [Lucide](https://lucide.dev) `chevron-down`, stroked white and rendered to 64x64 | ISC, which GPL-3.0 may redistribute |

Both are white with an alpha channel, 64x64, and drawn small (18pt for the cog,
16pt for the chevron): one source at a size no display asks to upscale.

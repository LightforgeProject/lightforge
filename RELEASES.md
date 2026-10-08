# Lightforge release notes

What changed, in the words of someone using the launcher rather than someone
reading its source. Newest first. The launcher shows the section for the
version it is running, once, after an update.

## 0.1.0

- Download, verify and launch every World of Warcraft flavour — Retail,
  Classic, Classic Era and their PTR and beta branches — without Battle.net.
- Set Battle.net up by itself: the Wine prefix, a verified Proton-GE build and
  the client, in three steps you can watch.
- Keep the library on more than one drive. A game can be installed to any
  drive you add, and Battle.net installs its own games to the one you make the
  default.
- A first run that asks where things go, finds the Blizzard games that were
  already on this machine, and keeps them exactly where they are.
- Pick a region and a language instead of downloading English from Europe.
- Addons from CurseForge, WoWInterface, Tukui and GitHub, with updates, a
  changelog for each, and WoWUp import and export.
- Game optimizations as three presets — Frames, Balanced, Looks — over the
  switches they set, with your own environment variables kept apart from them.
- Run the game on Proton's Wayland backend instead of XWayland, ask for HDR,
  and say which monitor it should treat as its primary.
- Give the driver's shader cache room, so a long session stops throwing away
  shaders it is about to need again.
- Cap the frame rate while the window is not focused, turn off cloth physics,
  keep the pointer inside the window, draw a software cursor, and ignore the
  desktop's mouse acceleration.
- The frame cap works. It was written without the setting that enables it, so
  it had never done anything.
- The games Battle.net launches get their own optimization profile, in
  Settings › Battle.net, covering the client and every game it starts.
- Per-game Proton: pin a build to one game, including the games Battle.net
  starts, and keep the in-game shop working.
- News from Wowhead, Icy Veins and Blizzard in one page.
- Battle.net can start minimized, and the launcher can get out of the way when
  a game comes up.

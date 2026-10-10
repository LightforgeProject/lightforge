# Lightforge release notes


## 1.0.0-beta.2

- Play comes back the moment the game closes. It used to stay dark until the
  whole Wine prefix had wound down behind the game — the voice proxy, the
  in-game shop's browser, then Wine's own background services — which on a
  long session reads as a launcher that has hung rather than one that is
  tidying up. The tidying still happens, out of your way.
- News on the Play page now starts off, so a first run is the game's artwork
  and nothing else. It is one switch in Settings under General, and anyone
  who already has it on keeps it.
- The download and install commands in the README and the install guide name
  the files GitHub actually serves for a release.

## 1.0.0-beta.1

- The first public build. Everything below is what it does.
- Install, update and launch every World of Warcraft flavour — Retail,
  Classic, Classic Era, Anniversary — with each line's PTR and beta branches
  behind its version picker.
- Downloads come straight from Blizzard's content servers, through the same
  storage format the real client uses, with no Battle.net involved and no
  Windows installer in the way.
- An interrupted download resumes where it stopped rather than starting over,
  and only one runs at a time so neither ETA is a guess.
- Check an install two ways: a fast pass that reads a header per file, and a
  deep one that hashes every stored byte against its own checksums. Both are
  safe to run while the game is open, and anything damaged is simply fetched
  again.
- Pick the region and the language you actually want, instead of downloading
  English from Europe.
- Optional extras like the high-resolution texture pack stay off unless you
  ask for them, and their size is shown before you decide. On one build that
  is 37 GiB nobody asked for.
- Remove a version and give back only the data no other version is using.
- Keep games on as many drives as you like. Add a partition as a library,
  choose where each game installs, and set the one Battle.net installs its own
  games to.
- A drive that is not mounted is reported as exactly that — the game is still
  on it, untouched — rather than quietly downloading it again. Libraries are
  recognised by a marker on the volume, so a drive that comes back on a
  different mount point is still found.
- See free space per drive, and clear caches per drive.
- Set Battle.net up by itself: the Wine prefix, a verified Proton-GE build and
  the client, in three steps you can watch.
- Everything you install through Battle.net gets a tile here and starts with
  one press — the launcher opens the client, waits for the sign-in, names the
  game and waits until you quit.
- The in-game shop and the WoW Token work, which took some finding.
- Stale processes from a previous session are cleared before anything starts,
  so Battle.net stops sitting on "Initializing Data...".
- Proton-GE is downloaded, checked against its published checksum and managed
  for you — no Steam, no Lutris, no system Wine needed. Pin a specific build
  to one game, including the games Battle.net starts.
- A first run that asks where things should go, picks region and language,
  finds the Blizzard games already on this machine and keeps them exactly
  where they are, whether they came from Lutris, Bottles, Heroic or a prefix
  you made by hand.
- Addons from CurseForge, WoWInterface, Tukui and GitHub in one list, with
  search, a most-downloaded list to browse, and changelogs.
- Each addon can follow stable, beta or alpha builds, be pinned to one
  version, or be left out of bulk updates entirely.
- Bring a collection over from WoWUp by pasting its export token, and export
  yours the same way.
- Addons you installed by hand are recognised by their contents and managed
  from then on.
- Where an author has asked not to be distributed by third-party tools,
  Lightforge says so and sends you to their page rather than going around it.
- Game optimizations as three presets — Frames, Balanced, Looks — over the
  settings they change, with your own environment variables kept separate.
- Run the game on Proton's Wayland backend instead of XWayland, ask for HDR,
  and say which monitor it should treat as its primary.
- Give the driver's shader cache room, so a long session stops throwing away
  shaders it is about to need again.
- Cap the frame rate while the window is not focused, turn off cloth physics,
  keep the pointer inside the window, draw a software cursor, and ignore the
  desktop's mouse acceleration.
- MangoHud and your own launch wrappers, for the games Lightforge starts and
  the ones Battle.net starts.
- The games Battle.net launches get their own optimization profile, in
  Settings, covering the client and everything it starts.
- News from Wowhead, Icy Veins and Blizzard in one page, and beside the game
  you are looking at.
- Report a bug or send feedback from inside the launcher, with a screenshot
  you can drop on the window and the last things the launcher said. Nothing is
  ever sent without pressing Send, and the report shows you exactly what it
  carries.
- Battle.net can start minimized, and the launcher can get out of the way when
  a game comes up.
- A command line with forty-five commands beside the launcher, for scripting
  and for looking at what Blizzard is actually serving.
- One 22 MB binary, no background service, nothing running once you close it.

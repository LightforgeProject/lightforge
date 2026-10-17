# Lightforge release notes


## 1.0.0-beta.5

Lightforge 1.0.0-beta.5 is a big one: full controller support for the Steam
Deck and Steam Machine, safer handling of your game files and addons, and
error messages that tell you how to fix the problem.

- Handheld/Console Mode. On SteamOS, setup asks whether this is a handheld or
  console (Steam Deck, Steam Machine) or a desktop PC running SteamOS, and
  Settings > General has the same switch on any system. With it on, Lightforge
  runs fullscreen, with buttons, tabs and game tiles sized for touch and
  gamepad.
- Full controller support in Handheld/Console Mode, setup wizard included. The
  D-pad, the left stick, A and B work out of the box, and text fields bring up
  Steam's on-screen keyboard. WoW: Forever plays well on a controller, so it
  mattered to me that Lightforge does too.
- Settings > General can add Lightforge to your Steam library with its own
  artwork, so it starts from Game Mode like any other game.
- New installer for SteamOS, Bazzite and other systems: download
  lightforge-installer.desktop from the releases page and double-click it. It
  downloads the newest version, verifies it, installs it into your home folder
  and starts Lightforge, with no terminal needed. Run it again any time to
  update.
- New game optimization for WoW: Forever that turns on its controller mode.
- Errors now say what to do. Common problems like a full drive, a lost
  connection or damaged game files come with a suggestion, and where possible
  a button that fixes it.
- Settings is now one page. The list on the left jumps to each section and
  marks the one you are reading.
- The cog next to Play now opens Game Settings: the game's Proton, its addons
  and its backups in one place. The cog menu itself is shorter.
- Your WoW settings folder (WTF) is backed up before the first launch of each
  new build. Five backups are kept per game, and you can restore or delete
  them in Game Settings.
- Drag games along the game bar to put them in any order, for example WoW:
  Forever first or Retail last.
- Right-click a game in the game bar to remove it. "Add a game" is now "All
  Games", and opening a game there no longer adds it to the bar until you
  install or play it.
- WoW: Forever now gets the addons made for its own game version.
- Addon updates can no longer lose an addon. The new version is fully written
  before the old one is replaced, so a full disk or a failed write leaves the
  installed addon as it was.
- A failed or interrupted Proton-GE install no longer leaves a broken
  half-installed build behind.
- Uninstalling a WoW version now frees all of its space by compacting the game
  data it shared with your other versions. Settings > Storage can also free
  game data that no installed version uses anymore.
- Only one task can write to a game's files at a time, so a download, a repair
  and a launch can no longer overlap.
- Settings you change in the game's own options now stay yours. Lightforge's
  optimizations no longer overwrite them at the next launch.
- Bug reports now have categories, and a contact address is required so I can
  reach you for follow-up testing.
- Bazzite, Silverblue and other Fedora Atomic systems now get working setup
  advice (rpm-ostree instead of dnf), and INSTALL.md explains how to install
  Lightforge there.
- Lightforge starts faster: it opens with the artwork it already has instead
  of waiting for a check.
- lightforge-gui --version now prints the version instead of starting the
  launcher.
- General stability and bug fixes.


## 1.0.0-beta.4

- Added the WoW: Forever stress test. It sits beside the beta in Forever's
  version picker, which stays on the beta until Forever launches.
- WoW: Forever now appears in Add a game.
- A game Blizzard offers as a pre-install says so: the button reads
  PRE-INSTALL, and a line under it says when the servers open. You can still
  start it before then to check the install works; the login is what waits.


## 1.0.0-beta.3

- Added Heroes of the Storm.
- A World of Warcraft install found during setup is now added even when its
  folder refuses new files. Only Battle.net's half is skipped in that case,
  and the launcher says so, where before the whole thing was abandoned and a
  game already on the disk sat there offering to download itself again.
- If the folder is simply missing its own write permission, Lightforge adds
  it back rather than asking you to run chmod on your own install, and tells
  you it did.
- A game that could not be added now says so in red instead of a grey line
  that looked exactly like the ones reporting success.
- An install sitting in a drive Lightforge already knows about is picked up
  instead of skipped, which is what happens if you run the first-run setup a
  second time.
- Downloads check that the install folder will accept a file before they
  start, rather than finding out at the end. An update that could not be
  finished used to fail after every byte was already on the disk.
- When something goes wrong, the message now carries the system's own reason
  — permission denied, read-only disk, no space — instead of only naming the
  folder it happened in.
- Games installed on an added drive are recorded for Battle.net under that
  drive, so Battle.net can find them instead of looking in the default
  library and reporting that the folder holds no game files.
- Lightforge no longer creates an empty World of Warcraft folder in every
  drive it manages. That folder looked like a broken install, and it took the
  name an existing install would otherwise be moved into.
- Quitting a game now leaves the Wine prefix tidy straight away. It used to
  be left for the next launch to clean up, which is why starting a game
  shortly after closing one could sit there for several seconds doing
  nothing visible. On a retail session that wind-down ran for nine seconds,
  and pressing Play anywhere inside it paid for the whole thing.
- Play comes back about a third of a second sooner once the game's window
  has closed.
- The two configuration files a client reads before it can start anything
  are now written into the game's own data while it downloads. The client
  used to fetch them itself on the first launch of every new build — about
  a second and a half spent waiting on a server for files that were already
  on the disk.
- The pre-launch check that clears the prefix now records how long each of
  its steps took, and it goes into a bug report. A launch that feels stuck
  can be answered with where the time went rather than a guess.
- The install dialog is wider, and the drive it offers no longer squeezes
  "1.37 TiB free" into three stacked words. Install folders inside your home
  directory are written as ~/Games/… instead of in full.
- You can cancel a download now rather than only pausing it. It is in the cog
  beside the Play button, and it is there whether the download is running,
  queued or paused.
- Blizzard games found during setup now appear in the game band as soon as you
  tick them. Battle.net's own games had to be picked a second time on a later
  step, which was asking the same question twice.
- Lightforge now tells you when a newer version has been released. It asks
  GitHub once while the launcher is starting, and if there is a newer one it
  says so with a link to the release. Nothing is downloaded or installed for
  you: the program belongs to whatever you installed it with, and that is
  where it should be replaced.

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

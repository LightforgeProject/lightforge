# When something goes wrong

Everything on this page has happened, to me, to a test VM or to someone who
told me about it. I only add problems somebody actually hit, so if yours is
missing, tell me in [Discord](https://discord.gg/vtTgmUV9Bq) and I'll add it.

Start with `lightforge doctor`, or Settings → About → Diagnostics in the app.
It checks most of the things on this page and gives you a report you can paste.

## Setting up

### Battle.net's installer dies after about a second

This is the most common problem on a fresh machine. It looks like a Lightforge
bug because every step before it succeeds.

Everything before it (Proton, the prefix, the registry work) is 64-bit.
`Battle.net-Setup.exe` is the **only 32-bit program in the whole setup**, so a
machine without 32-bit libraries gets through every other step and fails on
this one.

The launcher checks for this and shows the command for your distro.
[INSTALL.md](INSTALL.md#to-use-battlenet) has the table; on Fedora it's
`sudo dnf install wine.i686`.

If you've installed the 32-bit packages and it *still* exits immediately,
please tell me. It means the check passed and something else is missing that
the check can't see. Send a report; it includes the installer's own error
output.

### "the prefix at … has not been created yet", or "no Battle.net install at …"

Setup was declined or interrupted. Nothing is broken, it just hasn't been done
yet.

**Settings › Setup** continues where it stopped. Each step knows whether it's
already done, so running it again only does the missing parts.

### Battle.net opens and sits on "Initializing Data…"

An `Agent.exe` from an earlier session is still running and holds the lock the
new one needs.

Lightforge clears this before every launch (that's the pre-flight step). To do
it by hand, or when something is stuck without a launch:

```bash
lightforge preflight
```

If it keeps coming back, please report it. Pre-flight should catch a stale
Agent, so this is a bug in Lightforge.

### The in-game shop is a grey or black rectangle

The shop and the Token are drawn by a Chromium browser inside the game, and it
only works when the game is started from the right kind of path. Lightforge
handles this: the install gets its own Wine drive letter and the game is
started by its native path.

If you still see grey, check which Proton build the game uses.
**GE-Proton11-7** is the one I tested it with. You can pin a build per game
next to the version picker on the Play page.

It is not a login problem, and Lightforge doesn't touch your Battle.net
session for it.

## Games and downloads

### A game says "nvme2 is not mounted"

This isn't an error, and downloading won't fix it. Your game is still on a
drive that isn't connected right now, for example an unplugged external disk
or a mount that didn't come back after a reboot.

The button says RETRY and only checks again. Mount the drive and press it.
Lightforge won't install over it or start a new download, because that would
leave you with two copies of the game.

If the drive comes back at a different mount point, Lightforge finds it too.
Libraries are recognised by a marker file on the drive, not by their path, and
Lightforge looks for them at startup and on the Storage page.

### The download is much bigger than I expected

Some builds have optional content, usually high-resolution textures. On the
Forever beta the texture pack alone is **37 GiB**.

Before an install or update, the Play panel lists what's included:
*"includes optional content: …"*. The texture pack is a checkbox in the
install dialog, with its size next to it, and afterwards a switch called
**High-resolution textures** in **Game Settings › Graphics** (the cog next to
PLAY). It's off unless you turn it on, which is also Battle.net's default.
Turning it on starts the download; turning it off asks whether to delete the
texture files as well.

### The download is slower than my connection

Usually the limit is Blizzard's CDN for your region. I measured this: a
rewrite of the download code to get more speed made no difference, and I
reverted it.

Two things to check anyway: whether you set a speed limit in
**Settings › Downloads**, and whether another download is writing to the same
store. Only one transfer runs at a time, because two would share one
connection and both ETAs would be wrong.

### Battle.net wants to repair a game Lightforge downloaded

Battle.net only adopts an install it can identify, and it reads `.product.db`
for that. Without it you get a repair prompt or error 2800 instead of Play.

Lightforge writes it after every completed download. For an older install, or
one you moved by hand:

```bash
lightforge product-db
```

### Lightforge says the store needs a repair

There are two checks:

```bash
lightforge verify          # fast: is every file where the index says, at the right size?
lightforge verify --deep   # thorough: hash every stored byte against its own checksums
```

The fast one reads about 55 MB of a 140 GB store, because every file starts
with a 30-byte header that says what it is and where it belongs. The deep one
reads everything. Both are safe to run while the game is open: they only read,
and they notice if something changes underneath them.

If the *indices* are damaged and the data is fine:

```bash
lightforge reindex
```

That rewrites the `.idx` files without touching your archives. It fixed a real
retail store here that showed up as needing repair.

A repair is just a download: a damaged file is removed from the index, so the
next download sees it as missing and fetches exactly that file.

## Addons

### "the author has disabled third-party downloads"

Some authors don't allow third-party tools to download their addons, and
Lightforge respects that.

The row has an **Open page** button. Install it from there by hand, and
Lightforge will still manage it once it recognises the folders.

### An addon I know exists doesn't show up in search

Try its exact name. Some providers only find an addon by its exact name, so
"big wigs" may find nothing while "BigWigs" finds it right away.

The list you see before searching is sorted by total downloads, so older
addons come first. Tukui doesn't publish download counts, so its addons never
show up in that list; search for them by name.

### An addon I installed by hand isn't managed

Lightforge can recognise it by its files. **My Addons** shows a note at the
top, like *"3 addons were installed by hand"*, with a **Recognise them**
button. It fingerprints the folders, matches them against the catalogues and
adds every addon it can identify to updates, channels and pinning.

From the command line:

```bash
lightforge addon-adopt
```

It only takes folders that no managed addon already owns.

## The launcher itself

### Blank text, or it won't start at all

**Blank labels** mean there are no fonts. Lightforge doesn't ship its own and
uses whatever fontconfig provides. Every normal desktop has fonts; a minimal
container may not.

**A window that never appears, or a GPU that misbehaves**: force software
rendering.

```bash
ICED_BACKEND=tiny-skia lightforge-gui
```

It tries Vulkan first and falls back to software by itself, so you only need
this when the driver reports Vulkan support but doesn't work properly.

### NVIDIA on Wayland, driver older than 555

Drivers before 555 have no explicit sync, which a Wayland compositor needs to
show frames cleanly. Check which driver is loaded:

```bash
cat /proc/driver/nvidia/version
```

**The launcher window flickers or freezes**: draw it in software.

```bash
ICED_BACKEND=tiny-skia lightforge-gui
```

**The game flickers with the Wayland backend on**: switch it off in
**Game Settings** (or in **Settings › Battle.net** for the client and the
games it starts). The game then runs through XWayland. With these drivers the
switch's description says so.

A 555 or newer driver fixes both.

### Where are the logs?

In `~/.local/share/lightforge/logs/`: `lightforge-gui.log` for the launcher and
`lightforge-cli.log` for the command line. Each file is rotated at 4 MiB and
three old ones are kept, so they never grow past 16 MiB.

For more detail, start it with `LIGHTFORGE_LOG=debug`. For startup timing in
the terminal:

```bash
LIGHTFORGE_TRACE=1 lightforge-gui
```

Usually the in-app reporter is easier: it already includes the recent activity,
your GPU and driver, the session type and the version, and shows you all of it
before anything is sent.

### The release-notes popup never appeared

It shows once per version. `~/.config/lightforge/config.toml` records which
one you've seen:

```toml
patch_notes_seen = "1.0.0-beta.1"
```

Delete that line to see it again. The switch in **Settings › General** only
controls whether the popup may open; it doesn't reset which version you've
seen.

You can always read the notes without the popup, under **Lightforge Updates**.

## Something that isn't on this page

Use the reporter: **☰ › Report a bug**. You can drop a screenshot onto the
window, it shows you exactly what it will send, and nothing leaves your
machine until you press Send. [DISCLAIMER.md](DISCLAIMER.md#what-leaves-your-machine)
lists what's in it.

Or come and talk: **[discord.gg/vtTgmUV9Bq](https://discord.gg/vtTgmUV9Bq)**.

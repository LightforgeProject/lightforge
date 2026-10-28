# When something goes wrong

Everything on this page has actually happened — to me, to a test VM, or to
somebody who told me about it. I'd rather keep it short and real than pad it
with problems nobody has hit, so if yours isn't here, it probably isn't here
yet: come and tell me in [Discord](https://discord.gg/vtTgmUV9Bq) and it
gets added.

Start with `lightforge doctor`, or Settings → About → Diagnostics in the app.
It checks the usual suspects below and gives you a report to paste.

## Setting up

### Battle.net's installer dies after about a second

By far the most likely thing to go wrong on a fresh machine, and it looks like
a Lightforge bug because every step before it goes green.

Everything up to that point — Proton, the prefix, the registry work — is
64-bit and perfectly happy. `Battle.net-Setup.exe` is the **first and only
32-bit binary in the whole sequence**, so a machine with no 32-bit userspace
sails through setup and then falls over on the last step.

The launcher checks for this and prints the command for your distro.
[INSTALL.md](INSTALL.md#to-use-battlenet) has the table; on Fedora it's
`sudo dnf install wine.i686`.

If you've installed the 32-bit packages and it *still* exits immediately, I
want to hear about it — that means the check passed and the real dependency is
something the check can't see. Send a report; it carries the installer's own
error output.

### "the prefix at … has not been created yet", or "no Battle.net install at …"

Setup was declined, or interrupted part-way. Nothing is broken; it just hasn't
been done.

**Settings › Setup** picks up where it stopped. Each step knows whether it's
already done, so running it again costs only the parts that are missing.

### Battle.net opens and sits on "Initializing Data…"

The classic. An `Agent.exe` from a previous session is still alive and holding
the lock the new one wants.

Lightforge already cleans this up before every launch — that's what pre-flight
is. If you want to do it by hand, or something is stuck without a launch in
progress:

```bash
lightforge preflight
```

If it keeps coming back, that's worth reporting. Pre-flight not catching a
stale Agent is a bug in Lightforge, not in your machine.

### The in-game shop is a grey or black rectangle

The shop and the Token are drawn by Chromium inside the game, and it is very
particular about the path it's launched from. Lightforge already handles this:
the install gets its own Wine drive letter and the game is started by its
native path.

If you're still seeing grey, check which Proton build the game is pinned to —
**GE-Proton11-7** is the one this was verified on. Per-game pinning lives
beside the version picker on the Play page.

One thing it is *not*: a login problem. Nothing about the shop needs
Lightforge to touch your Battle.net session, and it doesn't.

## Games and downloads

### A game says "nvme2 is not mounted"

Not an error, and never fixed by downloading. Your game is intact on a drive
that isn't currently here — an external disk unplugged, a mount that didn't
come back after a reboot.

The button says RETRY and only does one thing: look again. Mount the drive
and press it. Lightforge deliberately won't install over the top or start a
fresh download, because the one thing worse than a missing game is a second
copy of it.

If the drive came back on a *different* mount point, that's handled too —
libraries are recognised by a marker file on the volume, not by their path,
and Lightforge goes looking at startup and on the Storage page.

### The download is much bigger than I expected

Optional content. Some builds carry extra categories — high-resolution
textures most often — and on the Forever beta the texture pack alone is
**37 GiB**.

Before an install or update, the Play panel spells out what's included:
*"includes optional content: …"*. The big one is offered as a checkbox in the
install dialog, with its size next to it, and afterwards as **High-resolution
textures** in **Game Settings › Graphics** (the cog beside PLAY). It's off
unless you ask, which is the same default Battle.net uses. Switching it on
starts the download; switching it off asks whether to delete the texture files
as well.

### The download is slower than my connection

Usually not us. The ceiling is normally Blizzard's CDN link for your region,
and that was measured rather than assumed — a rewrite of the download path to
chase more throughput made no difference and got reverted.

Two things worth checking anyway: whether you've set a speed limit in
**Settings › Downloads**, and whether something else is writing to the same
CASC store (only one transfer runs at a time, on purpose — two would split one
connection and make both ETAs fiction).

### Battle.net wants to repair a game Lightforge downloaded

Battle.net won't adopt an install it can't identify. The file it reads for
that is `.product.db`, and if it's missing you get a repair prompt or error
2800 instead of Play.

Lightforge writes it on every completed download. For an install that predates
that, or one that's been moved by hand:

```bash
lightforge product-db
```

### Lightforge says the store needs a repair

Two different checks, worth knowing apart:

```bash
lightforge verify          # fast: is every file where the index says, at the right size?
lightforge verify --deep   # thorough: hash every stored byte against its own checksums
```

The fast one reads about 55 MB of a 140 GB store, because every file carries a
30-byte header that proves its identity and position. The deep one reads
everything — and it's safe to run while the game is up, since it only reads
and notices if anything moved under it.

If the *indices* are what's damaged rather than the data:

```bash
lightforge reindex
```

That rewrites the `.idx` files without touching a byte of your archives. It's
what fixed a real retail store here that came up as needing repair.

Repair isn't a separate mechanism, by the way: a damaged entry is dropped from
the index, which makes the planner see the file as missing, which makes the
next download fetch exactly it and nothing else.

## Addons

### "the author has disabled third-party downloads"

Not a failure — a choice. Some authors opt out of distribution by third-party
tools, and Lightforge respects that rather than routing around it.

The row gives you an **Open page** button. Install it from there by hand and
Lightforge will still manage it afterwards, once it recognises the folders.

### An addon I know exists doesn't show up in search

Try its exact name. Some providers only answer confidently to an exact slug,
so "big wigs" may find nothing where "BigWigs" finds it immediately.

Also: the list you see before searching is ordered by lifetime downloads, which
skews towards old classics. And Tukui publishes no download counts at all, so
its addons never appear in that list — search for them by name.

### An addon I installed by hand isn't managed

Lightforge can recognise it by its contents. **My Addons** notices them and
says so at the top of the list — *"3 addons were installed by hand"* — with a
**Recognise them** button. It fingerprints the folders, matches them against
the catalogues, and pulls everything it can identify into updates, channels
and pinning like anything else.

From the command line:

```bash
lightforge addon-adopt
```

It only ever claims folders that are genuinely free — a folder already
belonging to a managed package is never stolen from it.

## The launcher itself

### Blank text, or it won't start at all

**Blank labels** mean no fonts. Nothing is embedded; text is drawn with
whatever fontconfig provides. Any normal desktop has this — a stripped
container doesn't.

**A window that never appears, or a GPU that misbehaves**: force software
rendering.

```bash
ICED_BACKEND=tiny-skia lightforge-gui
```

It tries Vulkan first and falls back on its own, so this is only for the cases
where the driver says yes and then doesn't deliver.

### NVIDIA on Wayland, driver older than 555

Drivers before 555 have no explicit sync, which a Wayland compositor needs to
show their frames cleanly. Check the one you have loaded:

```bash
cat /proc/driver/nvidia/version
```

**The launcher window flickers or freezes**: draw it in software.

```bash
ICED_BACKEND=tiny-skia lightforge-gui
```

**The game flickers with the Wayland backend on**: switch it off in
**Game Settings** (or in **Settings › Battle.net** for the client and what it
starts). The game goes back through XWayland. On these drivers the switch
says so in its description.

A 555 or newer driver fixes both.

### Where are the logs?

There aren't any, deliberately — no log file growing in a corner of your disk.
Run it from a terminal to see what it's saying, and for startup timing:

```bash
LIGHTFORGE_TRACE=1 lightforge-gui
```

In practice the in-app reporter is the better route: it already collects the
recent activity, your GPU and driver, the session type and the version, and
shows you all of it before anything is sent.

### The release-notes popup never appeared

It shows once per version. Your `~/.config/lightforge/config.toml` records
which one you've seen:

```toml
patch_notes_seen = "1.0.0-beta.1"
```

Delete that line to see it again. The switch in **Settings › General** only
controls whether the popup is allowed to open — it doesn't clear what you've
already seen, which catches people out.

You can always read them without the popup, under **Lightforge Updates**.

## Something that isn't on this page

Use the reporter — **☰ › Report a bug**. You can drop a screenshot straight
onto the window, it shows you exactly what it's about to send, and nothing
leaves your machine until you press Send. [DISCLAIMER.md](DISCLAIMER.md#what-leaves-your-machine)
spells out what's in it.

Or just come and talk: **[discord.gg/vtTgmUV9Bq](https://discord.gg/vtTgmUV9Bq)**.

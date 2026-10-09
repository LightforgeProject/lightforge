# Installing Lightforge

Packages for Arch, Debian, Fedora and Flathub are the next thing on my list.
Until they exist there's one way in, and it's not a bad one: Lightforge is a
Rust program with almost no dependencies, so building it is three commands and
a couple of minutes of your CPU fan.

## Build it

```bash
git clone https://git.tuxcore.pro/kenny/lightforge.git
cd lightforge
cargo build --release --locked
```

That gives you two binaries in `target/release/`:

- **`lightforge-gui`** — the launcher. This is the one you want.
- **`lightforge`** — the command line, if you like that sort of thing. See
  [docs/CLI.md](docs/CLI.md).

Run it straight out of the tree:

```bash
./target/release/lightforge-gui
```

Or put them somewhere on your `PATH` — they're self-contained, with every
icon and the fonts' worth of chrome compiled in, so there's no data directory
to install alongside them:

```bash
install -Dm755 target/release/lightforge-gui ~/.local/bin/lightforge-gui
install -Dm755 target/release/lightforge     ~/.local/bin/lightforge
```

`--locked` matters more than it looks: it builds against the exact dependency
versions the project was tested with, rather than whatever resolved today.

## What you need

### To build

A Rust toolchain, **1.86 or newer** — from [rustup](https://rustup.rs) if your
distro's is older, which it often is. That's genuinely it. No GTK, no Qt, no
Vulkan headers, no `-dev` packages to hunt down: the launcher links `libc`,
`libm` and `libgcc_s` and nothing else, because everything a desktop app
normally links against is opened at runtime instead.

### To run

**Fonts.** Text is drawn with whatever fontconfig gives us; nothing is
embedded. Any normal desktop has this. A bare container doesn't, and renders
blank labels.

**A GPU driver.** The launcher prefers Vulkan and falls back to software
rendering on its own if it can't get it, so it will start on almost anything.
If you've got a GPU that makes it unhappy, force the fallback:

```bash
ICED_BACKEND=tiny-skia lightforge-gui
```

**That's all WoW needs.** Proton-GE is downloaded and checked by Lightforge
itself, so you don't need Steam, Lutris, or a system Wine for any of it.

### To use Battle.net

One extra thing, and it catches people out: **a 32-bit userspace**.

Everything Lightforge does to build the prefix is 64-bit and works fine
without it. Then `Battle.net-Setup.exe` turns out to be the single 32-bit
binary in the whole sequence, so a machine without one sails happily through
Proton and prefix creation and dies in under a second on the last step. The
launcher checks for this before it starts and tells you the command for your
distro, but here they are:

| | |
| --- | --- |
| **Fedora** | `sudo dnf install wine.i686` ✅ verified on Fedora 44 |
| **Debian / Ubuntu** | `sudo dpkg --add-architecture i386 && sudo apt update && sudo apt install wine32:i386` |
| **Arch** | `sudo pacman -S wine` (multilib must be enabled) |
| **openSUSE** | `sudo zypper install wine-32bit` |

Being honest about that table: **only the Fedora line has been run on a real
machine.** The others are right as far as I can tell from how those
distributions are put together, but I haven't proved them, and I'd rather say
so than let you find out. If one of them is wrong, tell me in Discord and I'll
fix it the same day.

Two things worth knowing if you go digging:

- It's named as the *Wine* package on purpose, even though Lightforge uses
  Proton's own Wine. What's actually needed is the 32-bit graphics stack
  underneath it, and the distro's `wine` package is maintained by people who
  package that for a living. On Fedora, `glibc.i686` and `freetype.i686`
  alone pass every check and the installer still fails.
- Installing Steam also fixes it, on Debian at least — but only by accident,
  because the Steam package enables i386 and drags the 32-bit chain in with
  it. Nobody should need Steam to run this.

**Don't want Battle.net at all?** Decline it during setup. WoW works without
it; you lose the shop, the Token and the other games.

## A binary that runs on other machines

If you want to hand the result to someone, don't hand them a build from your
own machine — it demands whatever glibc you happen to have, which on a rolling
distro is a version nobody else has yet. Measured here: a local build wanted
glibc 2.44, which SteamOS and Bazzite are both older than, so it started on
neither.

```bash
make release
```

builds inside a Debian 12 container (glibc 2.36) and **refuses to finish if
the result asks for anything newer**, which is the bit that makes it
trustworthy rather than hopeful. The binaries land in
`target/container/release/`, stripped, and run on Arch, Fedora, Ubuntu,
Bazzite and the Steam Deck alike.

Needs `podman` (or `make release CONTAINER=docker`).

## First run

The launcher walks you through a short setup the first time:

1. **Where games should live** — by default `~/Games/Blizzard`, and you can
   add other drives later or right now.
2. **Region and language**, so you're not downloading English from Europe
   when you wanted neither.
3. **A scan for Blizzard games already on this machine.** If you've played
   through Lutris, Bottles, Heroic or a hand-made prefix, your installs are
   found and adopted **where they already are** — nothing is moved and
   nothing is re-downloaded. This reads Battle.net's own registry rather than
   crawling your disks.
4. **Battle.net**, if you want it — the prefix, a verified Proton-GE build
   and the client itself, in three steps you can watch.

Then you're on the Play page and it's one button.

## Updating

```bash
git pull
cargo build --release --locked
```

Nothing migrates behind your back; your config, games and addons are outside
the build directory and stay where they are. There's no self-updater yet,
which is one of the better arguments for the distro packages.

## Uninstalling

Delete the binaries, and then as much of this as you want gone:

```text
~/.config/lightforge/        settings
~/.local/share/lightforge/   Proton builds, addon ledger
~/.cache/lightforge/         artwork, news, catalogues — safe to delete any time
```

The games are somewhere else on purpose:

```text
~/Games/Blizzard/prefix/     the Wine prefix — disposable, rebuild any time
~/Games/Blizzard/games/      your actual games. Possibly 120 GB of them.
```

Removing the prefix costs you nothing but a rebuild. **Removing `games/` costs
you the downloads**, so check what's in there first — and if you added other
drives as libraries, each of those has its own `Lightforge/games/` folder too.

## When the packages land

They'll be on the releases page, and in the usual places:

- **Arch** — the AUR, both a source package and a prebuilt one.
- **Debian / Ubuntu** — a `.deb`.
- **Fedora** — an `.rpm`.
- **Everything else** — a Flatpak, which is also the answer for the Steam Deck.

## Stuck?

Come and say so in **[Discord](https://discord.gg/vtTgmUV9Bq)** — install
problems are the most useful bug reports I get, because they're the ones I
can't reproduce on my own machine.

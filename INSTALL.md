# Installing Lightforge

There's a package for Debian/Ubuntu, one for Fedora, and a PKGBUILD for Arch.
Failing that, Lightforge is a Rust program with almost no dependencies, so
building it is three commands and a couple of minutes of your CPU fan.

Everything below installs the same two binaries, the menu entry and the icons.

## Debian, Ubuntu, Mint

Download `lightforge_<version>_amd64.deb` from the
[releases page](https://github.com/LightforgeProject/lightforge/releases).
The filename says `1.0.0.beta.1` because GitHub will not serve a `~` in an
asset name; the version inside the package, which is the one `apt` reads and
orders by, is `1.0.0~beta.1`:

```bash
sudo apt install ./lightforge_1.0.0.beta.1-1_amd64.deb
```

It asks for glibc 2.36 or newer — Debian 12, Ubuntu 22.10 and anything after
them. `apt` pulls in nothing else; see [what you need](#to-run) below.

Ubuntu 22.04 LTS is glibc 2.35 and `apt` will refuse the package. The binaries
themselves only import 2.34 and 2.35, so the [tarball](#steam-deck-and-everything-else)
below runs there — the `.deb` states the container's own 2.36 because that is
the version it can actually promise for every future build.

## Fedora, RHEL, openSUSE

Download `lightforge-<version>.x86_64.rpm` from the same place:

```bash
sudo dnf install ./lightforge-1.0.0.beta.1-1.x86_64.rpm   # or zypper install
```

## Arch, CachyOS, EndeavourOS

**Not on the AUR yet** — registrations there are closed and there's no account
to upload from. The PKGBUILDs are finished and in the repository, so until
that changes:

```bash
git clone https://git.tuxcore.pro/kenny/lightforge.git
cd lightforge/packaging/aur/lightforge-bin   # prebuilt binaries
makepkg -si
```

Use `packaging/aur/lightforge` instead to build it from source. Both were
built and installed in a clean Arch container before they were committed, and
both land on the AUR as `lightforge` and `lightforge-bin` the day an account
is possible.

## Steam Deck, and everything else

A Flatpak is the answer there and isn't done yet. In the meantime the
[portable tarball](https://github.com/LightforgeProject/lightforge/releases)
runs anywhere with glibc 2.36 or newer, SteamOS included:

```bash
tar xzf lightforge-1.0.0-beta.1-x86_64-linux.tar.gz
cd lightforge-1.0.0-beta.1
install -Dm755 lightforge-gui ~/.local/bin/lightforge-gui
install -Dm755 lightforge     ~/.local/bin/lightforge
install -Dm644 pro.tuxcore.Lightforge.desktop ~/.local/share/applications/
cp -r icons/hicolor ~/.local/share/icons/
```

## Build it yourself

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

Or install it properly, which also gives you the menu entry and the icons a
package would:

```bash
make install PREFIX=~/.local          # no root needed
sudo make install                     # or system-wide, into /usr/local
```

`make uninstall` takes the same `PREFIX` and removes exactly what it put
there. The binaries are self-contained — every icon and the chrome are
compiled in — so there's no data directory either way.

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

With a package, however your system updates everything else — install the new
`.deb` or `.rpm` over the old one, or `makepkg -si` again on Arch. From a
source tree:

```bash
git pull
cargo build --release --locked
```

Nothing migrates behind your back; your config, games and addons are outside
the build directory and stay where they are. There's no self-updater, which is
most of the argument for installing a package rather than a binary.

## Uninstalling

`sudo apt remove lightforge`, `sudo dnf remove lightforge`,
`sudo pacman -R lightforge-bin`, or `make uninstall PREFIX=~/.local` — then as
much of this as you want gone:

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

## Stuck?

Come and say so in **[Discord](https://discord.gg/vtTgmUV9Bq)** — install
problems are the most useful bug reports I get, because they're the ones I
can't reproduce on my own machine.

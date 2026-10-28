# Installing Lightforge

There's a package for Debian/Ubuntu, a COPR repository and a package for
Fedora, a PKGBUILD for Arch, and an installer for SteamOS, Bazzite and
everything else. You can also build it yourself: it's a Rust program with
very few dependencies, so that takes three commands and a few minutes.

Everything below installs the same two binaries, the menu entry and the icons.

## Debian based (Debian, Ubuntu, Pop!_OS, Mint…)

Download `lightforge_<version>_amd64.deb` from the
[releases page](https://github.com/LightforgeProject/lightforge/releases).
The file name says `1.0.0.beta.7` because GitHub doesn't allow a `~` in an
asset name. The version inside the package, which `apt` reads and sorts by,
is `1.0.0~beta.7`:

```bash
sudo apt install ./lightforge_1.0.0.beta.7-1_amd64.deb
```

It needs glibc 2.36 or newer: Debian 12, Ubuntu 22.10 or anything later.
`apt` installs nothing else with it; see [what you need](#to-run) below.

Ubuntu 22.04 LTS has glibc 2.35, and `apt` will refuse the package. The
binaries themselves only need 2.34 and 2.35, so the
[installer](#steamos-bazzite-and-other-systems) below works there. The `.deb`
asks for 2.36 because that's the version of the build container, and the only
one I can promise for every future build.

## Fedora

### From COPR (recommended)

Lightforge has its own [COPR repository](https://copr.fedorainfracloud.org/coprs/simplykenny/lightforge/),
built from source for every current Fedora release. Add it once, and
`dnf upgrade` updates Lightforge together with the rest of your system:

```bash
sudo dnf copr enable simplykenny/lightforge
sudo dnf install lightforge
```

### The .rpm directly

Without the repository, download `lightforge-<version>.x86_64.rpm` from the
[releases page](https://github.com/LightforgeProject/lightforge/releases)
and install it by hand. You then download each new release yourself:

```bash
sudo dnf install ./lightforge-1.0.0.beta.7-1.x86_64.rpm
```

The same file installs on openSUSE with `sudo zypper install`. Like the
`.deb`, it needs glibc 2.36 or newer.

## Arch, CachyOS, EndeavourOS

**Not on the AUR yet.** AUR registrations are closed, so I have no account to
upload from. The PKGBUILDs are finished and in the repository, so for now:

```bash
git clone https://git.tuxcore.pro/kenny/lightforge.git
cd lightforge/packaging/aur/lightforge-bin   # prebuilt binaries
makepkg -si
```

Use `packaging/aur/lightforge` instead to build it from source. I built and
installed both in a clean Arch container before committing them, and both go
on the AUR as `lightforge` and `lightforge-bin` once I can get an account.

## SteamOS, Bazzite and other systems

The installer works on any distribution with glibc 2.36 or newer, needs no
root, and only touches your home folder. Run it again any time to update.

**Double-click:** download `lightforge-installer.desktop` from the
[releases page](https://github.com/LightforgeProject/lightforge/releases) and
double-click it. Your desktop will warn that the file is untrusted; choose
Continue. A terminal window shows the download, checks it, installs Lightforge
and starts it. GNOME Files opens the file as text instead of running it, so on
GNOME use the command below.

**Install from a terminal:**

```bash
curl -fsSL https://raw.githubusercontent.com/LightforgeProject/lightforge/main/packaging/installer/install.sh | bash
```

**Uninstall:**

```bash
curl -fsSL https://raw.githubusercontent.com/LightforgeProject/lightforge/main/packaging/installer/install.sh | bash -s -- --uninstall
```

Uninstalling removes Lightforge itself. Your games, the Wine prefix and your
settings stay where they are; see [Uninstalling](#uninstalling) for where they
are.

On a Steam Deck, do this in Desktop Mode (Steam button, Power, Switch to
Desktop). SteamOS already has the 32-bit libraries Battle.net's installer
needs, so leave `steamos-readonly` alone. The first-run setup asks whether the
device is a Steam Deck or Steam Machine or a desktop PC running SteamOS.
Retail World of Warcraft is well over 100 GB, so on a Deck add your SD card as
a drive during setup.

## Build it yourself

```bash
git clone https://git.tuxcore.pro/kenny/lightforge.git
cd lightforge
cargo build --release --locked
```

That gives you two binaries in `target/release/`:

- **`lightforge-gui`**: the launcher.
- **`lightforge`**: the command line. See [docs/CLI.md](docs/CLI.md).

Run it from the build folder:

```bash
./target/release/lightforge-gui
```

Or install it, which also adds the menu entry and the icons, like a package
would:

```bash
make install PREFIX=~/.local          # no root needed
sudo make install                     # or system-wide, into /usr/local
```

`make uninstall` takes the same `PREFIX` and removes exactly what it
installed. Every icon and the window decorations are compiled into the
binaries, so there's no data directory either way.

Use `--locked`: it builds with the exact dependency versions the project was
tested with, instead of whatever is newest today.

## What you need

### To build

A Rust toolchain, **1.90 or newer**. Get it from [rustup](https://rustup.rs)
if your distro's is older. You don't need GTK, Qt or Vulkan headers. The only
`-dev` package is libudev's, for gamepads (`libudev-dev` on Debian/Ubuntu,
`systemd-devel` on Fedora, part of `systemd` on Arch). The launcher links
`libc`, `libm`, `libgcc_s` and `libudev` and nothing else; everything else a
desktop app needs is loaded at runtime.

### To run

**Fonts.** Text is drawn with whatever fontconfig provides; no fonts are
built in. Every normal desktop has them. A minimal container doesn't, and
shows blank labels.

**A GPU driver.** The launcher uses Vulkan and falls back to software
rendering by itself if Vulkan doesn't work, so it starts almost anywhere. If
your GPU gives it trouble, force software rendering:

```bash
ICED_BACKEND=tiny-skia lightforge-gui
```

**That's all WoW needs.** Lightforge downloads and checks Proton-GE itself, so
you don't need Steam, Lutris or a system Wine.

### To use Battle.net

Battle.net needs one more thing: **32-bit libraries**.

Everything Lightforge does to build the prefix is 64-bit and works without
them. `Battle.net-Setup.exe` is the only 32-bit program in the setup, so
without 32-bit libraries, Proton and the prefix set up fine and then the
installer exits within a second. The launcher checks for this before it
starts and shows the command for your distro. Here they are:

| | |
| --- | --- |
| **Fedora** | `sudo dnf install wine.i686` ✅ verified on Fedora 44 |
| **Fedora Atomic** (Bazzite, Silverblue) | `rpm-ostree install --apply-live wine.i686` (if it refuses: without the flag, then reboot) |
| **Debian / Ubuntu** | `sudo dpkg --add-architecture i386 && sudo apt update && sudo apt install wine32:i386` |
| **Arch** | `sudo pacman -S wine` (multilib must be enabled) |
| **openSUSE** | `sudo zypper install wine-32bit` |

**Only the Fedora line has been tested on a real machine.** The others should
be right for how those distributions are packaged, but I haven't tested them.
If one is wrong, tell me in Discord and I'll fix it.

Two more details:

- The table names the *Wine* package even though Lightforge uses Proton's own
  Wine. What's really needed is the 32-bit graphics stack, and the distro's
  `wine` package pulls in all of it. On Fedora, `glibc.i686` and
  `freetype.i686` alone pass every check, and the installer still fails.
- On Debian, installing Steam also fixes it, because the Steam package enables
  i386 and pulls in the 32-bit libraries. You shouldn't need Steam for this,
  though.

**You can skip Battle.net.** Decline it during setup. WoW works without it,
but you won't have the in-game shop, the Token or the other games.

## A binary that runs on other machines

If you want to give a build to someone else, don't use one built on your own
machine. It needs the glibc version you have, and on a rolling distro that's
newer than everyone else's. Here, a local build needed glibc 2.44, which is
newer than SteamOS and Bazzite have, so it didn't start on either.

```bash
make release
```

builds inside a Debian 12 container (glibc 2.36) and **fails if the result
needs anything newer**. The binaries end up in `target/container/release/`,
stripped, and run on Arch, Fedora, Ubuntu, Bazzite and the Steam Deck.

You need `podman` (or `make release CONTAINER=docker`).

## First run

The first time, the launcher takes you through a short setup:

1. **Where games should live**: `~/Games/Blizzard` by default. You can add
   other drives now or later.
2. **Region and language**, so you get the right game version and language.
3. **A scan for Blizzard games already on this machine.** If you've played
   through Lutris, Bottles, Heroic, Faugus or your own prefix, your installs
   are found and used **where they are**. Nothing is moved or downloaded
   again. The scan reads Battle.net's own registry, so it doesn't search your
   whole disk.
4. **Battle.net**, if you want it: the prefix, a verified Proton-GE build and
   the client itself, in three steps you can follow.

After that you're on the Play page.

## Updating

When a new version is out, Lightforge shows an update notice:

- **Installer**: the notice updates Lightforge for you. Running the installer
  again works too.
- **`.deb` or `.rpm` you downloaded**: the notice fetches and checks the new
  package and opens a terminal where your package manager installs it and
  asks for your password. Or install the new file over the old one yourself.
- **Fedora COPR and Fedora Atomic**: `sudo dnf upgrade`, or your system's
  normal update.
- **Arch**: `makepkg -si` again with the new PKGBUILD.
- **From source**:

```bash
git pull
cargo build --release --locked
```

Updating never touches your config, games or addons; they are stored outside
the program.

## Uninstalling

`sudo apt remove lightforge`, `sudo dnf remove lightforge`,
`sudo pacman -R lightforge-bin`, the installer's
[uninstall command](#steamos-bazzite-and-other-systems), or
`make uninstall PREFIX=~/.local`. Then delete whichever of these you want
gone:

```text
~/.config/lightforge/        settings
~/.local/share/lightforge/   Proton builds, addon ledger, logs
~/.cache/lightforge/         artwork, catalogues (safe to delete any time)
```

The games are stored separately:

```text
~/Games/Blizzard/prefix/     the Wine prefix (can be rebuilt any time)
~/Games/Blizzard/games/      your games, often more than 100 GB
```

Deleting the prefix only means it gets rebuilt. **Deleting `games/` deletes
your downloads**, so check what's in there first. If you added other drives as
libraries, each of them has its own `Lightforge/games/` folder too.

## Getting help

Ask in **[Discord](https://discord.gg/vtTgmUV9Bq)**. Install problems are the
reports that help me most, because I can't reproduce them on my own machine.

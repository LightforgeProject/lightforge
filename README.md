<div align="center">

<img src="docs/banner.png" alt="Lightforge" width="640">

**A native Linux launcher for World of Warcraft — and a proper home for the rest of Battle.net.**

[![CI](https://github.com/LightforgeProject/lightforge/actions/workflows/ci.yml/badge.svg)](https://github.com/LightforgeProject/lightforge/actions/workflows/ci.yml)
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-E9AB28?style=flat-square)](LICENSE)
[![Status: beta](https://img.shields.io/badge/status-1.0.0--beta.4-00B4FF?style=flat-square)](RELEASES.md)
[![Linux](https://img.shields.io/badge/platform-Linux-8A94A3?style=flat-square)](#requirements)
[![Discord](https://img.shields.io/badge/chat-Discord-5865F2?style=flat-square)](https://discord.gg/vtTgmUV9Bq)

</div>

<img src="docs/play.jpg" alt="The Play page" width="100%">

---

Lightforge installs, patches, verifies and launches World of Warcraft on Linux
without the Battle.net client. It speaks Blizzard's own distribution protocols
— TACT and NGDP for the CDN, CASC for local storage — reimplemented from
scratch in Rust, so the game comes down the same pipe the real client uses and
lands in a store the real client can read.

That last part is the whole point. Run an install through Lightforge, then
point Battle.net at the folder, and Battle.net adopts it: same storage format,
same `.build.info`, no re-download. The two are interchangeable, and you get to
pick which one you open.

Where it has been proven so far: a **123 GiB Retail install** downloaded,
verified byte-for-byte against its own checksums, adopted by Battle.net and
played; a full Classic Era install from empty; a store rebuilt from scratch
after deliberate corruption. **775 tests** in the tree. One 22 MB binary, no
Electron, no background service, nothing running when the launcher is closed.

It is a beta, it has had one pair of eyes on it, and the section below says
plainly what it does not do.

## Why this exists when Lutris and Bottles already run Battle.net

They run **the client**, under Wine, and that is the part that hurts: Agent
wedges itself, the client idles at "Initializing Data…", updates take a scenic
route through a Windows installer, and the launcher weighs more than the thing
it launches. Lutris, Bottles and Heroic all make that client easier to run.
None of them replace it, because replacing it means implementing Blizzard's
content pipeline, and nobody had.

Lightforge replaces it for WoW. For everything else — Diablo, Overwatch,
Hearthstone, StarCraft — it keeps one well-maintained Battle.net prefix and
hands off to the client, which is genuinely good at managing its own games.
You install them the usual way, they appear as tiles here, and they start with
one press.

## What it does

**Installs and updates WoW itself.** Retail, Classic, Classic Era,
Anniversary and WoW: Forever, and the PTR, beta and stress-test branches behind
each one's version picker. A game Blizzard offers as a pre-install downloads
ahead of time and says when its servers open.
Real CASC storage, resumable downloads, and verification that checks every
stored byte against its own checksums without decoding anything — including
while the game is running.

**Keeps your games on whatever drives you have.** Add a partition as a library
and install anything to it, Battle.net's own games included. Pull the drive
out and Lightforge tells you so instead of guessing.

**Manages your addons.** CurseForge, WoWInterface, Tukui and GitHub in one
list, with update channels per addon, changelogs, pinning, and WoWUp import
and export so you can bring your collection over in one paste.

**Tunes the game.** Three presets over real console variables, Proton's
Wayland backend with HDR, shader-cache headroom, a background frame cap,
cursor fixes that actually exist in the client, and your own environment
variables kept well away from all of it.

**Sets Battle.net up for you, and then gets out of the way.** The Wine prefix,
a verified Proton-GE build and the client itself, in three steps you can
watch. Everything you install through it gets a tile here and starts with one
press: Lightforge opens the client, waits for the sign-in, tells it which
game, and sits quietly until you quit. The in-game shop and the Token work
too, which took a while to get right.

**Tells you what's going on.** News from Wowhead, Icy Veins and Blizzard,
filtered to the game you're actually looking at.

**Stays out of your way.** Native Wayland, software-light, no background
service, no tray daemon, nothing running when the launcher is closed.

## What it deliberately doesn't do

Worth being straight about, because some of these are permanent:

- **No credential handling of any kind.** You sign in to Battle.net yourself,
  in Blizzard's own window. Lightforge has no SSO path, no token storage and
  no interest in acquiring one.
- **It never touches the running game.** No memory reads, no injection, no
  process internals. Lightforge manages the prefix, the files and the process
  *around* `Wow.exe`, never inside it. That line is non-negotiable, and it is
  the reason the project can exist at all.
- **Nothing is sent anywhere.** No analytics, no crash pings, no phoning home.
  There's a bug reporter, and it sends exactly once, when you fill it in and
  press Send; it shows you the whole payload first. There is no panic hook.
- **It's WoW-first.** Diablo, Overwatch, Hearthstone and friends appear as
  tiles and hand off to Battle.net, which installs and runs them. Lightforge
  doesn't pretend to manage games it isn't built for.
- **Linux only.** No Windows build, now or later.

## How it was built

This was written by me with [Claude Code](https://claude.com/claude-code), and
I'd rather say so in the README than have anyone wonder. "Written with an AI
assistant" covers two very different working methods, so here is which one:

The architecture is mine. Every real fork was put to me as a trade-off and
decided by me — one shared prefix rather than one per product, a synchronous
core with the network at its edge, the product catalogue in a hosted manifest
instead of the binary, libraries identified by a marker file rather than by
their path. The protocol work was settled against real CDN responses rather
than against what a model believed about TACT, and repeatedly the belief was
wrong: the `max_gap` default is a measured sweep, and the three 32-bit
remedies that have never been run on a real machine carry `// UNVERIFIED`
markers saying so. Nothing was committed unread. A reverted writer-thread
rewrite is still recorded in the notes so nobody tries it again.

What you should take from that: judge it on the 775 tests, on the 123 GiB
install that Battle.net adopted, and on the code. All three are in front of
you. [TECHDOC.md](TECHDOC.md) goes further into both the pipeline and the
method.

## Install

Grab a package from the
[releases page](https://github.com/LightforgeProject/lightforge/releases):

```bash
sudo apt install ./lightforge_1.0.0.beta.4-1_amd64.deb     # Debian, Ubuntu
sudo dnf install ./lightforge-1.0.0.beta.4-1.x86_64.rpm    # Fedora, openSUSE
```

On SteamOS, Bazzite and other systems, download `lightforge-installer.desktop`
from the same page and double-click it, or run it from a terminal:

```bash
curl -fsSL https://raw.githubusercontent.com/LightforgeProject/lightforge/main/packaging/installer/install.sh | bash
```

On Arch there's a PKGBUILD in the repository. The AUR has registrations
closed, so there's no account to upload it from yet:

```bash
cd packaging/aur/lightforge-bin && makepkg -si
```

Or build it yourself, which takes one command and a couple of minutes:

```bash
git clone https://github.com/LightforgeProject/lightforge.git
cd lightforge
cargo build --release --locked

./target/release/lightforge-gui
```

[**INSTALL.md**](INSTALL.md) covers the rest: the installer in more detail,
the Steam Deck, what you need per distro, the 32-bit package Battle.net's
installer quietly depends on, and how to remove it again.

## Requirements

| | |
| --- | --- |
| **OS** | Any current Linux. glibc 2.36+ for release binaries; source builds don't care. |
| **GPU** | Vulkan if you have it (Mesa, NVIDIA); it falls back to software rendering on its own if you don't. |
| **Also** | `fontconfig`, and a 64-bit Rust 1.90+ toolchain if you're building. |
| **Optional** | A 32-bit userspace (`multilib` / i686 libraries). **Only** needed for Battle.net's installer, which is the single 32-bit binary in the whole flow. WoW alone doesn't need it. |

Proton-GE is downloaded and verified by Lightforge itself. You don't need
Steam, Lutris, or a system Wine.

## First run

A short wizard asks where games should live, picks your region and language,
**finds Blizzard games already on this machine and adopts them where they
are** (nothing gets moved or re-downloaded), offers to set Battle.net up, and
fetches the artwork so the first screen isn't empty.

Then it's one button.

## Where things go (with the default settings)

```text
~/Games/Blizzard/
├── prefix/          the Wine prefix — disposable, rebuild it any time
└── games/           your actual game files, safe from prefix wipes

~/.config/lightforge/       settings
~/.local/share/lightforge/  Proton builds, addon ledger
~/.cache/lightforge/        artwork, news, catalogues
```

Game data lives outside the prefix on purpose: you can throw the prefix away
and rebuild it without touching 120 GB of downloads.

### …or on any other drive

Games don't have to live in your home directory. Add a partition under
**Settings › Storage** and Lightforge claims a `Lightforge/games/` folder on
it; from then on every install asks which drive it should go to, and one of
them is the default that Battle.net installs its own games to as well.

```text
/mnt/nvme2/Lightforge/games/World of Warcraft/     a second library
```

The prefix always stays on your home drive: it's small, it's disposable, and
everything expects it there. Only the games move.

A library is identified by a marker file on the volume itself, not by its
path, so Lightforge can tell "this drive isn't mounted" apart from "this
folder is empty". That distinction is the difference between *"nvme2 is not
mounted — the game is still on it, untouched"* and quietly re-downloading
120 GB onto your system disk.

## There's a CLI too

Everything the GUI does, scriptable, plus a pile of things it doesn't: raw CDN
inspection, CASC index dumps, build-config diffs.

Forty-five commands, all of them in **[docs/CLI.md](docs/CLI.md)**.

## This is a beta

It works, and it's been through full installs of Retail and Classic Era,
including the 123 GiB retail install mentioned up top. But it's had one pair
of eyes on it, on one machine. Expect rough edges on hardware and distros it
hasn't met yet, and please tell me when you find them: the in-app reporter
(`☰ › Report a bug`) collects the useful details for you.

Hit something odd? [**TROUBLESHOOTING.md**](TROUBLESHOOTING.md) covers the
failures I already know about: the 32-bit package Battle.net's installer
needs, a drive that isn't mounted, a store that wants a repair.

See [RELEASES.md](RELEASES.md) for what's new.

## What's next

No dates, and no promises — this is one person's spare time. Roughly in the
order I expect to get to them:

**Soon**

- **A Flatpak**, and the AUR the moment registrations there reopen. The
  `.deb`, the `.rpm` and the Arch PKGBUILDs are done.
- **Steam Deck.** The Flatpak is most of the answer; the glibc floor that used
  to block it no longer does.
- **Move a game to another drive** without re-downloading it.

**After that**

- **Pre-downloads.** Blizzard publishes a patch before it goes live, and the
  launcher should stage it so release day is a few minutes instead of an
  evening. The channel for it is already understood; it needs a real
  pre-download window to be built against.
- **A MangoHud panel**, to choose what the overlay shows instead of editing a
  config by hand.
- **Steam integration**: add your games to Steam's library while still
  launching them the Lightforge way.
- **Subscription status** on the Play page, if there's an honest way to ask
  for it.
- **Hearthstone's companion app**, installed and launched from its tile.

**Being thought about**

- Alternate and private servers, which the product model already allows for.
- Moving or removing the games Battle.net installed, rather than only
  installing them.

Something missing? Say so in [Discord](https://discord.gg/vtTgmUV9Bq). The
list is short enough that one good argument can move things up it.

## Support the project

Lightforge is free, GPL-3.0, and built in my spare time. If it saved you an
evening of fighting Battle.net, you can buy me a coffee:

[![Ko-fi](https://img.shields.io/badge/Ko--fi-support-FF5E5B?style=for-the-badge&logo=ko-fi&logoColor=white)](https://ko-fi.com/simplykenny)
[![PayPal](https://img.shields.io/badge/PayPal-support-003087?style=for-the-badge&logo=paypal&logoColor=white)](https://paypal.me/simplykenny)

Telling someone about it works too.

## Under the hood

[**TECHDOC.md**](TECHDOC.md) is the technical documentation: the TACT/CASC
pipeline, how a plan avoids 1.57 million range requests, why the store has to
be byte-identical to Agent's, the prefix layout, and the rest of it. It also
says where the project came from and how it was built.

## Everything else

| | |
| --- | --- |
| [INSTALL.md](INSTALL.md) | Building it, what each distro needs, and how to remove it |
| [TROUBLESHOOTING.md](TROUBLESHOOTING.md) | The failures I already know about, and what to do about them |
| [docs/CLI.md](docs/CLI.md) | All forty-five commands, generated from the binary |
| [TECHDOC.md](TECHDOC.md) | How it works underneath |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Reporting bugs, sending patches, the house rules |
| [SECURITY.md](SECURITY.md) | Reporting a vulnerability, and how downloads are trusted |
| [DISCLAIMER.md](DISCLAIMER.md) | Blizzard, the guardrail, and what leaves your machine |
| [RELEASES.md](RELEASES.md) | What's in each version |

## Community

- [Discord](https://discord.gg/vtTgmUV9Bq) — help, bugs, and what's next
- [@simplyKennybtw](https://x.com/simplyKennybtw)

## Legal

Lightforge is an independent, unofficial project. It is **not affiliated with,
endorsed by, or sponsored by Blizzard Entertainment**. World of Warcraft,
Battle.net and Blizzard Entertainment are trademarks of Blizzard
Entertainment, Inc.

Lightforge ships no game content. It downloads from Blizzard's own CDN, using
your own account and your own subscription, the same way their client does.

[**DISCLAIMER.md**](DISCLAIMER.md) has the rest: what the launcher touches and
what it will never touch, an honest word about risk, and a list of every host
it talks to and what each one sees.

## License

[GPL-3.0](LICENSE). Do what you like with it; keep it open.

# Disclaimer

The boring-but-important page. I've tried to write it the way I'd explain it
to someone in Discord rather than the way a lawyer would, and nothing here is
meant to be taken on trust — the source is right there, and I've pointed at
the relevant files wherever it helps.

## Lightforge is not a Blizzard product

It's an independent, unofficial project with no connection to Blizzard
Entertainment. Not endorsed by them, not sponsored by them, not reviewed by
them. They don't know I exist.

World of Warcraft, Battle.net, Hearthstone, Diablo, Overwatch, StarCraft and
Warcraft are trademarks of Blizzard Entertainment, Inc. They appear here
because this is software for running their games, and for no other reason.

## It doesn't give you anything you don't already own

Lightforge contains no Blizzard game files and redistributes none. What it
does is talk to Blizzard's own content servers — the same hosts, the same
protocol, the same files the official client fetches — using **your** account
and **your** subscription.

No account, no game time, no game. Lightforge doesn't provide any of those and
doesn't get around needing them. If you can't download WoW with Blizzard's
launcher, you can't download it with mine either.

## What it touches, and what it will never touch

This is the part I care most about getting right, because it's the first thing
anyone sensible asks about a third-party launcher for an online game.

**Lightforge works entirely outside the game.** It manages:

- the Wine prefix the game runs in,
- files on your disk — downloads, config files, addon folders,
- the lifecycle of the process it starts, and stale ones left behind.

**It never goes inside the game.** No reading or writing `Wow.exe`'s memory,
no injection, no patching the executable, no automating anything you'd
otherwise do with your hands. Not "we don't currently" — this is a design rule
the project is built around, and a feature that needed to cross it wouldn't
get written.

**It never handles your credentials.** There is no single-sign-on path, no
token storage, no interception of the login flow, nothing that touches a
password. You sign in to Battle.net in Blizzard's own window, exactly as you
would without me. Lightforge couldn't log in as you if it wanted to; it has
nothing to log in with.

It does read one thing Battle.net leaves on disk: the client's cached copy of
Blizzard's public game catalogue, for game names, which tile a game belongs to,
and a pre-install's release date. It skips anything in it that depends on your
account, such as which games you own.

## The honest bit about risk

I'm not going to tell you that you can't get banned for using this, because
nobody can promise that about any third-party software, and anyone who does is
either guessing or selling something.

What I *can* tell you is specifically what the software does, which is
everything above: it manages files and processes around the game and nothing
within it. Your relationship with Blizzard is governed by their terms of
service, not by mine. Choosing to use a third-party launcher is your call, and
the risk — whatever you judge it to be — is yours.

And since the licence says it in legal language, here it is in plain words:
**this software comes with no warranty of any kind** (GPL-3.0, sections 15 and
16). It moves hundreds of gigabytes and deletes files when you ask it to. It's
beta software written by one person. Keep backups of anything you'd be upset
to lose.

## What leaves your machine

Short version: **nothing about you, unless you press Send on a bug report.**

There's no analytics, no usage tracking and no crash pings. There isn't even a
panic handler that could send something without you — reports exist only
because somebody filled in a form.

Two requests do go out when the launcher starts, and neither says anything
about you beyond the IP address any request reveals: it fetches the game
artwork it shows you, and it asks GitHub whether a newer Lightforge has been
released. The second one is a public release listing, nothing is downloaded or
installed from it, and Settings › General turns it off. Both are in the table
below.

The longer version, because "we respect your privacy" is what everyone says
and a list is more useful:

| Who it talks to | When | What they see |
| --- | --- | --- |
| `*.version.battle.net`, `level3.blizzard.com` | installing, updating, checking for patches | Blizzard's own CDN, same as their client |
| `lightforge.tuxcore.pro` | artwork, news, addon search | your IP address, and what you searched for |
| `api.mmoui.com`, `cdn.wowinterface.com`, `api.tukui.org`, `api.github.com`, `edge.forgecdn.net` | installing or updating addons | your IP address, by those providers |
| `wow.zamimg.com` | news headlines with pictures | your IP address, by Wowhead's CDN |
| `api.github.com` | checking for new Proton-GE builds | a public release listing |
| `api.github.com` | once at startup, checking whether a newer Lightforge was released | a public release listing |
| `bugs.k3nny.tech` | **only when you press Send** | exactly the report shown to you first |

About that last row, since it's the only one that carries anything personal:

- The report shows you its whole contents before it goes anywhere — your
  message, the recent log lines, your GPU and driver, the launcher version,
  and the screenshot if you attached one.
- Your IP is not recorded with it (`send_default_pii = false`), and your
  machine's hostname is replaced with `redacted` before sending — desktop
  hostnames are usually somebody's actual name.
- A screenshot you attach is uploaded to `lightforge.tuxcore.pro` and the
  report carries a link to it. Have a look at what's on your screen first.
- Sending the same report twice in five minutes is refused, and if it can't
  be sent at all it's copied to your clipboard instead of being queued
  somewhere.

If you'd rather read this as code than as a table: it's all in
`crates/launcher-gui/src/report.rs` and `crates/core/src/net.rs`.

## Other people's work

**Proton-GE** is downloaded from [GloriousEggroll's
releases](https://github.com/GloriousEggroll/proton-ge-custom) and checked
against the sha512 published alongside it before anything is unpacked. It's
his project under his licence; Lightforge just fetches and manages it.

**Addons** come from CurseForge, WoWInterface, Tukui and GitHub under their own
terms. I don't host addon code, I don't review it, and I can't vouch for it —
an addon is someone else's program running inside your game. Where an author
has asked not to be distributed by third-party tools, Lightforge says so and
sends you to their page instead of downloading it anyway.

**The icons** in the launcher are [Lucide](https://lucide.dev) (ISC) and
[Simple Icons](https://simpleicons.org) (CC0), listed file by file in
`crates/launcher-gui/assets/README.md`. The Discord and X marks are links to
those services and nothing more — neither company endorses this.

## Questions, or something you want taken down

Come and say so: **[discord.gg/vtTgmUV9Bq](https://discord.gg/vtTgmUV9Bq)**.
That goes for anything on this page — if you represent a rightsholder and
something here is wrong, I'd much rather hear it directly and fix it than find
out some other way.

---

Lightforge is [GPL-3.0](LICENSE). See the [README](README.md) for what it
actually does.

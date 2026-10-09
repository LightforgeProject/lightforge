# Security

## Reporting a vulnerability

**Please don't open a public issue for a security problem.**

Message me privately on **[Discord](https://discord.gg/vtTgmUV9Bq)** — join
and DM me rather than posting in a channel. If you're reading this on a mirror
that supports private security advisories, that works too.

Useful things to include, roughly in order of how much they help:

- what an attacker can actually do with it,
- the steps to reproduce, and whether it needs local access,
- the version (`lightforge --version`) and your distribution,
- a patch, if you have one.

I'm one person doing this in my spare time. I'll acknowledge within a few
days, and I'd rather hear about something half-confirmed than not hear about
it — a false alarm costs me an hour, the other way round costs everyone else
more.

If you want credit when it's fixed, say so and you'll get it.

## What's supported

Lightforge is in beta, so: **the latest release, and nothing else.** There are
no backports to older versions — the fix goes into the next build. That's also
the honest reason to use a distribution package once they exist, because
there's no self-updater yet.

## In scope

- The launcher and the CLI.
- The services they talk to, which I run: the asset host, the addon proxy and
  the screenshot endpoint behind `lightforge.tuxcore.pro`.
- Anything that lets a third party put code or files on a user's machine
  through Lightforge.

## Out of scope

- **Blizzard's own services and terms.** If you've found something in their
  infrastructure, they have their own disclosure process and it isn't me.
- **Addon code.** Addons are other people's programs, downloaded from their
  authors' chosen hosts. Lightforge doesn't review them and can't vouch for
  them; a malicious addon is a matter for its provider.
- **Upstream projects** — Proton, Wine, Mesa, the Rust crates in `Cargo.lock`.
  Report those to them, though I'd still like to know if Lightforge makes an
  upstream issue meaningfully worse.
- **Anything needing an already-compromised machine.** If something else is
  already running as your user, it doesn't need Lightforge's help.

## Things that look like bugs and aren't

Worth stating so nobody spends an evening on them:

- **The addon proxy is unauthenticated on purpose.** Anyone who knows the URL
  can use it. It's rate-limited per IP and heavily cached, and it holds no
  user data — the trade-off is deliberate, and the API key behind it never
  reaches the client.
- **Nothing is sent without being asked for.** No telemetry, no crash
  reporting, no panic hook. A bug report goes out only when somebody fills in
  the dialog and presses Send, and the dialog shows the whole payload first.
  See [DISCLAIMER.md](DISCLAIMER.md#what-leaves-your-machine).
- **Lightforge never handles credentials.** There's no login path, no token
  storage, no interception of Battle.net's sign-in. There is nothing in the
  process to steal.

## How downloads are trusted

Since "a launcher that downloads and runs things" is the obvious place to
look, here's the model, and it doesn't depend on the network being honest:

- **Game data is content-addressed.** Every file Blizzard serves is named by
  the hash of its own bytes, and Lightforge checks that hash before the file
  counts as present. TLS stops an observer steering the launcher at a
  different *valid* build; the hash check is what stops arbitrary content.
- **Proton-GE is checked against the sha512 published with it** before
  anything is unpacked.
- **Addon packages are verified where the provider publishes a digest**, and
  the archive is only extracted after that. Where no digest is published, the
  download is over TLS from the provider's own host.
- **Version and CDN metadata is fetched over TLS**, not the plain-HTTP port
  Blizzard's older protocol used.

If you can break one of those, that's exactly the report I want.

## No warranty

Lightforge is [GPL-3.0](LICENSE) and comes with no warranty — sections 15 and
16 say it in the legal wording. Doing my best is a commitment I can make; a
guarantee isn't.

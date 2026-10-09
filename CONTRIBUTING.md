# Contributing

Thanks for looking. Lightforge is a one-person project, so anything from a
bug report to a patch is genuinely useful.

The fastest way to reach me is **[Discord](https://discord.gg/vtTgmUV9Bq)**.

## Reporting a bug

Use the built-in reporter — **☰ › Report a bug**. It collects the version,
your GPU and driver, the session type and the last things the launcher did,
and you can drop a screenshot onto the window. It shows you everything before
it sends anything.

If the launcher won't start at all, Discord is the place, with whatever it
printed in the terminal.

[TROUBLESHOOTING.md](TROUBLESHOOTING.md) covers the failures I already know
about — worth a look first.

## Suggesting something

Open an issue or say it in Discord. Worth knowing what the project is not
trying to be: Lightforge is WoW-first, and other Blizzard games are a
pass-through to Battle.net on purpose. Ideas that turn it into a general game
manager are probably not going to happen.

## Working on the code

```bash
git clone https://git.tuxcore.pro/kenny/lightforge.git
cd lightforge
cargo build
cargo test
```

[INSTALL.md](INSTALL.md) has the prerequisites — there are almost none.

Before you open a pull request:

```bash
cargo test
cargo clippy --all-targets
```

Tests must pass, and clippy should report nothing new — there is one
long-standing warning about a function with eight arguments, and that is the
only one you should see. If you touched a CLI command, also run `make cli-doc`
so `docs/CLI.md` matches the binary.

## Pull requests

**Say something first** if it's more than a small fix — an issue or a message
in Discord. Not for ceremony: it's a one-person project with opinions, and I'd
rather talk for five minutes than have you write something I then have to
argue with.

Then the usual:

- Branch off `main`, one change per pull request. Two unrelated fixes in one
  branch means neither can be merged until both are right.
- Say what it does and why in the description, and how you checked it. "Ran
  the suite" is fine; "installed Classic Era on a second drive and it worked"
  is better.
- Keep the commit messages in the style above. No need to squash — I'd rather
  read five honest commits than one invented one.
- Expect questions about *why* rather than about style. If a change is
  surprising, the explanation belongs in a comment, not only in the pull
  request, because the comment is what the next person reads.

I'm one person in a timezone, so give me a few days before assuming I've
ignored you. Poke me in Discord if I have.

**Small things are very welcome.** Typos, wording, a doc that's wrong. One in
particular: [INSTALL.md](INSTALL.md) lists the 32-bit package needed for
Battle.net's installer on four distributions, and **only the Fedora line has
been tested on a real machine**. If you run Debian, Arch or openSUSE and can
confirm or correct yours, that's a genuinely valuable patch.

Note on where: the repository lives on my own Forgejo at
`git.tuxcore.pro`, which needs an account to open a pull request against. If
that's a hassle, send a patch or a branch link in Discord and I'll take it
from there.

## House rules

**Don't run `cargo fmt`.** There's no `rustfmt.toml` and the tree is formatted
by hand. One `fmt` run rewrites about 12,500 lines across 60 files and buries
whatever you actually changed. Match the style of the code around you instead.

**Write tests for behaviour, not for coverage.** The suite is there to catch
real regressions; a test that restates the implementation is noise.

**Comments explain why, not what.** If a line is surprising, say what made it
that way — a measurement, a quirk of Blizzard's format, a bug it prevents.

**Commit messages** follow `type: short summary`, lowercase, in the
imperative: `feat:`, `fix:`, `perf:`, `refactor:`, `chore:`, `docs:`.

## One hard limit

Lightforge manages the prefix, the files and the process **around** the game,
never anything inside it. No reading or writing the game's memory, no
injection, no patching the client, no automation of play, and nothing that
touches credentials or authentication tokens.

This isn't a preference, it's the line the project exists on the right side
of. A pull request that crosses it won't be merged, however good it is —
please ask before writing one if you're unsure. [DISCLAIMER.md](DISCLAIMER.md)
explains the reasoning.

## Licence

Lightforge is [GPL-3.0](LICENSE). By contributing you agree your work ships
under the same licence.

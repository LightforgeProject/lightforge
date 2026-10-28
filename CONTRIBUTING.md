# Contributing

Thanks for looking. Lightforge is a one-person project, so bug reports and
patches both help a lot.

The fastest way to reach me is **[Discord](https://discord.gg/vtTgmUV9Bq)**.

## Reporting a bug

Use the built-in reporter: **☰ › Report a bug**. It collects the version,
your GPU and driver, the session type and the last things the launcher did,
and you can drop a screenshot onto the window. It shows you everything before
it sends anything.

If the launcher won't start at all, post in Discord with whatever it printed
in the terminal.

[TROUBLESHOOTING.md](TROUBLESHOOTING.md) covers the problems I already know
about, so have a look there first.

## Suggesting something

Open an issue or post it in Discord. One thing to know first: Lightforge is
WoW-first, and other Blizzard games are handed to Battle.net. I'm unlikely to
take ideas that turn it into a general game manager.

## Working on the code

```bash
git clone https://git.tuxcore.pro/kenny/lightforge.git
cd lightforge
cargo build
cargo test
```

[INSTALL.md](INSTALL.md) lists the prerequisites. There are very few.

Before you open a pull request:

```bash
cargo test
cargo clippy --all-targets
```

Tests must pass, and clippy should report nothing. If you changed a CLI
command, also run `make cli-doc` so `docs/CLI.md` matches the binary.

## Fuzzing the parsers

Every parser of bytes from Blizzard's CDN, a CASC store, Battle.net's
`product.db` or Steam has a target in `fuzz/`. They need a nightly toolchain
and `cargo-fuzz`, and do not run in CI:

```bash
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz --locked
fuzz/seed.sh "$HOME/Games/Blizzard/games/World of Warcraft"
cd fuzz
cargo +nightly fuzz list
cargo +nightly fuzz run -O idx -- -max_total_time=600 -rss_limit_mb=2048
```

`seed.sh` copies a few real files from an install into `fuzz/corpus/`, which
stays out of git. Formats that only live inside the archives (encoding,
manifests, BLTE) start from their magic bytes. A crash lands in
`fuzz/artifacts/<target>/`. Fix it, add a unit test for the input that
caused it, and run the target again.

## Pull requests

**Talk to me first** if it's more than a small fix, in an issue or on
Discord. I have opinions about how this project should work, and a short chat
up front saves us both from a pull request I can't merge.

Then:

- Branch off `dev`, one change per pull request. `main` is the last release.
  Two unrelated fixes in one branch can't be merged until both are ready.
- Say in the description what it does, why, and how you tested it. "Ran the
  tests" is fine; "installed Classic Era on a second drive and it worked" is
  better.
- Use the commit style below. You don't need to squash; several real commits
  are easier for me to review than one combined one.
- Expect questions about *why*, more than about style. If a change is
  surprising, explain it in a code comment as well as in the pull request,
  since the comment is what the next person reads.

I'm one person, so give me a few days before you assume I've forgotten you.
Poke me in Discord if I have.

**Small things are very welcome**: typos, wording, a doc that's wrong. One in
particular: [INSTALL.md](INSTALL.md) lists the 32-bit package Battle.net's
installer needs on four distributions, and **only the Fedora line has been
tested on a real machine**. If you run Debian, Arch or openSUSE and can
confirm or correct yours, that helps a lot.

The repository lives on my own Forgejo at `git.tuxcore.pro`, and you need an
account there to open a pull request. If that's a hassle, send a patch or a
branch link in Discord and I'll take it from there.

## House rules

**Don't run `cargo fmt`.** There's no `rustfmt.toml` and the code is formatted
by hand. One `fmt` run changes about 12,500 lines across 60 files and hides
your actual change. Match the style of the code around you.

**Test behaviour.** Tests are there to catch real regressions. A test that
only repeats the implementation doesn't help.

**Comments explain why.** If a line is surprising, say what made it that way:
a measurement, a quirk of Blizzard's format, or the bug it prevents.

**Commit messages** follow `type: short summary`, lowercase, in the
imperative: `feat:`, `fix:`, `perf:`, `refactor:`, `chore:`, `docs:`.

## One hard limit

Lightforge manages the prefix, the files and the process **around** the game,
never anything inside it. No reading or writing the game's memory, no
injection, no patching the client, no automated play, and nothing that
touches credentials or authentication tokens.

I won't merge a pull request that crosses this line, no matter how good it is.
If you're not sure, ask before you start. [DISCLAIMER.md](DISCLAIMER.md)
explains why.

## Licence

Lightforge is [GPL-3.0](LICENSE). By contributing you agree your work ships
under the same licence.

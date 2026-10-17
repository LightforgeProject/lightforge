# Lightforge — technical documentation

How the launcher works underneath: the formats it speaks, the decisions that
aren't obvious from reading a function, and where to look when you need the
detail.

This is the middle layer. Above it is [README.md](README.md), which says what
the thing does; below it are the module doc comments, which are where the real
specifics live — most modules open with several paragraphs explaining why they
are the shape they are, and this page will send you to them rather than
repeat them.

## Where it came from

The idea is from the beginning of 2026: a Linux launcher for WoW that didn't
depend on Battle.net. For months the part that mattered — downloading and
patching the game yourself — looked closed. It isn't closed, it's just
undocumented, and in **August 2026** I worked out how Blizzard's distribution
pipeline actually fits together: the version service, the manifests, the
archives, and the local storage format they land in. Once that was
understood, the rest was work rather than guessing.

This repository starts on **16 September 2026**, and it is a rebuild rather
than a beginning. What you are reading documents the state about a month
later, at `1.0.0-beta.2`.

### How it was built

This is the second implementation. The first one existed before any of this
repository did, and its real output was not a working product — it was understanding what
the problem actually was. Rebuilding from scratch in September, knowing that,
is why the architecture here is settled rather than discovered: the prefix
layout, the dual directory, the shape of the download path and the decision
to keep the core synchronous were all things the prototype had already argued
for or against.

The rebuild was done by me with the help of  [Claude
Code](https://claude.com/claude-code), which I'd rather state plainly than
have anyone wonder about. It is worth being precise about what that means,
because "written with an AI assistant" covers two completely different
working methods and only one of them produces a codebase you can maintain.

What the assistant did was type, quickly, against decisions that were already
made — and argue back, which is most of its value. What it did not do:

- **Decide the architecture.** Every real fork was put to me as a trade-off
  with a recommendation, and decided by me: one shared prefix rather than one
  per product, a synchronous core with the network at its edge, the product
  catalogue in a hosted manifest instead of the binary, libraries identified
  by a marker file rather than by their path, per-game Proton pinning,
  shared-rather-than-per-game optimization profiles for Battle.net titles.
  Those choices are why the code is shaped the way it is.
- **Decide what was true.** The protocol work was settled against real CDN
  responses, not against what a model believed about TACT — and repeatedly
  the belief was wrong. The `max_gap` default above is a measured sweep. The
  32-bit remedies in `runtime.rs` carry `// UNVERIFIED` markers on the three
  that have never been run on a real machine, because reasoning about distro
  layouts produced two wrong answers before a VM caught them.
- **Get the last word.** Nothing was committed unread, and plenty was sent
  back or thrown away. A reverted writer-thread rewrite is still recorded in
  the notes precisely so nobody tries it again.

The pattern you'll find throughout the comments — a measurement, a rejected
alternative, a note saying which parts are verified and which are reasoned —
is the method, not decoration. Where this document explains something
oddly specifically, it is usually because the first and entirely reasonable
version of that code was wrong, and something real said so.

## The shape of it

A Cargo workspace, three crates, one direction of dependency:

| Crate | Lines | What it is |
| --- | --- | --- |
| `lightforge-core` | ~34,000 | All the domain logic. Knows nothing about any UI. |
| `lightforge-gui` | ~19,000 | The iced application. |
| `lightforge-cli` | ~3,000 | A thin `clap` front end over the same core. |

Both front ends consume `core`; `core` depends on neither, and never on iced
or clap. That rule is what makes the CLI possible at all, and the CLI in turn
is what made most of this debuggable — a protocol bug is far easier to chase
from a terminal than through a UI.

**`core` is synchronous except at the network edge.** Everything that touches
HTTP lives behind `async fn` in `net.rs` and the modules that call it;
everything else is ordinary blocking code that a test can call directly. The
GUI pushes the async parts onto tokio via `tasks.rs`, and blocking work onto
`spawn_blocking`, so the render thread never waits on either.

**Errors**: each module owns a `thiserror` enum for what can genuinely go
wrong in it (`tact::download::DownloadError`, `library::LibraryError`, …), and
`error::CoreError` aggregates them at the crate boundary. Orchestration in the
CLI and GUI wraps those in `anyhow`. Error text is user-facing — it ends up in
a status line — so it reads as a sentence, not as a type name.

**TLS is rustls**, not the system OpenSSL, so behaviour doesn't drift with a
distribution's crypto policy.

## On-disk layout

Everything resolves paths through `paths::Layout`, so the structure lives in
one place rather than being rebuilt from string joins all over the tree:

```text
~/Games/Blizzard/
├── prefix/                        Proton compat-data dir (disposable)
│   └── pfx/                       the Wine prefix itself
│       └── drive_c/
│           └── Program Files (x86)/
│               ├── Battle.net/
│               └── World of Warcraft/   -> symlink into games/
└── games/
    └── World of Warcraft/         the real thing: Data/, .build.info, _retail_/
```

The split is not tidiness. Game files live on the native filesystem and are
**symlinked** into the prefix, so the prefix stays disposable — you can delete
and rebuild it without touching 120 GB of downloads. And because the game then
executes from a path Wine believes is a normal Windows location, the in-game
Chromium (the shop, the Token) stops rendering a grey rectangle.

Lightforge's own data is XDG, and deliberately not under the games root:

```text
~/.config/lightforge/config.toml      settings, written atomically
~/.local/share/lightforge/            Proton builds, the addon ledger, the queue
~/.cache/lightforge/                  artwork, news, catalogues — disposable
```

## Getting a game: TACT / NGDP

`tact/` is the biggest subsystem and the reason the project exists. Blizzard's
distribution pipeline, reimplemented well enough that Agent cannot tell the
difference.

### Asking what exists

```text
https://<region>.version.battle.net/<product>/versions
https://<region>.version.battle.net/<product>/cdns
https://<region>.version.battle.net/<product>/bgdl      (pre-downloads)
```

All three return **BPSV** — the same pipe-delimited table grammar as
`.build.info`. `tact/version.rs` parses it directly rather than using
`ribbit-client` or `tact-client`: the former gets HTTP 400 from Blizzard's TCP
endpoint on both protocol versions, and the latter's V2 path rejects a
response the server returns as 200. There is also **no product listing** —
the `summary` endpoint 404s — so enumerating products means probing a list of
candidate codes.

A `versions` row names two hashes that matter: the **build config** and the
**CDN config**.

### What a build is made of

```text
build config ──┬── encoding manifest    content key → encoding key
               ├── download manifest    which files this install wants
               └── install manifest     the loose files (Wow.exe, DLLs, CEF)

CDN config ────── archive list          ~1400 archives, each with an index
```

- **`tact/encoding.rs`** — the content-key/encoding-key map. Its page-table
  interleaving is the thing third-party parsers most often get wrong; getting
  it wrong costs you 95% of the manifest.
- **`tact/manifest.rs`** — the download manifest decides *which* files this
  machine wants, by tag expression: platform, locale, and optional categories.
  Tags are the mechanism behind "my download is 37 GiB bigger than expected" —
  some categories are unnamed in the manifest and must be gated by discovery,
  never by a hardcoded list.
- **`tact/archive.rs` / `tact/indices.rs`** — archive indices say which
  archive holds a key and at what offset. They are cached locally, because
  nothing can be *located* without them.

### The plan

The single most important piece of engineering in the downloader, in
`tact/plan.rs`. A retail install's wanted files are scattered across ~1400
archives:

- one range request per file → **1.57 million requests**;
- whole archives instead → **174.5 GiB transferred for 123.0 GiB wanted**.

Neither is acceptable, so the plan sits between them: locate every wanted key,
sort by `(archive, offset)`, merge neighbours closer than `max_gap` into a
**run** capped at `max_run`, then fetch each run with one range request and
split it locally. Concurrency is 32 by default — measured at 2.4× the
throughput of 8, because these transfers are latency-bound rather than
bandwidth-bound.

`tact/explain.rs` exists to answer "why is this plan that size?", which is a
question you will have.

### Writing the store

`tact/writer.rs` lays bytes out the way Agent would, and the bar is
*byte-identical*, because anything else makes Agent decide the install is
damaged and start the repair scan this project exists to avoid. Three details
are load-bearing and none are guessable:

1. Every archive reserves `idx::ARCHIVE_RESERVED` bytes at its head, and each
   bucket's index records a marker pointing at its own slot there.
2. Every file carries a **30-byte entry header** (`tact/entry.rs`) whose
   second checksum mixes in the archive number *and the offset it is written
   at* — so a payload is only valid at the place it was written.
3. The `.idx` files are V7 (`tact/idx.rs`), with two different Jenkins hashes
   and headerless aliases.

Content is written **BLTE-encoded, undecoded**. That matters more than it
sounds: about 2% of a build is encrypted with keys Blizzard hasn't published
yet, and storing the encoded bytes verbatim means unreleased content is
handled by doing nothing special at all.

### Resume, verify, repair, reclaim

- **Resume** is a property of the plan: what the store already holds is
  subtracted before anything is fetched, so an interrupted download continues
  rather than restarting.
- **`.build.info` is the completion marker.** It's written last and only on a
  genuinely complete run (`install.rs` enumerates every way a run can fall
  short), so a half-finished install reads as `Install` rather than offering
  `Play` on a broken game.
- **Verify, two depths** (`tact/verify.rs`). The fast pass reads each file's
  30-byte header — about 55 MB of reads across a 140 GB store, sorted to be
  sequential. The deep pass hashes every stored byte, and does it *without
  decoding anything*: a BLTE file's encoding key is the MD5 of its chunk
  table, and the table holds an MD5 of every chunk as stored, so raw bytes
  verify the whole structure with no zlib and no decryption keys. Both are
  safe while the game runs; a `Snapshot` taken before and compared after turns
  "someone else wrote to this archive" into an error rather than a false
  damage report.
- **Repair is not a separate mechanism.** A damaged entry is dropped from the
  index, the planner then sees the file as absent, and the next download
  fetches exactly it.
- **Reclaim** (`tact/reclaim.rs`) gives back the space of one version in a
  store shared by several. The keep-set comes from the builds that are
  *installed*, read from `.build.info` — never from what the CDN serves today,
  because a version a patch behind still needs files its newer build dropped.

## Telling Battle.net about an install

Two files, both someone else's format.

**`.build.info`** (`buildinfo.rs`) — BPSV, one row per product, shared between
Retail, Classic and the PTRs. Two rules: columns are located by name rather
than by index, because Blizzard reorders them across expansions; and only the
target product's row is ever mutated, because a whole-file rewrite would
clobber its siblings. Writing is atomic — temp file and rename — since Agent
memory-maps this file and a torn write is exactly the corruption we're
avoiding.

**`.product.db`** (`tact/productdb.rs`) — Agent's protobuf-ish registry of
what is installed where. Without it, Battle.net answers error 2800 and refuses
to adopt an install. Lightforge writes it from `.build.info`, so the two
cannot disagree, and merges a record into Agent's own registry so the client's
library lists the game without the user triggering a scan.

Reading it back has one trap worth knowing, since it cost a real bug: a record
carries **two** names — field 1 is the client's install key, field 2 is the
TACT product code. They are the same string for every product except
Hearthstone, which is `hs_beta` to Battle.net and `hsb` to the CDN. Index by
only one and an installed game reads as "not installed" forever.

`discover.rs` uses the same format in reverse: it finds installs that predate
Lightforge by reading the `product.db` of *other* launchers' prefixes —
Lutris, Bottles, Heroic, a hand-made `~/.wine` — and can lift a record out of
one and graft it into ours as raw bytes, with only the install path rewritten.
That's how a game installed elsewhere becomes playable here without moving a
single file.

## The prefix, Proton, and launching

- **`prefix.rs`** creates the prefix by running Proton itself (`wineboot`),
  then puts the POSIX symlink in place. Drive letters are assigned per
  library, two per library, because the in-game browser needs the install to
  look like a normal Windows path.
- **`proton.rs`** models Proton-GE releases, polls GitHub with an ETag so a
  check costs nothing when nothing changed, and verifies each download against
  the **sha512 published alongside it** before unpacking. Builds can be pinned
  per product, including for the games Battle.net starts.
- **`runtime.rs`** answers "can this machine run 32-bit Windows binaries at
  all?" — the question behind the most common first-run failure. It checks for
  a 32-bit loader before Proton is even downloaded, and runs Proton's own
  32-bit wine before the Battle.net step. Neither check can prove the 32-bit
  *graphics* stack is present, which is why the remedy names the distribution's
  32-bit Wine package: its dependency closure is what's actually needed.
- **`preflight.rs`** is everything that must be true before Battle.net is
  spawned: no stale Agent holding a lock, no leftover wineserver, and any
  download writing to the CASC store paused first — two writers in one store
  is a real corruption risk, not a theoretical one.

## Battle.net and the hand-off

`battlenet.rs` installs the client headlessly into the prefix and applies the
registry and config defaults that stop it fighting you.

For games outside the WoW family, Lightforge is a pass-through, and the
interesting part is naming the game correctly. The client works in **game
families** (`WTCG`, `Fen`, `Pro`), not TACT product codes, and
`--exec="launch <uid>"` wants the family's uid. Guessing is worse than
useless: an unrecognised id makes the client fall back to the first family it
knows, which is Warcraft III, landing the user on a game they may not own.

`families.rs` solves it by reading Blizzard's own published catalogue, which
the client caches on disk as JSON fragments:

```text
products[].id                    WTCG      the game family
  └ base.types.<type>.uid        hs_beta   the install key
      └ installs[<uid>].tact_product  hsb   the TACT product code
```

So the mapping is a lookup over files the client already left on disk, not a
table to maintain — and it covers every title Blizzard publishes rather than
the two somebody happened to observe in a log.

The same fragments carry the **pre-install** state. Battle.net's "Pre-Install
… Available October 19, 2026" for the WoW: Forever stress test is a fragment
giving `wowf` a `release_date_message`, and the version picker's "Stress Test"
is the part after the dash in its selector name. There is no boolean anywhere;
a release-date message is the signal. Two rules keep it honest:

- **Only current fragments.** The cache is content-addressed and never pruned,
  so it holds every revision ever fetched (one old copy still calls the
  Forever beta "Mists of Pandaria Beta"). The newest root fragment lists the
  current hash of each, and names and dates come from those alone. Families
  are read from every revision, since they never change.
- **Only conditions that can be decided.** A fragment may be conditional on
  licences, feature flags, platform, country or login region. Only the login
  region is evaluated, from the configured region; anything else is skipped.
  The licence-gated one is why: it gives the Forever beta "Available November
  4" for accounts that do not own Forever, and Lightforge does not look at
  what an account owns.

It is a label and a caption, never a gate. PLAY stays enabled: whether the
servers let you in is decided on Blizzard's side, the cache only refreshes when
the client runs, and starting the game early is how you find out the install
works before the servers open.

## Storage libraries

`library.rs`. A library is one partition's worth of games, shaped like the
default one. Four decisions:

1. **A library is identified by a marker file on the volume**
   (`games/.lightforge-library`), not by its path. An unmounted drive leaves
   an empty directory behind at its mount point, so path alone cannot tell
   "the drive is away" from "the library is empty" — and the cost of that
   confusion is 120 GB downloaded onto the system disk.
2. The config also records the filesystem UUID and a relative path, used
   **only** to find a drive that came back somewhere else.
3. A drive that is away *holds* the game rather than losing it: the state line
   says so, the only action is to look again, and everything that writes
   refuses on its own.
4. **One WoW install per library**, each with its own `.build.info` and CASC
   store, so the install-target picker means what it says.

## Addons

`addons/`. Two halves are entirely offline: what's installed, read from the
`.toc` files in the AddOns directory (which carry the provider ids their
authors published — `X-Curse-Project-ID` and friends), and a WoWUp import
token, which is base64 JSON listing provider ids. Because both speak provider
ids, an import can be reconciled against an install without asking any API
anything.

The rest:

- **Providers**: CurseForge (via the proxy, below), WoWInterface, Tukui and
  GitHub, each behind a small module in `addons/providers/`.
- **The proxy.** CurseForge's API needs a key, and a key shipped in a client
  is a key given away, so requests go through a small Go service that holds it
  server-side, caches hard and never lets the key reach the launcher.
- **Fingerprinting** (`addons/fingerprint.rs`) implements CurseForge's
  undocumented folder hash, which is what lets a hand-installed addon be
  recognised by its contents and adopted.
- **The ledger** records what Lightforge installed, which folders each package
  wrote (removal deletes exactly those), the file id, and per-addon choices.
- **Channels are a floor, not a filter**: choosing "beta" means beta *or
  better*, so an addon that ships a release after a beta still updates to it.

## Assets, news and the catalogue

`assets.rs` resolves a product's artwork through a hosted manifest, cached by
URL and revalidated with ETags so a start-up check costs a `304`. The
**product catalogue lives in that manifest too**, not in the binary: a new PTR
is one line in a published table plus a deploy, no release. Only *identity* is
server-driven — install directory and executable stay derived from the code,
so a bad publish can mislabel a product but never install one over another.

That derivation is also the limit. `wow_<flavor>` codes install to
`_<flavor>_`, so a new one needs no release; a code of any other shape does.
`wowf` (WoW: Forever) installs to `_forever_`, which its product config states
and its name does not, so it is a built-in code. And because an older build
reads the same manifest, **a manifest must not name a code that older builds
derive wrongly**: 1.0.0-beta.3 would install a stated `wowf` into a literal
`wowf/` folder. Its art therefore stays on the beta's entry, and the live
branch borrows it. Reading the folder and executable from the product config
would lift the limit, and is the planned fix.

`news.rs` fetches headlines through the same backend and files each article
against the game it belongs to.

## The GUI

`crates/launcher-gui`, iced 0.14, Elm architecture: one `Message` enum, one
`update`, one `view`.

- **`tasks.rs`** is the async boundary — every network or blocking call the UI
  makes goes through a function there and comes back as a `Message`.
- **`state.rs`** (in core, not the GUI) is the primary button's state machine.
  Three of its rules are deliberate: `.build.info` is the completion marker, an
  unreachable version service does *not* block launching a complete install,
  and a live transfer outranks whatever the last disk scan said.
- **`art.rs`** decodes artwork once and holds it as RGBA, because the renderer
  keeps a decoded image only while drawing. JPEGs are decoded at a reduced DCT
  scale when a thumbnail is wanted, which is the difference between a 33 MB
  buffer and a 0.5 MB one.
- **The allocator is tuned at startup.** `mallopt` pins the mmap threshold, so
  large short-lived image buffers are returned to the OS instead of being
  retained in glibc's per-thread arenas. Measured: it took the News page from
  444 MB resident to 149 MB.
- **Rendering** is wgpu with an automatic tiny-skia fallback;
  `ICED_BACKEND=tiny-skia` forces software.

## Configuration

TOML at `~/.config/lightforge/config.toml`, written atomically. It holds
settings a person edits — region, locale, libraries, per-product profiles,
optimization choices. Two things that are *state* rather than settings live
beside it instead: the download queue (`transfers.rs`, rewritten on every
pause and resume) and the addon ledger.

## Tests

`cargo test` runs 775 tests and takes a few seconds. Four kinds:

- **Unit tests** beside the code, testing behaviour rather than
  implementation. Many are regression tests with a comment naming the bug.
- **Wire-contract tests** (`crates/core/tests/catalog_contract.rs`) against
  JSON fixtures **generated by the proxy's own Go test suite**. Three
  Go↔Rust shape mismatches were found this way, each of which failed a whole
  response rather than one field.
- **Acceptance tests** (`crates/core/tests/acceptance.rs`), `#[ignore]` by
  default because they hit Blizzard's CDN and write gigabytes. They exist
  because the bugs worth catching lived in seams, not in functions: the store
  writer was fine and the registry splice was fine, and the splice still did
  nothing against a registry shaped like a real one.
- **Generated-doc checks**: `make check-cli-doc` fails if `docs/CLI.md` no
  longer matches the binary.

## Building and releasing

`cargo build --release` gives you a binary for *your* machine. For one that
runs on other people's:

```bash
make release
```

builds inside a Debian 12 container (glibc 2.36) and **fails if the result
asks for a newer glibc than that**. The check is the point: a build that
quietly picks up the host's libc looks fine locally and fails on a Steam Deck.
Measured here — a host build wanted glibc 2.44, which SteamOS (~2.37) and
Bazzite (~2.40) are both older than.

The binary links `libc`, `libm`, `libgcc_s` and `libudev` (gamepads, through
gilrs) and nothing else; Vulkan, Wayland and X11 are all opened at runtime.

## Deliberate non-goals

- **Linux only.** No Windows build, and no platform-abstraction scaffolding
  for one.
- **WoW-first.** Other Blizzard titles are a hand-off to Battle.net by design.
- **Nothing inside the game.** No memory access, no injection, no credential
  handling — see [DISCLAIMER.md](DISCLAIMER.md). This is the constraint that
  shapes the whole architecture: everything here is about files, prefixes and
  process lifecycle, because that is the entire permitted surface.
- **No self-updater yet**, which is why distribution packages matter.

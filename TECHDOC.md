# Lightforge technical documentation

How the launcher works underneath: the formats it speaks, the decisions you
can't see from reading a single function, and where to find the details.

[README.md](README.md) says what Lightforge does. The module doc comments hold
the specifics: most modules start with a few paragraphs on why they are built
the way they are. This page gives the overview and points to them instead of
repeating them.

## Where it came from

I had the idea at the beginning of 2026: a Linux launcher for WoW that doesn't
depend on Battle.net. For months, the important part (downloading and patching
the game yourself) looked impossible. It turned out to be undocumented, not
closed. In **August 2026** I worked out how Blizzard's distribution pipeline
fits together: the version service, the manifests, the archives, and the local
storage format they end up in. After that, the rest was a lot of work, but no
more guessing.

This repository starts on **16 September 2026**. It's a rebuild of an earlier
attempt.

### How it was built

This is the second implementation. The first one came before this repository,
and what it really produced was an understanding of the problem, not a working
product. Because I rebuilt from scratch in September with that knowledge, the
architecture here was planned up front: the prefix layout, the separate games
directory, the shape of the download path and the synchronous core were all
things the prototype had already tested.

I built the rebuild with the help of [Claude Code](https://claude.com/claude-code),
and I want to say that openly. "Written with an AI assistant" can mean two
very different ways of working, and only one of them gives you a codebase you
can maintain, so here is what it means for this one.

The assistant wrote code quickly, against decisions I had already made, and
argued back when it disagreed, which was a big part of its value. What it did
not do:

- **Decide the architecture.** Every real choice came to me as a trade-off
  with a recommendation, and I decided: one shared prefix instead of one per
  product, a synchronous core with the network at its edge, the product
  catalogue in a hosted manifest instead of the binary, libraries identified
  by a marker file instead of by their path, per-game Proton pinning, and
  shared optimization profiles for Battle.net games. Those choices are why the
  code looks the way it does.
- **Decide what was true.** The protocol work was checked against real CDN
  responses, not against what a model believed about TACT, and the model's
  beliefs were wrong more than once. The `max_gap` default below comes from a
  measured sweep. The 32-bit remedies in `runtime.rs` carry `// UNVERIFIED`
  markers on the ones that have never run on a real machine, because reasoning
  about distro layouts gave two wrong answers before a VM caught them.
- **Have the last word.** I read everything before it was committed, and sent
  back or threw away plenty. A reverted writer-thread rewrite is still written
  down so nobody tries it again.

That's why the comments are full of measurements, rejected alternatives and
notes on what's verified and what's only reasoned. When this document is oddly
specific about something, it's usually because the first, reasonable version
of that code was wrong, and a real test showed it.

## The shape of it

A Cargo workspace with three crates, depending in one direction:

| Crate | Lines | What it is |
| --- | --- | --- |
| `lightforge-core` | ~40,000 | All the domain logic. Knows nothing about any UI. |
| `lightforge-gui` | ~27,000 | The iced application. |
| `lightforge-cli` | ~4,000 | A thin `clap` front end over the same core. |

Both front ends use `core`. `core` depends on neither, and never on iced or
clap. That rule is what makes the CLI possible, and the CLI made most of this
debuggable: a protocol bug is much easier to chase from a terminal than
through a UI.

**`core` is synchronous except at the network edge.** Everything that touches
HTTP is an `async fn` in `net.rs` and the modules that call it. Everything
else is normal blocking code that a test can call directly. The GUI runs the
async parts on tokio from `tasks/`, and blocking work on `spawn_blocking`, so
the render thread never waits on either.

**Errors**: each module has a `thiserror` enum for what can go wrong in it
(`tact::download::DownloadError`, `library::LibraryError`, …), and
`error::CoreError` collects them at the crate boundary. The CLI and GUI wrap
those in `anyhow`. Error text ends up in a status line the user reads, so it's
written as a sentence and not as a type name.

**TLS is rustls**, not the system OpenSSL, so it behaves the same whatever
crypto policy a distribution sets.

## On-disk layout

All paths come from `paths::Layout`, so the structure is defined in one place:

```text
~/Games/Blizzard/
├── prefix/                        Proton compat-data dir (disposable)
│   └── pfx/                       the Wine prefix itself
│       └── drive_c/
│           └── Program Files (x86)/
│               ├── Battle.net/
│               └── World of Warcraft/   -> symlink into games/
└── games/
    └── World of Warcraft/         the real install: Data/, .build.info, _retail_/
```

Game files live on the normal filesystem and are **symlinked** into the
prefix, so you can delete and rebuild the prefix without touching 120 GB of
downloads. The game isn't started through that symlink, though: each library
also gets its own drive letter, and the game runs from there, which the
in-game shop needs (see [The in-game shop](#the-in-game-shop)).

Lightforge's own data follows XDG and is kept apart from the games:

```text
~/.config/lightforge/config.toml      settings, written atomically
~/.local/share/lightforge/            Proton builds, the addon ledger, the queue, logs
~/.cache/lightforge/                  artwork, catalogues (disposable)
```

## Getting a game: TACT / NGDP

`tact/` is the biggest part of the code and the reason the project exists:
Blizzard's distribution pipeline, reimplemented closely enough that Agent
accepts the result as its own.

### Asking what exists

```text
https://<region>.version.battle.net/<product>/versions
https://<region>.version.battle.net/<product>/cdns
https://<region>.version.battle.net/<product>/bgdl      (pre-downloads)
```

All three return **BPSV**, the same pipe-separated table format as
`.build.info`. `tact/version.rs` parses it itself instead of using
`ribbit-client` or `tact-client`: the first gets HTTP 400 from Blizzard's TCP
endpoint on both protocol versions, and the second's V2 path rejects a
response the server sends with status 200. There is also **no product
listing**, since the `summary` endpoint returns 404, so finding products means
trying a list of candidate codes.

A `versions` row names two important hashes: the **build config** and the
**CDN config**.

### What a build is made of

```text
build config ──┬── encoding manifest    content key → encoding key
               ├── download manifest    which files this install wants
               └── install manifest     the loose files (Wow.exe, DLLs, CEF)

CDN config ────── archive list          ~1400 archives, each with an index
```

- **`tact/encoding.rs`**: the content-key to encoding-key map. Third-party
  parsers often get its page-table layout wrong, and then lose 95% of the
  manifest.
- **`tact/manifest.rs`**: the download manifest decides *which* files this
  machine wants, by tag: platform, locale and optional categories. Tags are
  why a download can be 37 GiB bigger than expected. Some categories have no
  name in the manifest, so they have to be found by looking at the tags, never
  from a hard-coded list.
- **`tact/archive.rs` / `tact/indices.rs`**: archive indices say which archive
  holds a key and at what offset. They're cached locally, because nothing can
  be *located* without them.

### The plan

`tact/plan.rs` is the most important piece of the downloader. A retail
install's files are spread across ~1400 archives:

- one range request per file would be **1.57 million requests**;
- whole archives instead would be **174.5 GiB downloaded for 123.0 GiB
  wanted**.

Neither works, so the plan sits in between: it locates every wanted key, sorts
them by `(archive, offset)`, merges neighbours closer than `max_gap` into a
**run** of at most `max_run`, then fetches each run with one range request and
splits it locally. It runs 32 requests at once by default. I measured that at
2.4× the throughput of 8, because these transfers are limited by latency, not
bandwidth.

`tact/explain.rs` answers the question "why is this plan that size?".

### Writing the store

`tact/writer.rs` writes bytes exactly the way Agent does. Anything else and
Agent decides the install is damaged and starts the repair scan this project
is meant to avoid. Three details are essential, and you can't guess any of
them:

1. Every archive reserves `idx::ARCHIVE_RESERVED` bytes at its start, and each
   bucket's index records a marker pointing at its own slot there.
2. Every file has a **30-byte entry header** (`tact/entry.rs`). Its second
   checksum includes the archive number *and the offset it's written at*, so
   a file is only valid in the place it was written.
3. The `.idx` files are V7 (`tact/idx.rs`), with two different Jenkins hashes
   and headerless aliases.

Content is stored **BLTE-encoded, as downloaded**. About 2% of a build is
encrypted with keys Blizzard hasn't published yet, and because the encoded
bytes are stored as they are, that content needs no special handling.

### Resume, verify, repair, reclaim

- **Resume** comes from the plan: what the store already holds is left out
  before anything is fetched, so an interrupted download continues where it
  stopped.
- **`.build.info` marks completion.** It's written last, and only after a
  complete run (`tact/install/` lists every way a run can fall short), so a
  half-finished install shows `Install` and not `Play`.
- **Verify, two depths** (`tact/verify.rs`). The fast pass reads each file's
  30-byte header: about 55 MB of reads across a 140 GB store, sorted to be
  sequential. The deep pass hashes every stored byte *without decoding
  anything*: a BLTE file's encoding key is the MD5 of its chunk table, and the
  table holds an MD5 of every chunk as stored, so the raw bytes verify the
  whole file with no zlib and no decryption keys. Both are safe while the game
  runs. A `Snapshot` taken before and compared after turns "someone else wrote
  to this archive" into an error, so it isn't reported as damage.
- **Repair** is a download: a damaged entry is removed from the index, the
  planner then sees the file as missing, and the next download fetches exactly
  that file.
- **Reclaim** (`tact/reclaim.rs`) frees the space of one version in a store
  shared by several. What to keep comes from the builds that are *installed*
  (read from `.build.info`) or pre-downloaded, never from what the CDN serves
  today, because a version one patch behind still needs files its newer build
  dropped. Each build's encoding manifest is read from the store, or from the
  CDN when the store doesn't have it; Blizzard's download manifests don't list
  it, so a build Lightforge updated may not have it locally.

## Telling Battle.net about an install

There are two files, both in Blizzard's formats.

**`.build.info`** (`buildinfo.rs`): BPSV, one row per product, shared by
Retail, Classic and the PTRs. Two rules: columns are found by name, not by
position, because Blizzard reorders them between expansions; and only the
target product's row is ever changed, because rewriting the whole file would
overwrite the other products. Writes are atomic (temp file and rename),
because Agent memory-maps this file and a half-written file would corrupt it.

**`.product.db`** (`tact/productdb.rs`): Agent's protobuf-style registry of
what is installed where. Without it, Battle.net shows error 2800 and won't
adopt an install. Lightforge writes it from `.build.info`, so the two always
agree, and merges a record into Agent's own registry so the client lists the
game without the user starting a scan.

Reading it back has one trap that caused a real bug: a record has **two**
names. Field 1 is the client's install key, field 2 is the TACT product code.
They're the same for every product except Hearthstone, which is `hs_beta` to
Battle.net and `hsb` to the CDN. Look up only one of them, and an installed
game shows as not installed.

`discover.rs` uses the same format the other way around: it finds installs
that existed before Lightforge by reading the `product.db` of *other*
launchers' prefixes (Lutris, Bottles, Heroic, a hand-made `~/.wine`). It can
copy a record from one into ours as raw bytes, with only the install path
changed. That's how a game installed elsewhere becomes playable here without
moving any files.

## The prefix, Proton, and launching

- **`prefix.rs`** creates the prefix by running Proton itself (`wineboot`),
  then adds the POSIX symlink. Each library gets two drive letters, because the
  in-game browser needs the install to look like a normal Windows path.
- **`proton.rs`** knows Proton-GE releases, checks GitHub with an ETag so a
  check is free when nothing changed, and verifies each download against the
  **sha512 published with it** before unpacking. Builds can be pinned per
  product, including for the games Battle.net starts.
- **`runtime.rs`** checks whether this machine can run 32-bit Windows programs
  at all, which is behind the most common first-run failure. It looks for a
  32-bit loader before Proton is downloaded, and runs Proton's own 32-bit wine
  before the Battle.net step. Neither check can prove the 32-bit *graphics*
  stack is there, so the suggested fix names the distribution's 32-bit Wine
  package: its dependencies are what's actually needed.
- **`preflight.rs`** makes sure everything is ready before Battle.net starts:
  no stale Agent holding a lock, no leftover wineserver, and any download that
  writes to the CASC store paused first. Two writers in one store can really
  corrupt it.

### The in-game shop

The shop and the WoW Token don't use Battle.net's browser. They use
`Utils/BlizzardBrowser.exe`, a CEF 83 child process that `Wow.exe` starts
itself. Nothing passed to Battle.net (`--in-process-gpu` included) reaches
them, and they work from a direct launch with no Battle.net running.

Started from `C:` (through the prefix symlink) or the catch-all `Z:`, the
browser's GPU process crashes on start. Its log in
`_retail_/Logs/BlizzardBrowser/CEF-*.log` says `The GPU process has crashed`
three times, then Chromium gives up and shows a grey rectangle. With the
install on its own drive letter (`dosdevices/d:` and up) and the game started
by its native path, so Wine maps it onto that drive, the same log says
`Exiting GPU process due to errors during initialization` once, Chromium
switches to software rendering, and the page draws. I tested it up to a
completed purchase. Launching by the native path alone, without the drive
letter, doesn't help.

It also needs a recent enough Proton: on GE-Proton10-34 the support page
rendered but the shop still hung on "Loading…"; GE-Proton11-7 works. Related
upstream: Wine bug 47778.

`WSALookupServiceBegin failed with: 0` appears in every one of those logs,
working or not. It's Wine's unimplemented network-location API and has
nothing to do with the shop.

## Battle.net and the hand-off

`battlenet.rs` installs the client into the prefix without user input and
sets the registry and config defaults that keep it from getting in the way.

For games outside the WoW family, Lightforge hands off to Battle.net, and the
hard part is naming the game correctly. The client works in **game families**
(`WTCG`, `Fen`, `Pro`), not TACT product codes, and `--exec="launch <uid>"`
needs the family's uid. Guessing is risky: with an unknown id the client falls
back to the first family it knows, Warcraft III, and the user lands in a game
they may not own.

`families.rs` solves this by reading Blizzard's own published catalogue, which
the client caches on disk as JSON fragments:

```text
products[].id                    WTCG      the game family
  └ base.types.<type>.uid        hs_beta   the install key
      └ installs[<uid>].tact_product  hsb   the TACT product code
```

So the mapping is a lookup in files the client already wrote to disk, not a
table I have to maintain, and it covers every game Blizzard publishes.

The same fragments carry the **pre-install** state. Battle.net's "Pre-Install
… Available October 19, 2026" for the WoW: Forever stress test is a fragment
that gives `wowf` a `release_date_message`, and "Stress Test" in the version
picker is the part after the dash in its selector name. There's no flag for
it; the release-date message is the only signal. Two rules apply:

- **Only current fragments.** The cache is content-addressed and never
  cleaned up, so it holds every revision ever fetched (one old copy still
  calls the Forever beta "Mists of Pandaria Beta"). The newest root fragment
  lists the current hash of each one, and names and dates come only from
  those. Families are read from every revision, since they never change.
- **Only conditions Lightforge can evaluate.** A fragment can depend on
  licences, feature flags, platform, country or login region. Only the login
  region is checked, against the configured region; fragments with any other
  condition are skipped. The licence condition is why: it gives the Forever
  beta "Available November 4" for accounts that don't own Forever, and
  Lightforge doesn't look at what an account owns.

It only changes a label and a caption; it never blocks anything. PLAY stays
enabled: Blizzard's servers decide whether you get in, the cache only
refreshes when the client runs, and starting the game early lets you check the
install works before the servers open.

## Storage libraries

`library.rs`. A library is one drive's worth of games, laid out like the
default one. Four decisions:

1. **A library is identified by a marker file on the drive**
   (`games/.lightforge-library`), not by its path. An unmounted drive leaves
   an empty directory at its mount point, so the path alone can't tell "the
   drive is gone" from "the library is empty". Getting that wrong means
   downloading 120 GB onto the system disk.
2. The config also records the filesystem UUID and a relative path, used
   **only** to find a drive that came back at a different place.
3. While a drive is away, its games stay listed on it: the status line says
   so, the only action is to check again, and everything that writes refuses.
4. **One WoW install per library**, each with its own `.build.info` and CASC
   store, so the install target you pick is where the game really goes.

**Moving a game** (`migrate.rs`) copies only the store entries its build needs
that the target store doesn't already have, reads every copy back, and then
writes the game's `.build.info` row in the target. That row is the commit:
until it's written, the game still plays from the old install. Only after the
config points at the new library is the old copy removed, so a crash in
between leaves the game in two places, never in none. A copy left behind that way shows up in
Game Settings with a button to remove it.

## Addons

`addons/`. Two parts work fully offline: reading what's installed from the
`.toc` files in the AddOns directory (they carry the provider ids their
authors published, like `X-Curse-Project-ID`), and reading a WoWUp import
token, which is base64 JSON listing provider ids. Both use provider ids, so an
import can be matched against an install without asking any API.

The rest:

- **Providers**: CurseForge (through the proxy, below), WoWInterface, Tukui
  and GitHub, each in a small module in `addons/providers/`.
- **The proxy.** CurseForge's API needs a key, and anyone can pull a key out of
  a client, so requests go through a small Go service that keeps the key on
  the server, caches responses and never sends the key to the launcher.
- **Fingerprinting** (`addons/fingerprint.rs`) implements CurseForge's
  undocumented folder hash, so an addon installed by hand can be recognised by
  its files and adopted.
- **The ledger** records what Lightforge installed, which folders each package
  wrote (removing it deletes exactly those), the file id, and per-addon
  choices.
- **Channels are a minimum**: choosing "beta" means beta *or stable*, so
  an addon that ships a release after a beta still updates to it.

## Assets and the catalogue

`assets.rs` gets a product's artwork through a hosted manifest, cached by URL
and revalidated with ETags, so the check at startup costs a `304`. The
**product catalogue is in that manifest too**, not in the binary: a new PTR is
one line in a published table plus a deploy, no release. Only a product's
*identity* (its name and art) comes from the server.

The install folder and executable come from Blizzard's product config for
that product (`tact/product_config.rs`), with the product code as the fallback.
Older builds derived both from the code alone: `wow_<flavor>` installs to
`_<flavor>_`, and `wowf` (WoW: Forever, which installs to `_forever_`) was a
built-in exception. Because those builds read the same manifest, **the
manifest must not name a code that older builds derive wrongly**:
1.0.0-beta.3 would install a listed `wowf` into a literal `wowf/` folder. So
Forever's art stays on the beta's entry, and the live branch uses it from
there.

## The GUI

`crates/launcher-gui`, iced 0.14, Elm architecture: one `Message` enum, one
`update`, one `view`.

- **`update/`** has one file per area (products, launch, downloads, addons,
  storage and so on), and each `Message` is handled in exactly one of them; a
  test fails otherwise.
- **`tasks/`** is the async boundary: every network or blocking call the UI
  makes goes through a function there and comes back as a `Message`.
- **`state.rs`** (in core, not the GUI) is the state machine behind the main
  button. Three rules there are intentional: `.build.info` marks a complete
  install, an unreachable version service does *not* block launching a
  complete install, and a running transfer takes priority over what the last
  disk scan said.
- **`art.rs`** decodes artwork once and keeps it as RGBA, because the renderer
  keeps a decoded image only while drawing. JPEGs are decoded at a reduced DCT
  scale when only a thumbnail is needed: a 0.5 MB buffer instead of 33 MB.
- **The allocator is tuned at startup.** `mallopt` fixes the mmap threshold,
  so large short-lived image buffers go back to the OS instead of staying in
  glibc's per-thread arenas. Measured: it took the old News page from 444 MB
  resident to 149 MB.
- **Rendering** is wgpu, with an automatic tiny-skia fallback;
  `ICED_BACKEND=tiny-skia` forces software.
- **One instance per config.** A second launch brings the open window to the
  front and exits.

## Configuration

TOML at `~/.config/lightforge/config.toml`, written atomically. It holds the
settings a person edits: region, locale, libraries, per-product profiles,
launch settings. Two things that are *state* and not settings live next to it:
the download queue (`transfers.rs`, rewritten on every pause and resume) and
the addon ledger.

## Tests

`cargo test` runs 892 tests in a few seconds. The kinds:

- **Unit tests** next to the code, testing behaviour, not implementation.
  Many are regression tests with a comment naming the bug.
- **Property tests** (proptest) write and read back the `.idx`, store and
  `.product.db` formats with random input.
- **Wire-contract tests** (`crates/core/tests/catalog_contract.rs`) against
  JSON fixtures **generated by the proxy's own Go test suite**. They found
  three Go↔Rust shape mismatches, each of which broke a whole response
  instead of one field.
- **Live tests** (`crates/core/tests/acceptance.rs`, `migrate_live.rs` and
  others), `#[ignore]` by default because they use Blizzard's CDN or real
  installs and write gigabytes. They exist because the worst bugs were
  between modules: the store writer worked and the registry merge worked, but
  the merge still did nothing against a registry shaped like a real one.
- **Fuzz targets** in `fuzz/` for every parser of outside bytes; see
  [CONTRIBUTING.md](CONTRIBUTING.md#fuzzing-the-parsers).
- **Generated-doc checks**: `make check-cli-doc` fails if `docs/CLI.md` no
  longer matches the binary.

## Building and releasing

`cargo build --release` gives you a binary for *your* machine. For one that
runs on other people's:

```bash
make release
```

builds inside a Debian 12 container (glibc 2.36) and **fails if the result
needs a newer glibc**. Without that check, a build that picks up the host's
glibc works locally and fails on a Steam Deck. Here, a host build needed
glibc 2.44, newer than SteamOS (~2.37) and Bazzite (~2.40).

The binary links `libc`, `libm`, `libgcc_s` and `libudev` (gamepads, through
gilrs) and nothing else; Vulkan, Wayland and X11 are all loaded at runtime.

## Non-goals

- **Linux only.** No Windows build, and no abstraction layer for one.
- **WoW first.** Other Blizzard games are handed off to Battle.net.
- **Nothing inside the game.** No memory access, no injection, no handling of
  credentials; see [DISCLAIMER.md](DISCLAIMER.md). This rule shapes the whole
  architecture: everything here is about files, prefixes and processes,
  because that's all Lightforge is allowed to touch.

# The Lightforge CLI

Everything the launcher does, without the launcher — and a fair amount it
doesn't do at all, because this is also the tool the GUI was debugged with.

**`lightforge --help` lists every command**, and `lightforge <command> --help`
explains one of them in full. That is the authority; this page is the same
text, laid out so you can read it in one go.

```console
$ lightforge --help
```

One option applies everywhere:

```text
--root <ROOT>   Override the Blizzard root (default: ~/Games/Blizzard)
```

Most commands take `--product <CODE>` (`wow`, `wow_classic_era`,
`wow_classic`, …) and default to `wow`. Anything taking a path defaults to the
install your config points at, so you rarely have to pass one.

> Generated from the binary — run `make cli-doc` after changing a command.

- [Everyday](#everyday)
- [Setup, Proton and the prefix](#setup-proton-and-the-prefix)
- [Battle.net](#battlenet)
- [Games: install, update, remove](#games-install-update-remove)
- [Addons and settings backups](#addons-and-settings-backups)
- [Catalogue and artwork](#catalogue-and-artwork)

## Everyday

### `lightforge paths`

```text
Print the resolved directory layout

Usage: lightforge paths [OPTIONS]

Options:
      --root <ROOT>  Override the Blizzard root (default: ~/Games/Blizzard)
  -h, --help         Print help
```

### `lightforge config`

```text
Print the effective configuration as TOML

Usage: lightforge config [OPTIONS]

Options:
      --root <ROOT>  Override the Blizzard root (default: ~/Games/Blizzard)
  -h, --help         Print help
```

### `lightforge doctor`

```text
Check this system for what Battle.net and the games need under Proton

Usage: lightforge doctor [OPTIONS]

Options:
      --json         Print the checks as JSON
      --root <ROOT>  Override the Blizzard root (default: ~/Games/Blizzard)
  -h, --help         Print help
```

### `lightforge status`

```text
Resolve the primary action button's state for a product

Usage: lightforge status [OPTIONS]

Options:
      --product <PRODUCT>  Product code, e.g. `wow` or `wow_classic_era` [default: wow]
      --root <ROOT>        Override the Blizzard root (default: ~/Games/Blizzard)
      --region <REGION>    [default: eu]
      --offline            Skip the version service and report what the disk alone implies
  -h, --help               Print help
```

### `lightforge update-check`

```text
Compare a local install against what Blizzard currently ships

Usage: lightforge update-check [OPTIONS] [PATH]

Arguments:
  [PATH]  Install root. Defaults to the configured games directory

Options:
      --product <PRODUCT>  Product code, e.g. `wow` [default: wow]
      --root <ROOT>        Override the Blizzard root (default: ~/Games/Blizzard)
      --region <REGION>    Blizzard region to ask [default: eu]
  -h, --help               Print help
```

### `lightforge play`

```text
Launch the game directly under Proton, the `[ PLAY ]` path.

Runs pre-flight first, then starts `Wow.exe` through the prefix's Windows-visible path. Battle.net is not involved.

Usage: lightforge play [OPTIONS]

Options:
      --product <PRODUCT>
          Product to launch, e.g. `wow` or `wow_classic`
          
          [default: wow]

      --root <ROOT>
          Override the Blizzard root (default: ~/Games/Blizzard)

      --proton <PROTON>
          Proton build to launch under

      --dry-run
          Print what would run and exit

  -h, --help
          Print help (see a summary with '-h')
```

### `lightforge preflight`

```text
Clean the prefix up the way a launch would, without launching anything

Usage: lightforge preflight [OPTIONS]

Options:
      --proton <PROTON>    Proton build whose `wineserver` shuts the prefix down
      --root <ROOT>        Override the Blizzard root (default: ~/Games/Blizzard)
      --product <PRODUCT>  Product the launch is for, so a download can be paused
      --dry-run            Report what would be done without touching anything
      --grace <GRACE>      Seconds a process gets between SIGTERM and SIGKILL [default: 5]
  -h, --help               Print help
```

## Setup, Proton and the prefix

### `lightforge setup`

```text
Report or run first-run provisioning: Proton, prefix, Battle.net

Usage: lightforge setup [OPTIONS]

Options:
      --root <ROOT>      Override the Blizzard root (default: ~/Games/Blizzard)
      --status           Only report what is missing; provision nothing
      --proton <PROTON>  Proton-GE release to install if one is needed
  -h, --help             Print help
```

### `lightforge proton`

```text
List Proton builds installed on this system

Usage: lightforge proton [OPTIONS]

Options:
      --root <ROOT>  Override the Blizzard root (default: ~/Games/Blizzard)
  -h, --help         Print help
```

### `lightforge proton-check`

```text
Check upstream Proton-GE releases and report whether a newer one exists

Usage: lightforge proton-check [OPTIONS]

Options:
      --root <ROOT>  Override the Blizzard root (default: ~/Games/Blizzard)
  -h, --help         Print help
```

### `lightforge proton-install`

```text
Download, verify, and install a Proton-GE release

Usage: lightforge proton-install [OPTIONS] [VERSION]

Arguments:
  [VERSION]  Release tag, e.g. `GE-Proton11-7`. Defaults to the latest stable

Options:
      --root <ROOT>  Override the Blizzard root (default: ~/Games/Blizzard)
  -h, --help         Print help
```

### `lightforge prefix`

```text
Report on the shared prefix and the game symlink

Usage: lightforge prefix [OPTIONS]

Options:
      --root <ROOT>  Override the Blizzard root (default: ~/Games/Blizzard)
  -h, --help         Print help
```

### `lightforge prefix-init`

```text
Create or update the prefix, then put the game symlink in place

Usage: lightforge prefix-init [OPTIONS]

Options:
      --proton <PROTON>  Proton build to use, e.g. `GE-Proton11-7`. Defaults to the tested build when it is installed, else the newest supported one
      --root <ROOT>      Override the Blizzard root (default: ~/Games/Blizzard)
  -h, --help             Print help
```

### `lightforge prefix-link`

```text
Create or repair only the game symlink, without running Proton

Usage: lightforge prefix-link [OPTIONS]

Options:
      --root <ROOT>  Override the Blizzard root (default: ~/Games/Blizzard)
  -h, --help         Print help
```

## Battle.net

### `lightforge battlenet-fetch`

```text
Download Battle.net-Setup.exe into the cache

Usage: lightforge battlenet-fetch [OPTIONS]

Options:
      --root <ROOT>  Override the Blizzard root (default: ~/Games/Blizzard)
  -h, --help         Print help
```

### `lightforge battlenet-setup`

```text
Install Battle.net unattended, then apply registry and config defaults

Usage: lightforge battlenet-setup [OPTIONS]

Options:
      --proton <PROTON>        Proton build to run the installer under
      --root <ROOT>            Override the Blizzard root (default: ~/Games/Blizzard)
      --locale <LOCALE>        Battle.net UI language [default: enUS]
      --installer <INSTALLER>  Use an already-downloaded installer instead of the cached one
      --timeout <TIMEOUT>      Seconds to wait for the install to appear [default: 600]
  -h, --help                   Print help
```

### `lightforge battlenet-run`

```text
Open Battle.net in the prefix, optionally handing straight off to a game

Usage: lightforge battlenet-run [OPTIONS]

Options:
      --proton <PROTON>              Proton build to launch under
      --root <ROOT>                  Override the Blizzard root (default: ~/Games/Blizzard)
      --product <PRODUCT>            Product code to launch, e.g. `wow`. Omit to just open the client
      --exec <EXEC>                  Override the `--exec="launch <uid>"` verb Battle.net is given
      --uri <URI>                    Hand the client a `battlenet://` link instead, the way the system protocol handler does. Takes a game family, e.g. `WTCG`
      --install-game <INSTALL_GAME>  Open the client's install flow for an install uid, e.g. `w1r`
      --skip-preflight               Skip pre-flight. For testing a hand-off against a client that is already running, without killing the session first
  -h, --help                         Print help
```

## Games: install, update, remove

### `lightforge download`

```text
Download a product from Blizzard's CDN into a CASC store

Usage: lightforge download [OPTIONS] <DEST>

Arguments:
  <DEST>  Install root to build. Created if absent

Options:
      --product <PRODUCT>
          [default: wow]
      --root <ROOT>
          Override the Blizzard root (default: ~/Games/Blizzard)
      --region <REGION>
          [default: eu]
      --locale <LOCALE>
          Locale to install, e.g. `enUS` [default: enUS]
      --flavor <FLAVOR>
          Flavor directory the install files go in. Default: the one the product config names, e.g. `_forever_` for `wowf`
      --dry-run
          Plan and report, then stop without fetching any content
      --optional <OPTIONAL>
          Opt in to optional content by tag name, e.g. `HighRes`. Repeatable
      --limit-archives <LIMIT_ARCHIVES>
          Only cache this many archive indices. Bounds a test run; a real install needs all of them, and the plan is incomplete without
      --limit-runs <LIMIT_RUNS>
          Only fetch this many runs. Bounds a test run
      --skip-loose
          Skip the loose files, which are the large ones
      --install-files
          Also extract the install manifest's files (Wow.exe and friends)
      --stage
          Pre-download the build Blizzard lists ahead of its release, into the store only. The installed build stays the one that launches
      --concurrency <CONCURRENCY>
          Requests in flight. These transfers are request-latency bound, not bandwidth bound: 8 measured 9-10 MB/s where 32 measured 25 [default: 32]
      --gap-kib <GAP_KIB>
          Merge files separated by fewer than this many KiB into one request [default: 64]
      --checkpoint-mib <CHECKPOINT_MIB>
          Rewrite the indices every this many MiB. Bounds what an interrupted download has to fetch again [default: 4096]
      --show-install
          With `--dry-run`, list the install-manifest files this build would extract, and where each would land
      --cancel-after <CANCEL_AFTER>
          Trip the cancel token after this many seconds, the same path the GUI's pause button takes. For measuring how long a pause takes to come to rest
  -h, --help
          Print help
```

### `lightforge verify`

```text
Verify every file in a store against its own entry headers, and optionally drop the damaged ones so a download refetches them

Usage: lightforge verify [OPTIONS] [PATH]

Arguments:
  [PATH]  Install to check. Defaults to the configured one

Options:
      --deep         Also hash every stored byte against its chunk table and key. Reads the whole store, minutes rather than seconds, and is safe while the game runs
      --root <ROOT>  Override the Blizzard root (default: ~/Games/Blizzard)
      --repair       Remove damaged entries from the indices. Requires a closed client
      --all          List every damaged entry rather than the first few
  -h, --help         Print help
```

### `lightforge reindex`

```text
Rewrite a store's `.idx` files without touching its archives.

Repairs an install whose indices were written with too little free space for the client's update section, the `NeedsRepair` case, in seconds, instead of re-downloading the whole product.

Usage: lightforge reindex [OPTIONS] [PATH]

Arguments:
  [PATH]
          Install root. Defaults to the managed WoW install

Options:
      --root <ROOT>
          Override the Blizzard root (default: ~/Games/Blizzard)

  -h, --help
          Print help (see a summary with '-h')
```

### `lightforge move`

```text
Move a game to another library without downloading it again. Prints the plan unless `--yes` is given

Usage: lightforge move [OPTIONS] --to <TO>

Options:
      --product <PRODUCT>  [default: wow]
      --root <ROOT>        Override the Blizzard root (default: ~/Games/Blizzard)
      --to <TO>            The library to move it to, by name or id, as `lightforge paths` lists them
      --yes                Actually move. Without this, nothing is touched
  -h, --help               Print help
```

### `lightforge uninstall`

```text
Remove an installed product. Prints the plan unless `--yes` is given

Usage: lightforge uninstall [OPTIONS]

Options:
      --product <PRODUCT>  [default: wow]
      --root <ROOT>        Override the Blizzard root (default: ~/Games/Blizzard)
      --path <PATH>        Install root. Defaults to the managed WoW install
      --yes                Actually delete. Without this, nothing is touched
  -h, --help               Print help
```

### `lightforge prune`

```text
Drop game data no installed or pre-installed version uses, then compact what that leaves. Prints the plan unless `--yes`.

That data is mostly what earlier patches left behind. Never run it while a download is running or paused: what that has fetched so far belongs to no version yet, and would have to be fetched again.

Usage: lightforge prune [OPTIONS]

Options:
      --path <PATH>
          Install root. Defaults to the managed WoW install

      --root <ROOT>
          Override the Blizzard root (default: ~/Games/Blizzard)

      --yes
          Actually drop and compact. Without this, nothing is touched

  -h, --help
          Print help (see a summary with '-h')
```

### `lightforge compact`

```text
Rewrite the store's archives that are mostly dead space, copying what still lives in them into fresh ones. Prints the plan unless `--yes`.

Dead space is what dropped files leave behind: earlier patches, a repair, or a version uninstalled before uninstalling compacted.

Usage: lightforge compact [OPTIONS]

Options:
      --path <PATH>
          Install root. Defaults to the managed WoW install

      --root <ROOT>
          Override the Blizzard root (default: ~/Games/Blizzard)

      --yes
          Actually rewrite. Without this, nothing is touched

  -h, --help
          Print help (see a summary with '-h')
```

### `lightforge build-info`

```text
Show the `.build.info` row for a product

Usage: lightforge build-info [OPTIONS] [PRODUCT]

Arguments:
  [PRODUCT]  Product code, e.g. `wow` or `wow_classic` [default: wow]

Options:
      --root <ROOT>  Override the Blizzard root (default: ~/Games/Blizzard)
  -h, --help         Print help
```

## Addons and settings backups

### `lightforge addons`

```text
List the addons installed for a product

Usage: lightforge addons [OPTIONS]

Options:
      --product <PRODUCT>  [default: wow]
      --root <ROOT>        Override the Blizzard root (default: ~/Games/Blizzard)
      --path <PATH>        Read this AddOns directory instead of the product's own
      --token <TOKEN>      Reconcile against a WoWUp import token (or `@file`)
  -h, --help               Print help
```

### `lightforge addon-search`

```text
Search for addons across every provider that can answer

Usage: lightforge addon-search [OPTIONS] <QUERY>

Arguments:
  <QUERY>  What to search for, or `owner/repo` for a GitHub addon

Options:
      --product <PRODUCT>  [default: wow]
      --root <ROOT>        Override the Blizzard root (default: ~/Games/Blizzard)
      --limit <LIMIT>      [default: 15]
      --catalog <CATALOG>  
  -h, --help               Print help
```

### `lightforge addon-install`

```text
Install an addon by provider and id

Usage: lightforge addon-install [OPTIONS] <ID>

Arguments:
  <ID>  Provider id: a number, a slug, or `owner/repo`

Options:
      --provider <PROVIDER>  [default: wowinterface]
      --root <ROOT>          Override the Blizzard root (default: ~/Games/Blizzard)
      --product <PRODUCT>    [default: wow]
      --catalog <CATALOG>    
  -h, --help                 Print help
```

### `lightforge addon-import`

```text
Install or update addons from a WoWUp import token

Usage: lightforge addon-import [OPTIONS] <TOKEN>

Arguments:
  <TOKEN>  The token, or `@path` to read it from a file

Options:
      --product <PRODUCT>  [default: wow]
      --root <ROOT>        Override the Blizzard root (default: ~/Games/Blizzard)
      --catalog <CATALOG>  Catalog base URL, or a fixture directory. Defaults to config
      --dry-run            Resolve and report without writing anything
  -h, --help               Print help
```

### `lightforge addon-update`

```text
Update every addon Lightforge installed for a product

Usage: lightforge addon-update [OPTIONS]

Options:
      --product <PRODUCT>  [default: wow]
      --root <ROOT>        Override the Blizzard root (default: ~/Games/Blizzard)
      --catalog <CATALOG>  
  -h, --help               Print help
```

### `lightforge addon-remove`

```text
Remove an addon Lightforge installed

Usage: lightforge addon-remove [OPTIONS] <ID>

Arguments:
  <ID>  Provider id, e.g. `2382`

Options:
      --provider <PROVIDER>  [default: curse]
      --root <ROOT>          Override the Blizzard root (default: ~/Games/Blizzard)
      --product <PRODUCT>    [default: wow]
  -h, --help                 Print help
```

### `lightforge addon-adopt`

```text
Recognise hand-installed addons by their contents and manage them

Usage: lightforge addon-adopt [OPTIONS]

Options:
      --product <PRODUCT>  [default: wow]
      --root <ROOT>        Override the Blizzard root (default: ~/Games/Blizzard)
      --path <PATH>        Read this AddOns directory instead of the product's own
      --ledger <LEDGER>    Write the ledger here instead of beside the product's
      --catalog <CATALOG>  Base URL of the addon catalog, or a fixture directory
  -h, --help               Print help
```

### `lightforge backup-list`

```text
List a game's `WTF/Account` backups, newest first

Usage: lightforge backup-list [OPTIONS]

Options:
      --product <PRODUCT>  [default: wow]
      --root <ROOT>        Override the Blizzard root (default: ~/Games/Blizzard)
  -h, --help               Print help
```

### `lightforge backup-create`

```text
Back up a game's `WTF/Account` now. Skipped when nothing changed

Usage: lightforge backup-create [OPTIONS]

Options:
      --product <PRODUCT>  [default: wow]
      --root <ROOT>        Override the Blizzard root (default: ~/Games/Blizzard)
  -h, --help               Print help
```

### `lightforge backup-restore`

```text
Restore a backup over `WTF/Account`. The current state is backed up first, so a restore can be undone the same way

Usage: lightforge backup-restore [OPTIONS] <BACKUP>

Arguments:
  <BACKUP>  The number `backup-list` shows (1 is the newest), or the file name

Options:
      --product <PRODUCT>  [default: wow]
      --root <ROOT>        Override the Blizzard root (default: ~/Games/Blizzard)
      --account <ACCOUNT>  Only this game account's folder, as `backup-list` names it; the other accounts and what they share stay as they are
  -h, --help               Print help
```

## Catalogue and artwork

### `lightforge products`

```text
Probe Blizzard's version service for which product codes are live.

There is no listing endpoint any more, so this asks about a built-in list of candidate codes and reports what answers.

Usage: lightforge products [OPTIONS]

Options:
      --region <REGION>
          [default: eu]

      --root <ROOT>
          Override the Blizzard root (default: ~/Games/Blizzard)

      --code <CODE>
          Extra codes to probe beyond the built-in list

      --all
          Show codes that answered 404 as well

      --concurrency <CONCURRENCY>
          Simultaneous probes
          
          [default: 8]

  -h, --help
          Print help (see a summary with '-h')
```

### `lightforge assets`

```text
Resolve a product's themed branding through the asset pipeline

Usage: lightforge assets [OPTIONS]

Options:
      --product <PRODUCT>  Product code to theme, e.g. `wow` [default: wow]
      --root <ROOT>        Override the Blizzard root (default: ~/Games/Blizzard)
      --source <SOURCE>    Base URL or fixture directory. Defaults to `assets_source` in config.toml, else the built-in source
  -h, --help               Print help
```

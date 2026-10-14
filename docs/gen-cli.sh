#!/bin/sh
# Regenerate docs/CLI.md from the binary's own --help.
#
# Written out rather than hand-maintained for one reason: a page listing
# forty-four commands by hand is wrong the first time a flag changes, and
# nobody notices until someone follows it. Everything below the headings is
# clap's output verbatim.
#
#   make cli-doc            (or)   sh docs/gen-cli.sh [path/to/lightforge]
set -eu

BIN=${1:-target/release/lightforge}
OUT=docs/CLI.md

[ -x "$BIN" ] || { echo "no binary at $BIN — cargo build --release first" >&2; exit 1; }

# Command groups, in the order they appear. A command in none of them stops
# the run: a new subcommand should be filed deliberately, not dropped on the
# floor because this list was not updated with it.
group_names='Everyday|Setup, Proton and the prefix|Battle.net|Games: install, update, remove|Addons and settings backups|Catalogue and artwork|Digging into the CDN and CASC'
group_cmds='paths config status update-check play preflight
setup proton proton-check proton-install prefix prefix-init prefix-link
battlenet-fetch battlenet-setup battlenet-config battlenet-run product-db
download plan verify reindex uninstall reclaim build-info build-info-set
addons addon-search addon-install addon-import addon-update addon-remove addon-adopt addon-fingerprints addon-token backup-list backup-create backup-restore
products assets
cdn-fetch cdn-archive content casc idx casc-rebuild casc-verify'

all=$("$BIN" --help | awk '/^  [a-z]/ { print $1 }' | grep -v '^help$')

# Every command has to be filed somewhere, and nothing may be filed twice.
for cmd in $all; do
    hits=$(printf '%s\n' $group_cmds | grep -cx "$cmd" || true)
    [ "$hits" = 1 ] || { echo "$cmd appears $hits times in the group list — fix docs/gen-cli.sh" >&2; exit 1; }
done
for cmd in $group_cmds; do
    printf '%s\n' $all | grep -qx "$cmd" || { echo "$cmd is in the group list but not in --help" >&2; exit 1; }
done

{
    cat <<'HEADER'
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
HEADER

    # Table of contents. `printf '%s\n'`, not `%s`: without the trailing
    # newline `read` discards the last group, which silently cost this page
    # its CDN section the first time it ran.
    echo
    printf '%s\n' "$group_names" | tr '|' '\n' | while IFS= read -r name; do
        anchor=$(printf '%s' "$name" | tr 'A-Z' 'a-z' | sed 's/[^a-z0-9 -]//g; s/ /-/g')
        echo "- [$name](#$anchor)"
    done

    # Each group, then each command in it.
    line=0
    printf '%s\n' "$group_names" | tr '|' '\n' | while IFS= read -r name; do
        line=$((line + 1))
        echo
        echo "## $name"
        for cmd in $(printf '%s\n' "$group_cmds" | sed -n "${line}p"); do
            echo
            echo "### \`lightforge $cmd\`"
            echo
            echo '```text'
            "$BIN" "$cmd" --help
            echo '```'
        done
    done
} >"$OUT"

echo "wrote $OUT ($(wc -l <"$OUT") lines)"

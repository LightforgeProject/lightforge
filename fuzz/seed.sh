#!/bin/sh
# Seed fuzz/corpus/ from a real install. Usage: fuzz/seed.sh <install dir>
# e.g. fuzz/seed.sh "$HOME/Games/Blizzard/games/World of Warcraft"
#
# The corpus stays local (it is Blizzard's data, and Agent's and Steam's
# files carry account details). Encoding, manifests and BLTE live inside
# the archives, so those targets start from their magic alone.
set -eu

install=${1:?usage: fuzz/seed.sh <install dir>}
corpus=$(dirname "$0")/corpus
steam=${STEAM_ROOT:-$HOME/.local/share/Steam}

for t in archive_index idx idx_file_name encoding manifest blte \
    productdb_decode productdb_splice steam_libraryfolders steam_loginusers steam_shortcuts; do
    mkdir -p "$corpus/$t"
done

ls "$install/Data/indices/"*.index | head -n 5 | while read -r f; do cp "$f" "$corpus/archive_index/"; done
ls "$install/Data/data/"*.idx | head -n 3 | while read -r f; do
    cp "$f" "$corpus/idx/"
    basename "$f" > "$corpus/idx_file_name/$(basename "$f")"
done

printf 'EN' > "$corpus/encoding/magic"
printf 'IN' > "$corpus/manifest/install"
printf 'DL' > "$corpus/manifest/download"
printf 'BLTE\0\0\0\0Z' > "$corpus/blte/magic"

db=$(find "$install/.." "$HOME/Games/Blizzard/prefix" -name product.db -path '*Agent*' 2>/dev/null | head -n 1)
if [ -n "$db" ]; then
    cp "$db" "$corpus/productdb_decode/"
    cp "$db" "$corpus/productdb_splice/"
fi

[ -f "$steam/steamapps/libraryfolders.vdf" ] && cp "$steam/steamapps/libraryfolders.vdf" "$corpus/steam_libraryfolders/"
[ -f "$steam/config/loginusers.vdf" ] && cp "$steam/config/loginusers.vdf" "$corpus/steam_loginusers/"
find "$steam/userdata" -name shortcuts.vdf 2>/dev/null | head -n 3 | while read -r f; do
    cp "$f" "$corpus/steam_shortcuts/$(basename "$(dirname "$(dirname "$f")")").vdf"
done
echo "seeded $corpus"

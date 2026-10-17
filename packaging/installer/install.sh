#!/usr/bin/env bash
# Installs or updates Lightforge for the current user. No root, nothing
# outside the home folder: the binaries in ~/.local/bin, the menu entry and
# icons under ~/.local/share, so running it again is an update.
#
#   install.sh                    newest release from GitHub
#   install.sh <tarball>          a tarball already on disk (testing a build)
#   install.sh --uninstall        remove it again; games and settings stay
#
# Usually run by lightforge-installer.desktop as `curl ... | bash`, so stdin
# is this script: prompts read from /dev/tty, and everything sits in main(),
# which only runs once the whole file has arrived.

set -euo pipefail

REPO=LightforgeProject/lightforge
DATA=${XDG_DATA_HOME:-$HOME/.local/share}
BIN=$HOME/.local/bin
APPS=$DATA/applications
ICONS=$DATA/icons
ENTRY=pro.tuxcore.Lightforge.desktop

say() { printf '\n==> %s\n' "$*"; }

# Konsole closes the moment the script ends, so whatever was printed last
# has to wait for the reader.
pause() {
    if { : </dev/tty; } 2>/dev/null; then
        read -r -t "${1:-600}" -p 'Press Enter to close this window.' _ </dev/tty || true
        echo
    fi
}

fail() {
    printf '\nInstall failed: %s\n\n' "$*" >&2
    pause
    exit 1
}

need() {
    for tool in "$@"; do
        command -v "$tool" >/dev/null || fail "this needs '$tool', which is not installed"
    done
}

# Plasma and GNOME watch the applications folder themselves; these only make
# the menu catch up sooner where the tools exist.
refresh_menu() {
    command -v update-desktop-database >/dev/null && update-desktop-database -q "$APPS" || true
    command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true
}

uninstall() {
    say "Removing Lightforge"
    rm -f "$BIN/lightforge-gui" "$BIN/lightforge" "$APPS/$ENTRY"
    find "$ICONS/hicolor" -name 'pro.tuxcore.Lightforge.*' -delete 2>/dev/null || true
    refresh_menu
    echo "Done. Your games, the Wine prefix and your settings were left where they are."
    pause 30
}

# The newest release's tarball, prereleases included: during the beta every
# release is one, and /releases/latest skips them.
latest_tarball_url() {
    curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=5" |
        grep -o '"browser_download_url": *"[^"]*-x86_64-linux\.tar\.gz"' |
        head -n 1 |
        sed 's/.*"\(https[^"]*\)"$/\1/'
}

main() {
    if [ "${1:-}" = --uninstall ]; then
        uninstall
        return
    fi

    need curl tar sha256sum sed grep find install

    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT

    if [ -n "${1:-}" ]; then
        [ -f "$1" ] || fail "no such file: $1"
        tarball=$1
        say "Installing from $tarball"
    else
        say "Looking up the newest Lightforge release"
        url=$(latest_tarball_url) || true
        [ -n "${url:-}" ] || fail "could not reach GitHub to find the newest release"
        name=${url##*/}

        say "Downloading $name"
        curl -fL --progress-bar -o "$work/$name" "$url" || fail "the download did not finish"
        curl -fsSL -o "$work/SHA256SUMS" "${url%/*}/SHA256SUMS" ||
            fail "could not download the checksums to verify it against"

        say "Checking the download"
        grep -q " $name\$" "$work/SHA256SUMS" || fail "$name is not listed in SHA256SUMS"
        (cd "$work" && grep " $name\$" SHA256SUMS | sha256sum -c --quiet) ||
            fail "the download does not match its checksum, nothing was installed"
        tarball=$work/$name
    fi

    tar -xzf "$tarball" -C "$work" || fail "could not unpack $tarball"
    src=$(find "$work" -mindepth 1 -maxdepth 1 -type d -name 'lightforge-*' | head -n 1)
    [ -n "$src" ] && [ -f "$src/lightforge-gui" ] || fail "that does not look like a Lightforge tarball"

    say "Installing into $BIN"
    install -Dm755 "$src/lightforge-gui" "$BIN/lightforge-gui"
    install -Dm755 "$src/lightforge" "$BIN/lightforge"
    # The full path: not every desktop session has ~/.local/bin on its PATH,
    # and an entry whose TryExec is not found is hidden rather than broken.
    install -Dm644 "$src/$ENTRY" "$APPS/$ENTRY"
    sed -i "s|=lightforge-gui|=$BIN/lightforge-gui|" "$APPS/$ENTRY"
    mkdir -p "$ICONS"
    cp -r "$src/icons/hicolor" "$ICONS/"
    refresh_menu

    # The CLI, not the GUI: a GUI from before 1.0.0-beta.5 ignores --version
    # and opens a window.
    version=$("$BIN/lightforge" --version 2>/dev/null || echo lightforge)
    say "Installed $version"
    echo "Lightforge is in your application menu (Games)."

    if pgrep -x lightforge-gui >/dev/null 2>&1; then
        echo "Lightforge is running: close and reopen it to use the new version."
    else
        echo "Starting Lightforge."
        setsid -f "$BIN/lightforge-gui" >/dev/null 2>&1 </dev/null || true
    fi
    pause 30
}

main "$@"

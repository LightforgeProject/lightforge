# Packaging

Everything a release needs that is not the binaries themselves: the desktop
entry, the icons, the AppStream metadata, and the three packages built from
them.

```text
packaging/
├── pro.tuxcore.Lightforge.desktop        the menu entry
├── pro.tuxcore.Lightforge.metainfo.xml   AppStream, for software centres
├── icons/hicolor/…                       16px → 512px, plus the SVG
└── aur/
    ├── lightforge/PKGBUILD               builds from the tagged source
    └── lightforge-bin/PKGBUILD           installs the released binaries
```

The `.deb` and `.rpm` are not here: their metadata lives in
`crates/launcher-gui/Cargo.toml`, because `cargo-deb` and `cargo-generate-rpm`
read it from there.

## One string, three places

`pro.tuxcore.Lightforge` is the AppStream component id, the desktop entry's
basename **and** the window's Wayland `app_id` (`APP_ID` in the GUI's
`main.rs`). A compositor matches a running window to its installed desktop
entry by that string and by nothing else, so if one of the three changes and
the others do not, the taskbar shows a placeholder instead of the icon.

## Building the packages

```bash
make release    # the binaries, in a Debian 12 container — see ../Containerfile
make packages   # tarball + .deb + .rpm + SHA256SUMS, into target/dist/
```

`make packages` deliberately does not depend on `make release`: it would
rebuild 27 MB of GUI three times over to produce three files from the same two
binaries. Run `release` first; each package target checks the binaries are
there and says so if they are not.

Neither `dpkg` nor `rpmbuild` has to exist on the machine doing this —
`cargo-deb` and `cargo-generate-rpm` write both formats natively, which is
what makes an Arch box a usable release machine:

```bash
cargo install cargo-deb cargo-generate-rpm
```

### Why the version changes shape

`1.0.0-beta.2` is a valid semver prerelease and an invalid package version
nearly everywhere else.

| | |
| --- | --- |
| Cargo | `1.0.0-beta.2` |
| deb / rpm | `1.0.0~beta.2` — `~` sorts *before* `1.0.0`, `-` does not |
| AUR | `1.0.0beta.2` — hyphens are not allowed at all |

`vercmp 1.0.0beta.2 1.0.0` is `-1`, so the Arch version orders correctly too
and the stable release will need no `epoch`. The Makefile derives the deb and
rpm forms from `Cargo.toml`; the two PKGBUILDs carry theirs by hand.

## Verifying a package before it goes out

```bash
podman run --rm -v "$PWD/target/dist:/d:Z" docker.io/library/debian:bookworm \
  sh -c 'apt-get update -qq && apt-get install -y /d/lightforge_*.deb && lightforge --version'

podman run --rm -v "$PWD/target/dist:/d:Z" registry.fedoraproject.org/fedora:41 \
  sh -c 'dnf install -y /d/lightforge-*.rpm && lightforge --version'

podman run --rm -v "$PWD/packaging:/p:Z" registry.fedoraproject.org/fedora:41 \
  sh -c 'dnf install -y -q desktop-file-utils appstream &&
         desktop-file-validate /p/pro.tuxcore.Lightforge.desktop &&
         appstreamcli validate /p/pro.tuxcore.Lightforge.metainfo.xml'
```

All three pass as of 1.0.0-beta.1. `appstreamcli --pedantic` has one remaining
note — the component id contains capitals — which is a style preference
(`org.gnome.Software` has them too) and not an error.

## The public mirror

Upstream is Forgejo, but that instance asks for a login for anything except
`git clone`: no raw files, no API, no release attachments. A package manager
cannot download from it, so releases are published to a GitHub mirror and
everything user-facing — the PKGBUILD sources, the AppStream screenshot, the
homepage field in both packages — points there.

It fails quietly, which is why it was missed until `1.0.0-beta.2`: an
anonymous `GET` of a Forgejo raw URL answers **200 with the sign-in page**,
not 401. A software centre fetching the screenshot that way shows no
screenshot and no error. Every browser-facing URL in the AppStream file —
homepage, bugtracker, help, vcs-browser and the screenshot — is therefore a
`github.com` or `raw.githubusercontent.com` one. The `git clone` URLs in the
docs stay on Forgejo, because cloning is the one thing that does work.

Setting it up once:

```bash
# an empty repo at github.com/LightforgeProject/lightforge first — no README,
# no licence, nothing to merge around
git remote add github git@github.com:LightforgeProject/lightforge.git
git push github main
```

Then either keep pushing to both by hand, or make one `git push` do it:

```bash
git remote set-url --add --push origin https://git.tuxcore.pro/kenny/lightforge.git
git remote set-url --add --push origin git@github.com:LightforgeProject/lightforge.git
```

Forgejo can also do it by itself — **Settings → Mirror Settings → Push
Mirror**, with a GitHub token that has `repo` scope — which keeps the mirror
current even when a push came from somewhere else.

## Cutting a release

```bash
make check-cli-doc        # docs/CLI.md matches the binary
make release              # binaries + the glibc floor check
make packages             # tarball, .deb, .rpm, SHA256SUMS

git tag -a v1.0.0-beta.2 -m 'Lightforge 1.0.0-beta.2'
git push origin v1.0.0-beta.2
git push github v1.0.0-beta.2

gh release create v1.0.0-beta.2 \
  --repo LightforgeProject/lightforge \
  --title 'Lightforge 1.0.0-beta.2' \
  --prerelease \
  target/dist/*
```

Then fill the two `sha256sums` in the PKGBUILDs — they are `SKIP` until the
tag exists, because the sums are of files GitHub has not generated yet:

- `lightforge` hashes GitHub's own source archive
  (`.../archive/refs/tags/v1.0.0-beta.2.tar.gz`), which is not byte-identical
  to anything produced locally. `updpkgsums` fetches it and fills it in.
- `lightforge-bin` hashes the tarball `make packages` just built, so its sum
  is already in `target/dist/SHA256SUMS`.

## The AUR, when it will have us

**Not published yet: AUR registrations are closed**, so there is no account to
upload from. Nothing here is wasted — both PKGBUILDs work today, and until the
AUR reopens they are the Arch install path:

```bash
git clone https://git.tuxcore.pro/kenny/lightforge.git
cd lightforge/packaging/aur/lightforge-bin   # or ../lightforge to build it
makepkg -si
```

Both were built and installed in a clean `archlinux` container before being
committed.

When registration reopens, each package is its own AUR git repo:

```bash
git clone ssh://aur@aur.archlinux.org/lightforge.git aur-lightforge
cp packaging/aur/lightforge/PKGBUILD aur-lightforge/
cd aur-lightforge
makepkg --printsrcinfo > .SRCINFO     # mandatory, and must match the PKGBUILD
git add PKGBUILD .SRCINFO && git commit -m 'Lightforge 1.0.0-beta.2' && git push
```

`.SRCINFO` is generated rather than written, and is not kept in this repo for
that reason: it would be a second copy of the PKGBUILD, free to drift.

## Flatpak

Not done yet. The pieces it needs — the app id, the desktop entry and the
AppStream file — are all here and all validate, which was most of the work.

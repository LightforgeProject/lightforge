# Packaging

Everything a release needs that is not the binaries themselves: the desktop
entry, the icons, the AppStream metadata, and the three packages built from
them.

```text
packaging/
├── pro.tuxcore.Lightforge.desktop        the menu entry
├── pro.tuxcore.Lightforge.metainfo.xml   AppStream, for software centres
├── icons/hicolor/…                       16px → 512px, plus the SVG
├── installer/
│   ├── lightforge-installer.desktop      the double-click installer
│   └── install.sh                        what it runs, fetched from main
├── rpm/lightforge.spec                   the Fedora COPR package, from source
└── aur/
    ├── lightforge/PKGBUILD               builds from the tagged source
    └── lightforge-bin/PKGBUILD           installs the released binaries
```

## The installer

`lightforge-installer.desktop` is a release asset for people who will never
open a terminal, Steam Deck owners first. Double-clicked, it opens a terminal
window and runs `install.sh` from the `main` branch on GitHub, which finds the
newest release, checks it against `SHA256SUMS`, and installs it into the home
folder: the binaries in `~/.local/bin`, the menu entry and icons under
`~/.local/share`. Running it again updates.

Because it is fetched from `main`, a fix to `install.sh` reaches everyone
without a release, and nothing in it is reachable before it is merged there.
To test a change before that, run it straight from the tree:
`bash packaging/installer/install.sh [tarball]`.

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
make packages   # tarball + .deb + .rpm + installer + SHA256SUMS, into target/dist/
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

`1.0.0-beta.5` is a valid semver prerelease and an invalid package version
nearly everywhere else.

| | |
| --- | --- |
| Cargo | `1.0.0-beta.5` |
| deb / rpm | `1.0.0~beta.5` — `~` sorts *before* `1.0.0`, `-` does not |
| AUR | `1.0.0beta.5` — hyphens are not allowed at all |

`vercmp 1.0.0beta.5 1.0.0` is `-1`, so the Arch version orders correctly too
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

All three pass as of 1.0.0-beta.4. `appstreamcli --pedantic` has one remaining
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

git tag -a v1.0.0-beta.5 -m 'Lightforge 1.0.0-beta.5'
git push origin v1.0.0-beta.5
git push github v1.0.0-beta.5

gh release create v1.0.0-beta.5 \
  --repo LightforgeProject/lightforge \
  --title 'Lightforge 1.0.0-beta.5' \
  --prerelease \
  target/dist/*
```

Then fill the two `sha256sums` in the PKGBUILDs — they are `SKIP` until the
tag exists, because the sums are of files GitHub has not generated yet:

- `lightforge` hashes GitHub's own source archive
  (`.../archive/refs/tags/v1.0.0-beta.5.tar.gz`), which is not byte-identical
  to anything produced locally. `updpkgsums` fetches it and fills it in.
- `lightforge-bin` hashes the tarball `make packages` just built, so its sum
  is already in `target/dist/SHA256SUMS`.

Last, trigger the COPR build (see below). It fetches the tag from GitHub, so
it has to come after the push to `github`.

## Fedora COPR

`packaging/rpm/lightforge.spec` builds from the tag's source archive on
GitHub, so COPR compiles it against each Fedora release's own libraries. The
glibc floor that the deb and the rpm in `target/dist` need does not apply to it.
`.copr/Makefile` is COPR's entry point: it downloads that archive and packs the
source RPM.

Users install it with:

```bash
sudo dnf copr enable simplykenny/lightforge
sudo dnf install lightforge
```

**The project, set up once** at copr.fedorainfracloud.org:

- New Project `lightforge`, chroots `fedora-*-x86_64` (x86_64 only, because
  Proton is), and **"Enable internet access during builds"** ticked. Cargo
  fetches the crates against the tag's `Cargo.lock`, and without network the
  build dies at the first one.
- Packages → New Package → **SCM**: clone URL
  `https://github.com/LightforgeProject/lightforge`, committish `main`,
  build method **make_srpm**. Leave the spec field empty: `.copr/Makefile`
  names it.

**Each release:**

1. Before tagging, set `%global upstream_version` in the spec to the new
   version and add a `%changelog` entry. `make packages` refuses to run
   (`check-spec`) while the spec and `Cargo.toml` disagree.
2. After the tag is on GitHub and `main` has been pushed there, go to
   Packages → lightforge → **Rebuild** (or run `copr-cli buildscm`). The
   spec on `main` names the tag, so building from `dev` would fetch a tag
   that might not exist yet.

Verified locally the way COPR runs it, with the spec and `.copr/` copied into
a `fedora:43` container:

```bash
make -f .copr/Makefile srpm outdir=/out spec=packaging/rpm/lightforge.spec
dnf builddep /out/*.src.rpm && rpmbuild --rebuild /out/*.src.rpm
```

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
git add PKGBUILD .SRCINFO && git commit -m 'Lightforge 1.0.0-beta.5' && git push
```

`.SRCINFO` is generated rather than written, and is not kept in this repo for
that reason: it would be a second copy of the PKGBUILD, free to drift.

## Flatpak

Not done yet. The pieces it needs — the app id, the desktop entry and the
AppStream file — are all here and all validate, which was most of the work.

# Fedora COPR package, built from the tagged source on GitHub.
#
# The one line a release edits is `upstream_version`: `Version` is derived from
# it, because rpm orders `1.0.0-beta.5` after `1.0.0` and `1.0.0~beta.5`
# before it. `make check-spec` fails while the two disagree with Cargo.toml.
%global upstream_version 1.0.0-beta.8

# Fedora's CFLAGS carry `-flto=auto`, and `ring` and `zstd-sys` compile C of
# their own: the objects come out as GCC bitcode and the link fails with a
# screen of `undefined symbol: ring_core_0_17_14__*`. Cargo does its own LTO
# anyway, `lto = "thin"` in the workspace's release profile.
%global _lto_cflags %{nil}

# The release profile builds without debug info, so there is nothing for a
# -debuginfo package to hold, and rpmbuild fails on the empty file list.
%global debug_package %{nil}

Name:           lightforge
Version:        %{lua: print((rpm.expand("%{upstream_version}"):gsub("-", "~")))}
Release:        1%{?dist}
Summary:        Install, update and play World of Warcraft on Linux

License:        GPL-3.0-or-later
URL:            https://github.com/LightforgeProject/lightforge
Source0:        %{url}/archive/v%{upstream_version}/%{name}-%{upstream_version}.tar.gz

# Proton is x86_64 only, so a build for anything else would install a
# launcher with nothing to launch.
ExclusiveArch:  x86_64

# Crates are fetched from crates.io during %%build, against the tagged
# Cargo.lock. The COPR project needs "Enable internet access during builds".
BuildRequires:  cargo >= 1.90
BuildRequires:  rust >= 1.90
BuildRequires:  gcc
# gilrs (gamepads) finds libudev through pkg-config.
BuildRequires:  pkgconfig(libudev)
BuildRequires:  desktop-file-utils
BuildRequires:  libappstream-glib

# Vulkan, Wayland and X11 are opened at run time rather than linked, so the
# automatic requires see only libc, libgcc_s and libudev.
Requires:       fontconfig
Requires:       hicolor-icon-theme
# The Battle.net installer is the one 32-bit binary in the whole flow.
Suggests:       wine.i686
Suggests:       mangohud

%description
Lightforge installs, updates and launches World of Warcraft on Linux through
Proton, downloading straight from Blizzard's CDN, and hands every other
Battle.net game to the Battle.net client in the same prefix. It also manages
addons and per-game Proton versions.

%prep
%autosetup -n %{name}-%{upstream_version}

%build
export CARGO_HOME=%{_builddir}/cargo-home
cargo build --release --locked

%install
install -Dm755 target/release/lightforge-gui %{buildroot}%{_bindir}/lightforge-gui
install -Dm755 target/release/lightforge     %{buildroot}%{_bindir}/lightforge

install -Dm644 packaging/pro.tuxcore.Lightforge.desktop \
  %{buildroot}%{_datadir}/applications/pro.tuxcore.Lightforge.desktop
install -Dm644 packaging/pro.tuxcore.Lightforge.metainfo.xml \
  %{buildroot}%{_metainfodir}/pro.tuxcore.Lightforge.metainfo.xml

for size in 16 22 24 32 48 64 128 256 512; do
  install -Dm644 packaging/icons/hicolor/${size}x${size}/apps/pro.tuxcore.Lightforge.png \
    %{buildroot}%{_datadir}/icons/hicolor/${size}x${size}/apps/pro.tuxcore.Lightforge.png
done
install -Dm644 packaging/icons/hicolor/scalable/apps/pro.tuxcore.Lightforge.svg \
  %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/pro.tuxcore.Lightforge.svg

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/pro.tuxcore.Lightforge.desktop
appstream-util validate-relax --nonet %{buildroot}%{_metainfodir}/pro.tuxcore.Lightforge.metainfo.xml
export CARGO_HOME=%{_builddir}/cargo-home
# The tests that reach Blizzard's CDN are #[ignore]d; what runs here is
# offline and takes seconds.
cargo test --release --locked --workspace

%files
%license LICENSE
%doc README.md RELEASES.md TROUBLESHOOTING.md docs/CLI.md
%{_bindir}/lightforge
%{_bindir}/lightforge-gui
%{_datadir}/applications/pro.tuxcore.Lightforge.desktop
%{_metainfodir}/pro.tuxcore.Lightforge.metainfo.xml
%{_datadir}/icons/hicolor/*/apps/pro.tuxcore.Lightforge.*

%changelog
* Thu Oct 29 2026 Kenny <onlyusemycontroler@gmail.com> - 1.0.0~beta.8-1
- Lightforge 1.0.0-beta.8; release notes are in RELEASES.md

* Sun Oct 25 2026 Kenny <onlyusemycontroler@gmail.com> - 1.0.0~beta.7-1
- Lightforge 1.0.0-beta.7; release notes are in RELEASES.md

* Tue Oct 20 2026 Kenny <onlyusemycontroler@gmail.com> - 1.0.0~beta.6-1
- Lightforge 1.0.0-beta.6; release notes are in RELEASES.md

* Mon Oct 19 2026 Kenny <onlyusemycontroler@gmail.com> - 1.0.0~beta.5-1
- First COPR build; release notes are in RELEASES.md
